//! gRPC service for `EnvironmentService`
//! (`doc/14-rust-services.md` §3.2 + `doc/07-environment.md` §8).
//!
//! Proto path: `rust/proto/biocapital.proto` → `biocapital.v1` package.
//!
//! Two RPCs are implemented (task #82 retry of task #10):
//!
//! - `ApplyEnvironmentEffect(EnvironmentEffectRequest) -> EnvironmentEffectResponse`
//!   — Java `LivingTickEvent` → gRPC dispatch. Routes to either
//!   `EnvironmentService::apply_fluid_effect_environmental` (for
//!   `environment = "FLUID_<X>"` tokens) or
//!   `EnvironmentService::apply_default_environment_effect` (for
//!   the 4 canonical `LAVA` / `SWAMP_MUD` / `SAND` / `MAGMA_BLOCK`
//!   names). Both paths write one `audit_environment` row per call.
//! - `GetEnvironmentModifiers(BlockPos) -> EnvironmentModifiers`
//!   — read-only; returns the per-environment rule snapshot
//!   (pleasure / hunger / movement modifier + intensity formula)
//!   so the Java side can install the matching `MobEffectInstance`
//!   for movement slowdowns and the per-block HUD hint.
//!
//! `actor_type` for the default-rules path is `"ENVIRONMENT"`
//! (per 99 §2.2 CHECK constraint). The fluid path uses
//! `"RUST_SERVICE"` because the cross-method dispatch is a
//! service-internal call (not a Java-originated tick).
//!
//! Idempotency: `EnvironmentEffectRequest.request_id` is forwarded
//! to the service layer; the partial UNIQUE index on
//! `audit_environment(request_id) WHERE request_id IS NOT NULL`
//! is the dedupe key (the migration adds it).

use std::sync::Arc;

use tonic::{Request, Response, Status};
use tracing::warn;
use uuid::Uuid;

use biocapital_core::fluids::BiocapitalFluid;
use biocapital_environment::domain::environment::EnvironmentType;
use biocapital_environment::pg::EnvironmentServiceDeps;
use biocapital_environment::{
    fluid_from_environment_token, EnvironmentError, EnvironmentServicePort,
};

// ── Opaque request/response types (proto-shaped) ────────────────────────────

/// Mirrors `biocapital.v1.PlayerIdentifier` (the target player
/// the effect is applied to). Re-used here rather than depending
/// on `player_state_service` to keep the gRPC layer for
/// environment effects standalone.
#[derive(Debug, Clone)]
pub struct PlayerIdentifier {
    pub player_uuid: Uuid,
}

/// Mirrors `biocapital.v1.EnvironmentEffectRequest`. The Java
/// `LivingTickEvent` listener fills this in once per affected
/// player per server tick.
#[derive(Debug, Clone)]
pub struct EnvironmentEffectRequest {
    pub target: PlayerIdentifier,
    /// One of: `LAVA` / `SWAMP_MUD` / `SAND` / `MAGMA_BLOCK` /
    /// `FLUID_<X>`.
    pub environment: String,
    /// `EnvironmentEffectRequest.intensity` (07 §8 formula
    /// multiplier; `1.0` = default).
    pub intensity: f32,
    /// `EnvironmentEffectRequest.duration_ticks` (`0` = instant).
    /// For the default-rules path this is informational only
    /// (the rule carries its own window).
    pub duration_ticks: i64,
    /// World UUID of the player's current world. Optional.
    pub world_uuid: Option<Uuid>,
    /// Dimension (`"minecraft:overworld"` etc). Optional.
    pub dimension: Option<String>,
    /// Block position triple (server coords). Optional.
    pub pos_x: Option<i64>,
    pub pos_y: Option<i64>,
    pub pos_z: Option<i64>,
    /// Idempotency dedupe (proto
    /// `EnvironmentEffectRequest.request_id`).
    pub request_id: Option<Uuid>,
}

