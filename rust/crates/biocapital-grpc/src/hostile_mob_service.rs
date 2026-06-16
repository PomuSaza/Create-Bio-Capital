//! gRPC service for `HostileMobService`
//! (`doc/14-rust-services.md` §3.2 + `doc/06-hostile-mobs.md` §8.2).
//!
//! Proto path: `rust/proto/biocapital.proto` → `biocapital.v1` package.
//!
//! Two RPCs are implemented:
//!   - `ApplyHostileDamage(DamageRequest) -> DamageResponse` (method_id 0)
//!   - `GetDropChance(CreatureIdRequest) -> DropChanceResponse` (method_id 1)
//!
//! `ApplyHostileDamage` is a **semantic alias** of
//! `PlayerStateService.ApplyDamage`: it carries the exact same
//! `DamageRequest` / `DamageResponse` proto messages and runs
//! the same hidden-HP flooring + `audit_player_state` write.
//! The implementation here does **not** re-implement the
//! damage path; it delegates to a `PlayerStateRpc` trait
//! object so the audit + clamp logic lives in exactly one
//! place. The cross-service indirection is documented in
//! `99 §4` (`HostileMobService → PlayerStateService` is a
//! "shares implementation" relationship, not a "calls over
//! the network" one).
//!
//! `GetDropChance` is independent: it reads
//! `mob_replacements.drop_chance_desire_fragment` for the
//! given creature id and returns the **highest-priority
//! enabled** row's probability. The list form
//! (`vanilla_drops`) is left empty — task #9 only ships the
//! 3 % Desire Fragment slot per 06 §5.2. The full vanilla
//! drop table is the Java-side `LootTable` payload and is
//! passed through untouched per 06 §5.1 ("完全等同原版").
//!
//! `actor_type = "HOSTILE_MOB"` is already mapped by
//! `player_state_service::actor_type_for_source` for the
//! `source ∈ {"zombie", "skeleton", "creeper", "mob"}` set
//! (06 §4.1 hostile-mob damage flow).

use std::sync::Arc;

use tonic::{Request, Response, Status};
use tracing::warn;
// `Uuid` is only referenced by the test module below; `use super::*`
// re-exports the lib's top-level imports.
#[allow(unused_imports)]
use uuid::Uuid;

use biocapital_creature::domain::{MobReplacement, DEFAULT_DESIRE_FRAGMENT_CHANCE};
// `MobReplacementRepository` is only referenced by the test module
// below; `use super::*` re-exports the lib's top-level imports.
#[allow(unused_imports)]
use biocapital_creature::pg::{
    MobReplacementRepository, MobReplacementServiceDeps, MobRepoError,
};

use crate::player_state_service::{
    DamageRequest, DamageResponse, PlayerStateRpc,
};

// ── Opaque request/response types (proto-shaped) ────────────────────────────

/// Mirrors `biocapital.v1.CreatureIdRequest`.
#[derive(Debug, Clone)]
pub struct CreatureIdRequest {
    /// Either a variant `creature_id` (`"variant_zombie"`) or a
    /// vanilla resource location (`"minecraft:zombie"`). The
    /// gRPC layer tries the literal value against
    /// `mob_replacements.creature_id` first, then against
    /// `mob_replacements.vanilla_id`. This is documented in
    /// the task #9 spec; the Java side will pass either form
    /// depending on whether the mob has already been replaced
    /// (variant id) or the raw spawn event just fired
    /// (vanilla id).
    pub creature_id: String,
}

/// Mirrors `biocapital.v1.DropChanceResponse`. `vanilla_drops`
/// is the list of vanilla loot entries the Java side passes
/// through unchanged; the server-side authoritative slot
/// today is just `desire_fragment_chance` (06 §5.2).
#[derive(Debug, Clone, Default)]
pub struct DropChanceResponse {
    pub desire_fragment_chance: f32,
    /// Reserved for the future per-item vanilla drop table
    /// (06 §5.1: "完全等同原版"). Today the Java side reads
    /// the loot table directly from the vanilla `LootTable`
    /// registry; the gRPC layer does not duplicate it.
    pub vanilla_drops: Vec<(String, f32)>,
    /// Whether at least one enabled `mob_replacements` row
    /// matched. `false` means "no override; fall back to the
    /// Java-side default 3 %".
    pub enabled: bool,
}

// ── Event meta (for the JNI dispatch hook) ─────────────────────────────────

/// Mirrors the `EventMeta` shape used by the other gRPC services
/// so the Sable JNI bridge can fire a single
/// `MobReplacedEvent` (99 §3.1) consistently. The bridge
/// currently only consumes `kind = "hostile.attack"` (the
/// damage event); the other kinds are reserved for task #11
/// when the variant-entity spawn fires the bridge.
#[derive(Debug, Clone, Default)]
pub struct EventMeta {
    /// One of: `"none"`, `"hostile.attack"`, `"hostile.replaced"`.
    pub kind: String,
    pub payload_json: String,
}

// ── gRPC service trait (the wiring target) ──────────────────────────────────

/// Mirrors the generated
/// `biocapital.v1.hostile_mob_service_server::HostileMobService`.
/// Once `biocapital-grpc/build.rs` is wired up this trait will
/// be implemented by the generated server; we implement it
/// directly on `HostileMobGrpc` so the rest of the system can
/// call into the service without waiting on the proto-gen step.
#[tonic::async_trait]
pub trait HostileMobRpc: Send + Sync + 'static {
    /// Hostile-mob damage application. Routes through
    /// `PlayerStateService.ApplyDamage` — semantically an
    /// alias (same `DamageRequest` / `DamageResponse`).
    async fn apply_hostile_damage(
        &self,
        request: Request<DamageRequest>,
    ) -> Result<Response<DamageResponse>, Status>;

    /// Drop chance lookup. Returns the highest-priority
    /// enabled row's `drop_chance_desire_fragment`. When no
    /// row matches, returns `enabled = false` and
    /// `desire_fragment_chance = DEFAULT_DESIRE_FRAGMENT_CHANCE`
    /// so the Java side has a sensible default (06 §5.2).
    async fn get_drop_chance(
        &self,
        request: Request<CreatureIdRequest>,
    ) -> Result<Response<DropChanceResponse>, Status>;
}

// ── Implementation ──────────────────────────────────────────────────────────

/// The actual gRPC service. Holds:
/// - `repo` for the `mob_replacements` table
///   (`MobReplacementRepository`);
/// - `player_state` for the `apply_damage` delegation
///   (`PlayerStateRpc` trait object).
///
/// Cheap to clone (`Arc` internals).
#[derive(Clone)]
pub struct HostileMobGrpc {
    deps: MobReplacementServiceDeps,
    /// The downstream `PlayerStateService` to delegate
    /// `apply_hostile_damage` to. We hold it as a trait
    /// object so the gRPC layer does not import
    /// `PlayerStateGrpc` directly (avoids a circular
    /// dependency on the `biocapital-grpc` crate).
    player_state: Arc<dyn PlayerStateRpc>,
}

impl HostileMobGrpc {
    pub fn new(
        deps: MobReplacementServiceDeps,
        player_state: Arc<dyn PlayerStateRpc>,
    ) -> Self {
        Self { deps, player_state }
    }
}

#[tonic::async_trait]
impl HostileMobRpc for HostileMobGrpc {
    // ── ApplyHostileDamage ──────────────────────────────────────
    //
    // 2026-06-14 task #9: this is a semantic alias of
    // `PlayerStateService.ApplyDamage`. The proto messages
    // are identical (per `biocapital.proto` line 319:
    // `ApplyHostileDamage(DamageRequest) returns (DamageResponse)`).
    //
    // We delegate the actual mutation to `PlayerStateRpc::apply_damage`
    // so the audit / hidden-HP / clamp logic lives in exactly
    // one place. The cross-service indirection is a
    // **trait-object call within the same process** — not a
    // network hop. Sable's runtime keeps the two services in
    // the same Tokio task.
    async fn apply_hostile_damage(
        &self,
        request: Request<DamageRequest>,
    ) -> Result<Response<DamageResponse>, Status> {
        // Force the `source` field to look like a hostile-mob
        // hit so the audit `actor_type_for_source` mapping in
        // `player_state_service` returns `HOSTILE_MOB` even if
        // the Java caller forgot to set it. The mapping
        // accepts the literal strings
        // `"zombie" / "skeleton" / "creeper" / "mob"` (see
        // `biocapital-grpc/src/player_state_service.rs`).
        let mut inner = request.into_inner();
        if inner.source.is_none() {
            inner.source = Some("mob".to_string());
        }
        // The mapping already produces `actor_type = "HOSTILE_MOB"`
        // for those four source values; any other source
        // (e.g. `"lava"`) is not a hostile-mob hit and we
        // refuse it here so the audit table stays clean.
        else {
            let s = inner.source.as_deref().unwrap_or("");
            let is_hostile = s.eq_ignore_ascii_case("zombie")
                || s.eq_ignore_ascii_case("skeleton")
                || s.eq_ignore_ascii_case("creeper")
                || s.eq_ignore_ascii_case("mob");
            if !is_hostile {
                return Err(Status::invalid_argument(format!(
                    "ApplyHostileDamage.source must be a hostile mob kind \
                     (zombie/skeleton/creeper/mob); got {s:?}"
                )));
            }
        }

        let damage_req = Request::new(inner);
        // `apply_damage` already returns
        // `Result<Response<DamageResponse>, Status>` — do NOT
        // re-wrap with `.map(Response::new)` (that would produce
        // `Result<Response<Response<DamageResponse>>, _>`).
        self.player_state.apply_damage(damage_req).await
    }