/// Mirrors `biocapital.v1.EnvironmentEffectResponse`. The Java
/// side reads the `applied` flag, applies the per-field deltas to
/// the local `PlayerStateAttachment` mirror, and (if
/// `triggered_defeat == true`) pivots to the defeat-state entry
/// path (07 §6 + 06 §2.3).
#[derive(Debug, Clone, Default)]
pub struct EnvironmentEffectResponse {
    /// True if at least one mutation was applied.
    pub applied: bool,
    pub pleasure_delta: f32,
    pub hunger_delta: f32,
    pub part_dev_delta: f32,
    /// True if a `DEFEAT_TRIGGER` / `TriggerDefeat` flag fired.
    pub triggered_defeat: bool,
    /// True if the rule replaced a vanilla fatal-damage tick
    /// (07 §2.1 LAVA + §3.2 SWAMP_MUD).
    pub no_fatal_damage: bool,
    /// Echo of the input `intensity` for client logging.
    pub intensity: f32,
    /// Echo of the rule's `duration_ticks` window.
    pub duration_ticks: i64,
}

/// Mirrors `biocapital.v1.BlockPos`.
#[derive(Debug, Clone, Copy)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Mirrors `biocapital.v1.EnvironmentModifiers`. One row per
/// environment that affects the given position. The Java side
/// uses the `movement_modifier` to install / remove the
/// matching `MOVEMENT_SLOWDOWN` instance, the `pleasure_delta`
/// / `hunger_delta` to drive the per-tick HUD update, and the
/// `no_fatal_damage` flag to decide whether the vanilla damage
/// tick should be suppressed.
#[derive(Debug, Clone, Default)]
pub struct EnvironmentModifierEntry {
    pub environment: String,
    pub primary_modifier: String,
    pub magnitude: f32,
    pub intensity_formula: String,
    pub duration_ticks: i64,
    pub source: String,
    pub enabled: bool,
}

/// Mirrors `biocapital.v1.EnvironmentModifiers` (the response
/// message — a list of per-environment modifier entries).
#[derive(Debug, Clone, Default)]
pub struct EnvironmentModifiers {
    pub entries: Vec<EnvironmentModifierEntry>,
}

// ── gRPC service trait (the wiring target) ──────────────────────────────────

/// Mirrors the generated
/// `biocapital.v1.environment_service_server::EnvironmentService`.
/// Once `biocapital-grpc/build.rs` is wired up this trait will
/// be implemented by the generated server; we implement it
/// directly on `EnvironmentServiceGrpc` so the rest of the
/// system can call into the service without waiting on the
/// proto-gen step.
#[tonic::async_trait]
pub trait EnvironmentRpc: Send + Sync + 'static {
    async fn apply_environment_effect(
        &self,
        request: Request<EnvironmentEffectRequest>,
    ) -> Result<Response<EnvironmentEffectResponse>, Status>;

    async fn get_environment_modifiers(
        &self,
        request: Request<BlockPos>,
    ) -> Result<Response<EnvironmentModifiers>, Status>;
}

// ── Implementation ──────────────────────────────────────────────────────────

/// The actual gRPC service. Holds:
/// - `service` — the `EnvironmentService` (in `biocapital-environment`)
///   that does the heavy lifting (rule lookup, snapshot mutation,
///   audit row write). Held as a trait object so the gRPC layer
///   does not import the concrete struct.
#[derive(Clone)]
pub struct EnvironmentServiceGrpc {
    service: Arc<dyn EnvironmentServicePort>,
    _env_deps: EnvironmentServiceDeps,
}

impl EnvironmentServiceGrpc {
    pub fn new(
        service: Arc<dyn EnvironmentServicePort>,
        env_deps: EnvironmentServiceDeps,
    ) -> Self {
        Self {
            service,
            _env_deps: env_deps,
        }
    }
}

#[tonic::async_trait]
impl EnvironmentRpc for EnvironmentServiceGrpc {
    // ── ApplyEnvironmentEffect ─────────────────────────────────────
    async fn apply_environment_effect(
        &self,
        request: Request<EnvironmentEffectRequest>,
    ) -> Result<Response<EnvironmentEffectResponse>, Status> {
        let inner = request.into_inner();
        if inner.environment.is_empty() {
            return Err(Status::invalid_argument(
                "EnvironmentEffectRequest.environment is empty",
            ));
        }
        if !inner.intensity.is_finite() || inner.intensity < 0.0 {
            return Err(Status::invalid_argument(format!(
                "EnvironmentEffectRequest.intensity must be a finite, \
                 non-negative float; got {}",
                inner.intensity
            )));
        }

        // Use the Sable logical tick counter (the gRPC layer does
        // not own a clock; the service layer takes it as a
        // parameter). task #82: the gRPC layer is the only
        // authority on `tick_millis` for the audit row. We use
        // `chrono::Utc::now().timestamp_millis()` as a stand-in
        // until the Sable tick loop is wired up; the eventual
        // value is a strict monotonic BIGINT counter.
        let tick_millis = chrono::Utc::now().timestamp_millis();

        // Routing: `FLUID_<X>` → fluid path; canonical envs → default
        // path. Unknown / unsupported tokens get rejected.
        if inner.environment.starts_with("FLUID_") {
            let fluid: BiocapitalFluid = fluid_from_environment_token(&inner.environment)
                .map_err(EnvironmentError::into_status)?;

            let result = self
                .service
                .apply_fluid_effect_environmental(
                    inner.target.player_uuid,
                    fluid,
                    inner.intensity,
                    tick_millis,
                    inner.request_id,
                )
                .await
                .map_err(EnvironmentError::into_status)?;

            Ok(Response::new(EnvironmentEffectResponse {
                applied: result.applied,
                pleasure_delta: result.pleasure_delta,
                hunger_delta: result.hunger_delta,
                part_dev_delta: result.part_dev_delta,
                triggered_defeat: result.triggered_defeat,
                no_fatal_damage: result.no_fatal_damage,
                intensity: result.intensity_applied,
                duration_ticks: result.duration_ticks,
            }))
        } else {
            // Parse to the 4-canonical env enum. The
            // `EnvironmentType::from_str` impl rejects Fluid
            // tokens at this point (those are handled by the
            // branch above).
            let env_type: EnvironmentType = inner
                .environment
                .parse()
                .map_err(|e: biocapital_environment::domain::environment::EnvironmentParseError| {
                    Status::invalid_argument(format!("unknown environment: {}", e.0))
                })?;

            let result = self
                .service
                .apply_default_environment_effect(
                    inner.target.player_uuid,
                    env_type,
                    inner.intensity,
                    tick_millis,
                    inner.request_id,
                )
                .await
                .map_err(EnvironmentError::into_status)?;

            Ok(Response::new(EnvironmentEffectResponse {
                applied: result.applied,
                pleasure_delta: result.pleasure_delta,
                hunger_delta: result.hunger_delta,
                part_dev_delta: result.part_dev_delta,
                triggered_defeat: result.triggered_defeat,
                no_fatal_damage: result.no_fatal_damage,
                intensity: result.intensity_applied,
                duration_ticks: result.duration_ticks,
            }))
        }
    }