    // ── GetDropChance ──────────────────────────────────────────
    async fn get_drop_chance(
        &self,
        request: Request<CreatureIdRequest>,
    ) -> Result<Response<DropChanceResponse>, Status> {
        let CreatureIdRequest { creature_id } = request.into_inner();
        if creature_id.is_empty() {
            return Err(Status::invalid_argument("creature_id is empty"));
        }

        // Step 1: try matching as a creature_id (variant form).
        // `MobReplacementRepository::get_for_vanilla` keys on
        // the `vanilla_id` column, so we use a small detour:
        // a focused `list(enabled_only=true)` filtered in
        // memory is fine here because the table is tiny
        // (tens of rows for a typical config). When the table
        // grows past ~hundreds, replace this with a dedicated
        // `get_for_creature` query.
        let rows: Vec<MobReplacement> = self
            .deps
            .repo
            .list(true)
            .await
            .map_err(repo_status)?
            .into_iter()
            .filter(|r| r.creature_id == creature_id || r.vanilla_id == creature_id)
            .collect();

        // Step 2: pick the highest-priority enabled row.
        let winner: Option<MobReplacement> = rows.into_iter().reduce(|acc, cand| {
            if cand.is_higher_priority_than(&acc) {
                cand
            } else {
                acc
            }
        });

        match winner {
            Some(r) => Ok(Response::new(DropChanceResponse {
                desire_fragment_chance: r.drop_chance_desire_fragment,
                vanilla_drops: Vec::new(),
                enabled: true,
            })),
            None => {
                // No row matches → return the canonical default
                // (06 §5.2) with `enabled = false` so the Java
                // side knows the row was missing. The Java
                // `MobDropsHandler` still has its legacy 3 %
                // constant as a final fallback.
                warn!(
                    creature_id = %creature_id,
                    "no enabled mob_replacements row matched; returning default"
                );
                Ok(Response::new(DropChanceResponse {
                    desire_fragment_chance: DEFAULT_DESIRE_FRAGMENT_CHANCE,
                    vanilla_drops: Vec::new(),
                    enabled: false,
                }))
            }
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Map a `biocapital_creature::pg::MobRepoError` to a `tonic::Status`.
/// The `MobRepoError` already implements `Into<Status>` itself
/// (mirrors `FluidRepoError`); we keep a local mapping here
/// so future extensions stay in one place. The current
/// `biocapital_creature::pg::mob_replacement::RepoError` only
/// has the two `sqlx` variants — no domain-level `NotFound`
/// / `InvalidUuid` — so this mapper is intentionally tiny.
fn repo_status(e: MobRepoError) -> Status {
    match e {
        MobRepoError::Sqlx(sqlx::Error::RowNotFound) => {
            Status::not_found("mob_replacements row not found")
        }
        MobRepoError::Sqlx(e) => Status::internal(format!("postgres error: {e}")),
        MobRepoError::Migrate(e) => Status::internal(format!("migration error: {e}")),
    }
}

// ── Tests (no live DB / network required) ──────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    use crate::player_state_service::{DamageRequest, DamageResponse, PlayerState, PlayerIdentifier, PlayerStateRpc};

    /// In-memory repo for tests. Holds a fixed set of rows.
    struct MemRepo {
        rows: Mutex<Vec<MobReplacement>>,
    }
    impl MemRepo {
        fn new(rows: Vec<MobReplacement>) -> Self {
            Self { rows: Mutex::new(rows) }
        }
    }
    #[async_trait]
    impl MobReplacementRepository for MemRepo {
        async fn get_for_vanilla(
            &self,
            vanilla_id: &str,
        ) -> Result<Vec<MobReplacement>, MobRepoError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.vanilla_id == vanilla_id && r.enabled)
                .cloned()
                .collect())
        }
        async fn list(
            &self,
            enabled_only: bool,
        ) -> Result<Vec<MobReplacement>, MobRepoError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|r| !enabled_only || r.enabled)
                .cloned()
                .collect())
        }
        async fn upsert(
            &self,
            _replacement: &MobReplacement,
        ) -> Result<(), MobRepoError> {
            Ok(())
        }
        async fn delete(&self, _id: Uuid) -> Result<(), MobRepoError> {
            Ok(())
        }
    }

    /// Stub `PlayerStateRpc` for `apply_hostile_damage`
    /// delegation tests. We capture the inbound request so
    /// the test can assert that the source was normalised to
    /// `"mob"` and the response was forwarded unchanged.
    struct CapturingPlayerState {
        last: Mutex<Option<DamageRequest>>,
    }
    impl CapturingPlayerState {
        fn new() -> Self {
            Self { last: Mutex::new(None) }
        }
    }
    #[async_trait]
    impl PlayerStateRpc for CapturingPlayerState {
        async fn get_state(
            &self,
            _request: Request<PlayerIdentifier>,
        ) -> Result<Response<PlayerState>, Status> {
            Err(Status::unimplemented("not used in hostile_mob tests"))
        }
        async fn update_state(
            &self,
            _request: Request<crate::player_state_service::PlayerStateUpdate>,
        ) -> Result<Response<PlayerState>, Status> {
            Err(Status::unimplemented("not used in hostile_mob tests"))
        }
        async fn apply_damage(
            &self,
            request: Request<DamageRequest>,
        ) -> Result<Response<DamageResponse>, Status> {
            let inner = request.into_inner();
            *self.last.lock().unwrap() = Some(DamageRequest {
                target: PlayerIdentifier { player_uuid: inner.target.player_uuid },
                amount: inner.amount,
                source: inner.source.clone(),
                part: inner.part.clone(),
                request_id: inner.request_id,
            });
            Ok(Response::new(DamageResponse {
                new_state: PlayerState {
                    player_uuid: inner.target.player_uuid,
                    pleasure: 0.0,
                    hunger: 0.0,
                    hidden_hp: 19.0,
                    low_hp_hits: 0,
                    parts: vec![],
                    updated_at_ms: 0,
                    defeat_count: 0,
                    active_contracts: 0,
                    max_hunger: 100,
                },
                killed: false,
            }))
        }
        async fn add_pleasure(
            &self,
            _request: Request<crate::player_state_service::PleasureRequest>,
        ) -> Result<Response<PlayerState>, Status> {
            Err(Status::unimplemented("not used in hostile_mob tests"))
        }
        async fn add_hunger(
            &self,
            _request: Request<crate::player_state_service::HungerRequest>,
        ) -> Result<Response<PlayerState>, Status> {
            Err(Status::unimplemented("not used in hostile_mob tests"))
        }
        async fn add_fluid_effect(
            &self,
            _request: Request<crate::player_state_service::FluidEffectRequest>,
        ) -> Result<Response<PlayerState>, Status> {
            Err(Status::unimplemented("not used in hostile_mob tests"))
        }
    }

    fn sample_replacement(vanilla: &str, creature: &str, chance: f32, priority: i32) -> MobReplacement {
        let mut r = MobReplacement::new(Uuid::new_v4(), vanilla, creature, 0);
        r.drop_chance_desire_fragment = chance;
        r.priority = priority;
        r
    }

    fn make_service(rows: Vec<MobReplacement>) -> (HostileMobGrpc, Arc<CapturingPlayerState>) {
        let repo = Arc::new(MemRepo::new(rows));
        let deps = MobReplacementServiceDeps::new(repo);
        let ps = Arc::new(CapturingPlayerState::new());
        let service = HostileMobGrpc::new(deps, ps.clone());
        (service, ps)
    }

    #[tokio::test]
    async fn get_drop_chance_returns_winner() {
        let rows = vec![
            sample_replacement("minecraft:zombie", "variant_zombie", 0.10, 1),
            sample_replacement("minecraft:zombie", "variant_zombie_halloween", 0.50, 5),
        ];
        let (svc, _ps) = make_service(rows);
        let resp = svc
            .get_drop_chance(Request::new(CreatureIdRequest {
                creature_id: "minecraft:zombie".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!((resp.desire_fragment_chance - 0.50).abs() < 1e-6);
        assert!(resp.enabled);
        assert!(resp.vanilla_drops.is_empty());
    }

    #[tokio::test]
    async fn get_drop_chance_falls_back_to_default_when_no_match() {
        let (svc, _ps) = make_service(vec![]);
        let resp = svc
            .get_drop_chance(Request::new(CreatureIdRequest {
                creature_id: "minecraft:enderman".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!((resp.desire_fragment_chance - DEFAULT_DESIRE_FRAGMENT_CHANCE).abs() < 1e-6);
        assert!(!resp.enabled);
    }

    #[tokio::test]
    async fn get_drop_chance_skips_disabled_rows() {
        let mut disabled = sample_replacement("minecraft:zombie", "variant_zombie", 0.99, 99);
        disabled.enabled = false;
        let (svc, _ps) = make_service(vec![disabled]);
        let resp = svc
            .get_drop_chance(Request::new(CreatureIdRequest {
                creature_id: "minecraft:zombie".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp.enabled);
    }

    #[tokio::test]
    async fn get_drop_chance_rejects_empty_creature_id() {
        let (svc, _ps) = make_service(vec![]);
        let status = svc
            .get_drop_chance(Request::new(CreatureIdRequest {
                creature_id: String::new(),
            }))
            .await
            .err()
            .expect("must fail");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_drop_chance_matches_creature_id_form() {
        let rows = vec![sample_replacement(
            "minecraft:zombie",
            "variant_zombie",
            0.25,
            0,
        )];
        let (svc, _ps) = make_service(rows);
        let resp = svc
            .get_drop_chance(Request::new(CreatureIdRequest {
                creature_id: "variant_zombie".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!((resp.desire_fragment_chance - 0.25).abs() < 1e-6);
        assert!(resp.enabled);
    }

    #[tokio::test]
    async fn apply_hostile_damage_defaults_source_to_mob() {
        let (svc, ps) = make_service(vec![]);
        let req = DamageRequest {
            target: PlayerIdentifier {
                player_uuid: Uuid::new_v4(),
            },
            amount: 1.0,
            source: None,
            part: None,
            request_id: Some(Uuid::new_v4()),
        };
        let resp = svc
            .apply_hostile_damage(Request::new(req))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp.killed);
        // Verify the captured request was normalised to "mob".
        let captured = ps.last.lock().unwrap().clone().expect("captured");
        assert_eq!(captured.source.as_deref(), Some("mob"));
    }

    #[tokio::test]
    async fn apply_hostile_damage_passes_known_hostile_source_through() {
        let (svc, ps) = make_service(vec![]);
        let req = DamageRequest {
            target: PlayerIdentifier {
                player_uuid: Uuid::new_v4(),
            },
            amount: 2.0,
            source: Some("zombie".to_string()),
            part: None,
            request_id: Some(Uuid::new_v4()),
        };
        svc.apply_hostile_damage(Request::new(req))
            .await
            .unwrap();
        let captured = ps.last.lock().unwrap().clone().expect("captured");
        assert_eq!(captured.source.as_deref(), Some("zombie"));
    }

    #[tokio::test]
    async fn apply_hostile_damage_rejects_non_hostile_source() {
        let (svc, _ps) = make_service(vec![]);
        let req = DamageRequest {
            target: PlayerIdentifier {
                player_uuid: Uuid::new_v4(),
            },
            amount: 2.0,
            source: Some("lava".to_string()),
            part: None,
            request_id: Some(Uuid::new_v4()),
        };
        let status = svc
            .apply_hostile_damage(Request::new(req))
            .await
            .err()
            .expect("must fail");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }
}