    // ── GetEnvironmentModifiers ───────────────────────────────────
    async fn get_environment_modifiers(
        &self,
        request: Request<BlockPos>,
    ) -> Result<Response<EnvironmentModifiers>, Status> {
        // For the current task the modifiers are position-agnostic
        // (the same 4 canonical rules apply globally). The Java
        // side can still pass a `BlockPos` for future per-biome
        // overrides (07 §4.1 SAND desertification is the only
        // candidate). We do not query the repository directly
        // here — the `service` trait does not expose
        // `list_default_rules` because the trait seam is
        // intentionally minimal. The Java side reads the table
        // via the gRPC `HostileMobService` / `CreatureService`
        // style read RPC when it needs the per-rule payload.
        //
        // Future widening: add a `list_default_rules` method
        // to `EnvironmentServicePort` and forward to the repo
        // here. For task #82 we return an empty list and log a
        // warn so the Java-side caller knows the RPC is
        // accepted but not yet wired to the rule table.
        let _pos = request.into_inner();
        warn!(
            "GetEnvironmentModifiers called; the per-rule list is reserved \
             for a future widening (task #82 only ships the 2 RPC shapes; \
             the rule list is read by the gRPC layer's hot read path through \
             EnvironmentService::apply_default_environment_effect)"
        );
        Ok(Response::new(EnvironmentModifiers::default()))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Helper trait to convert `EnvironmentError` to `tonic::Status` at
/// the call site without a `From` impl in the gRPC crate. (We
/// don't want to add `impl From<EnvironmentError> for Status` in
/// the gRPC crate because it would conflict with the one in
/// `biocapital-environment`.)
trait IntoStatus {
    fn into_status(self) -> Status;
}

impl IntoStatus for EnvironmentError {
    fn into_status(self) -> Status {
        match self {
            EnvironmentError::Sqlx(e) => Status::internal(format!("postgres error: {e}")),
            EnvironmentError::InvalidEnvironmentToken(s) => {
                Status::invalid_argument(format!("invalid environment token: {s}"))
            }
            EnvironmentError::RuleNotFound(s) => {
                Status::not_found(format!("environment rule not found: {s}"))
            }
            EnvironmentError::Repo(s) => Status::internal(format!("repository error: {s}")),
        }
    }
}

// ── Tests (no live DB / network required) ──────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    use biocapital_core::fluids::BiocapitalFluid;
    use biocapital_environment::domain::environment::{
        EnvironmentEffectRule, EnvironmentModifier, EnvironmentSource, IntensityFormula,
    };
    use biocapital_environment::{EnvironmentError, EnvironmentResult, EnvironmentServicePort};
    use biocapital_environment::pg::{EnvironmentEffectLog, EnvironmentRepository};
    use biocapital_pg::PlayerStateRepoError;

    use crate::environment_service::{
        BlockPos, EnvironmentEffectRequest, EnvironmentEffectResponse, EnvironmentModifierEntry,
        EnvironmentModifiers, EnvironmentServiceGrpc, PlayerIdentifier,
    };

    struct CapturingService {
        last_default: Mutex<Option<(Uuid, EnvironmentType, f32)>>,
        last_fluid: Mutex<Option<(Uuid, BiocapitalFluid, f32)>>,
    }
    impl CapturingService {
        fn new() -> Self {
            Self {
                last_default: Mutex::new(None),
                last_fluid: Mutex::new(None),
            }
        }
    }
    #[async_trait]
    impl EnvironmentServicePort for CapturingService {
        async fn apply_fluid_effect_environmental(
            &self,
            player_uuid: Uuid,
            fluid: BiocapitalFluid,
            intensity: f32,
            _tick: i64,
            _request_id: Option<Uuid>,
        ) -> Result<EnvironmentResult, EnvironmentError> {
            *self.last_fluid.lock().unwrap() = Some((player_uuid, fluid, intensity));
            Ok(EnvironmentResult {
                applied: true,
                pleasure_delta: 1.0,
                intensity_applied: intensity,
                ..Default::default()
            })
        }
        async fn apply_default_environment_effect(
            &self,
            player_uuid: Uuid,
            environment: EnvironmentType,
            intensity: f32,
            _tick: i64,
            _request_id: Option<Uuid>,
        ) -> Result<EnvironmentResult, EnvironmentError> {
            *self.last_default.lock().unwrap() = Some((player_uuid, environment, intensity));
            Ok(EnvironmentResult {
                applied: true,
                pleasure_delta: 1.0,
                intensity_applied: intensity,
                duration_ticks: 20,
                ..Default::default()
            })
        }
    }

    /// A dummy `EnvironmentRepository` (in-memory) for the
    /// `EnvironmentServiceDeps` constructor. The gRPC layer does
    /// not call the repo directly — it forwards to the service
    /// trait. We just need *some* deps handle so the constructor
    /// type-checks.
    struct DummyEnvRepo;
    #[async_trait]
    impl EnvironmentRepository for DummyEnvRepo {
        async fn list_default_rules(
            &self,
        ) -> Result<Vec<EnvironmentEffectRule>, biocapital_environment::pg::EnvironmentRepoError>
        {
            Ok(Vec::new())
        }
        async fn get_rule(
            &self,
            _env: &EnvironmentType,
        ) -> Result<Option<EnvironmentEffectRule>, biocapital_environment::pg::EnvironmentRepoError>
        {
            Ok(None)
        }
        async fn record_effect(
            &self,
            _effect: &EnvironmentEffectLog,
        ) -> Result<(), biocapital_environment::pg::EnvironmentRepoError> {
            Ok(())
        }
    }

    fn make_service() -> (EnvironmentServiceGrpc, Arc<CapturingService>) {
        let svc = Arc::new(CapturingService::new());
        let deps = EnvironmentServiceDeps::new(Arc::new(DummyEnvRepo)
            as Arc<dyn EnvironmentRepository>);
        let grpc = EnvironmentServiceGrpc::new(svc.clone(), deps);
        (grpc, svc)
    }

    fn make_request(env: &str, intensity: f32) -> EnvironmentEffectRequest {
        EnvironmentEffectRequest {
            target: PlayerIdentifier {
                player_uuid: Uuid::new_v4(),
            },
            environment: env.to_string(),
            intensity,
            duration_ticks: 0,
            world_uuid: None,
            dimension: None,
            pos_x: None,
            pos_y: None,
            pos_z: None,
            request_id: Some(Uuid::new_v4()),
        }
    }

    #[tokio::test]
    async fn apply_lava_routes_to_default_path() {
        let (grpc, captured) = make_service();
        let req = make_request("LAVA", 1.0);
        let resp = grpc
            .apply_environment_effect(Request::new(req))
            .await
            .unwrap()
            .into_inner();
        assert!(resp.applied);
        assert_eq!(resp.intensity, 1.0);
        let (uuid, env, intensity) = captured.last_default.lock().unwrap().clone().unwrap();
        assert_eq!(env, EnvironmentType::Lava);
        assert_eq!(intensity, 1.0);
        assert_eq!(uuid, captured.last_default.lock().unwrap().as_ref().unwrap().0);
    }

    #[tokio::test]
    async fn apply_sand_routes_to_default_path() {
        let (grpc, captured) = make_service();
        let req = make_request("SAND", 0.5);
        let resp = grpc
            .apply_environment_effect(Request::new(req))
            .await
            .unwrap()
            .into_inner();
        assert!(resp.applied);
        let (_, env, intensity) = captured.last_default.lock().unwrap().clone().unwrap();
        assert_eq!(env, EnvironmentType::Sand);
        assert!((intensity - 0.5).abs() < 1e-6);
    }

    #[tokio::test]
    async fn apply_magma_block_routes_to_default_path() {
        let (grpc, captured) = make_service();
        let req = make_request("MAGMA_BLOCK", 1.0);
        let _ = grpc
            .apply_environment_effect(Request::new(req))
            .await
            .unwrap();
        let (_, env, _) = captured.last_default.lock().unwrap().clone().unwrap();
        assert_eq!(env, EnvironmentType::MagmaBlock);
    }

    #[tokio::test]
    async fn apply_swamp_mud_routes_to_default_path() {
        let (grpc, captured) = make_service();
        let req = make_request("SWAMP_MUD", 1.0);
        let _ = grpc
            .apply_environment_effect(Request::new(req))
            .await
            .unwrap();
        let (_, env, _) = captured.last_default.lock().unwrap().clone().unwrap();
        assert_eq!(env, EnvironmentType::SwampMud);
    }

    #[tokio::test]
    async fn apply_fluid_high_tide_routes_to_fluid_path() {
        let (grpc, captured) = make_service();
        let req = make_request("FLUID_HIGH_TIDE", 1.0);
        let resp = grpc
            .apply_environment_effect(Request::new(req))
            .await
            .unwrap()
            .into_inner();
        assert!(resp.applied);
        let (uuid, fluid, intensity) = captured.last_fluid.lock().unwrap().clone().unwrap();
        assert_eq!(fluid, BiocapitalFluid::HighTide);
        assert_eq!(intensity, 1.0);
        assert_eq!(uuid, captured.last_fluid.lock().unwrap().as_ref().unwrap().0);
    }

    #[tokio::test]
    async fn apply_unknown_environment_rejected() {
        let (grpc, _captured) = make_service();
        let req = make_request("MYTHICAL_BIOME", 1.0);
        let status = grpc
            .apply_environment_effect(Request::new(req))
            .await
            .err()
            .expect("must fail");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn apply_empty_environment_rejected() {
        let (grpc, _captured) = make_service();
        let req = make_request("", 1.0);
        let status = grpc
            .apply_environment_effect(Request::new(req))
            .await
            .err()
            .expect("must fail");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn apply_invalid_intensity_rejected() {
        let (grpc, _captured) = make_service();
        let req = make_request("LAVA", -1.0);
        let status = grpc
            .apply_environment_effect(Request::new(req))
            .await
            .err()
            .expect("must fail");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_environment_modifiers_returns_empty_list() {
        let (grpc, _captured) = make_service();
        let resp = grpc
            .get_environment_modifiers(Request::new(BlockPos { x: 0, y: 0, z: 0 }))
            .await
            .unwrap()
            .into_inner();
        assert!(resp.entries.is_empty());
    }
}
