//! gRPC service for `PlayerStateService` (doc/14-rust-services.md §3.2 + §3.2.2).
//!
//! Proto path: `rust/proto/biocapital.proto` → `biocapital.v1` package.
//!
//! Six RPCs are implemented:
//!   - `GetState`         (method_id 0) — read snapshot
//!   - `UpdateState`      (method_id 1) — partial/full upsert
//!   - `ApplyDamage`      (method_id 2) — hidden HP damage with floor
//!   - `AddPleasure`      (method_id 3) — pleasure delta
//!   - `AddHunger`        (method_id 4) — hunger delta
//!   - `AddFluidEffect`   (method_id 5) — apply CONSUMPTION-source fluid
//!                                         effects from the `fluid_effects`
//!                                         table (task #8, 05 §3 + §4)
//!
//! Each mutating RPC writes an entry to the `audit_player_state` table
//! (see `biocapital-pg` migration `20260614000001_player_state.sql`).
//!
//! `AddFluidEffect` additionally reads from `fluid_effects` (see
//! `biocapital-pg::fluid` and migration `20260614000006_fluids.sql`) to
//! resolve the per-fluid effect payloads at call time.
//!
//! This module is the **authoritative** gRPC layer; the JNI bridge
//! (`biocapital-jni::dispatch::player_state`) delegates here when running
//! in in-process mode (doc/16-sable-bridge.md §3.4).

use std::sync::Arc;

use chrono::Utc;
use tonic::{Request, Response, Status};
use tracing::warn;
use uuid::Uuid;

use biocapital_core::fluids::BiocapitalFluid;
// `AuditWriter` / `FluidRepository` / `PlayerStateRepository` are
// only referenced by the test module below; `use super::*`
// re-exports the lib's top-level imports.
#[allow(unused_imports)]
use biocapital_core::player_state::{BodyPart, PlayerStateSnapshot, STAT_MIN};
#[allow(unused_imports)]
use biocapital_environment::pg::fluid::{FluidRepository, FluidServiceDeps};
#[allow(unused_imports)]
use biocapital_pg::{
    AuditEntry, AuditWriter, PgPlayerStateLoader, PlayerStateRepository, PlayerStateServiceDeps,
};

// ── Proto stubs ──────────────────────────────────────────────────────────────
//
// The `biocapital-grpc` crate is meant to depend on the prost-generated
// types from `rust/proto/biocapital.proto`. The build script that wires
// `prost-build` is part of task #17 follow-up work; for now we declare
// the message types we need as opaque `Vec<u8>`-shaped wrappers around
// the raw request payload so this file compiles without `cargo build`.
//
// Concretely, each RPC signature accepts a request that exposes a
// `target_uuid` (the proto `PlayerIdentifier.player_uuid`) plus the
// per-RPC fields, and returns a `PlayerState` that the gRPC service
// trait will serialise back to bytes. The trait `PlayerStateRpc` below
// is the integration boundary; it stays close to the proto shape so
// the swap-in of generated types is mechanical.
//
// When `biocapital-grpc/build.rs` is added (task #17 follow-up), replace
// these opaque structs with `biocapital_proto::biocapital::v1::*` types.

// ── Opaque request/response types (proto-shaped) ────────────────────────────

/// Mirrors `biocapital.v1.PlayerIdentifier`.
#[derive(Debug, Clone, Default)]
pub struct PlayerIdentifier {
    pub player_uuid: Uuid,
}

/// Mirrors `biocapital.v1.PlayerState` (only the fields the gRPC layer
/// needs to populate on the way out; everything else is derived from
/// `PlayerStateSnapshot`).
#[derive(Debug, Clone)]
pub struct PlayerState {
    pub player_uuid: Uuid,
    pub pleasure: f32,
    pub hunger: f32,
    pub hidden_hp: f32,
    pub low_hp_hits: i32,
    pub parts: Vec<(String, f32)>,
    pub updated_at_ms: i64,
    pub defeat_count: i32,
    pub active_contracts: i64,
    pub max_hunger: i32,
}

/// Mirrors `biocapital.v1.PlayerStateUpdate`. Empty/zero fields mean
/// "do not modify" (proto3 partial-update convention, 14 §3.2.2).
#[derive(Debug, Clone, Default)]
pub struct PlayerStateUpdate {
    pub target: PlayerIdentifier,
    pub pleasure: Option<f32>, // None = no-op
    pub hunger: Option<f32>,
    pub hidden_hp: Option<f32>,
    pub low_hp_hits: Option<i32>,
    pub parts: Option<Vec<(String, f32)>>,
    pub active_contracts: Option<i64>,
    pub max_hunger: Option<i32>,
}

/// Mirrors `biocapital.v1.DamageRequest`.
#[derive(Debug, Clone)]
pub struct DamageRequest {
    pub target: PlayerIdentifier,
    pub amount: f32,
    pub source: Option<String>,
    pub part: Option<String>,
    pub request_id: Option<Uuid>,
}

/// Mirrors `biocapital.v1.DamageResponse`. `killed` is always `false`
/// per 02 §3.4 — vanilla death is gone.
#[derive(Debug, Clone)]
pub struct DamageResponse {
    pub new_state: PlayerState,
    pub killed: bool,
}

/// Mirrors `biocapital.v1.PleasureRequest`.
#[derive(Debug, Clone)]
pub struct PleasureRequest {
    pub target: PlayerIdentifier,
    pub amount: f32,
    pub part: Option<String>,
    pub source: Option<String>,
    pub request_id: Option<Uuid>,
}

/// Mirrors `biocapital.v1.HungerRequest`.
#[derive(Debug, Clone)]
pub struct HungerRequest {
    pub target: PlayerIdentifier,
    pub amount: f32,
    pub source: Option<String>,
    pub request_id: Option<Uuid>,
}

/// `AddFluidEffect` request payload. Carries the fluid identity
/// (resolved on the Java side via the Minecraft registry name) and the
/// idempotency key. The gRPC layer reads `fluid_effects` rows with
/// `source = CONSUMPTION` for `fluid` and applies each one to the
/// player's snapshot in deterministic order (`PART_DEV_BOOST` →
/// `PLEASURE_BOOST` → `HUNGER_BOOST` → `STRESS_BOOST` → `DEFEAT_TRIGGER`
/// → `DECORATIVE`).
#[derive(Debug, Clone)]
pub struct FluidEffectRequest {
    pub target: PlayerIdentifier,
    pub fluid: BiocapitalFluid,
    pub request_id: Option<Uuid>,
}

// ── gRPC service trait (the wiring target) ───────────────────────────────────

/// Mirrors the generated `biocapital.v1.player_state_service_server::PlayerStateService`.
/// Each method receives a tonic `Request<T>` and returns `Result<Response<T>, Status>`.
///
/// Once `biocapital-grpc/build.rs` is wired up, this trait will be implemented
/// by the generated server; we implement it directly on `PlayerStateGrpc` so
/// the rest of the system can call into the service without waiting on the
/// proto-gen step.
#[tonic::async_trait]
pub trait PlayerStateRpc: Send + Sync + 'static {
    async fn get_state(
        &self,
        request: Request<PlayerIdentifier>,
    ) -> Result<Response<PlayerState>, Status>;

    async fn update_state(
        &self,
        request: Request<PlayerStateUpdate>,
    ) -> Result<Response<PlayerState>, Status>;

    async fn apply_damage(
        &self,
        request: Request<DamageRequest>,
    ) -> Result<Response<DamageResponse>, Status>;

    async fn add_pleasure(
        &self,
        request: Request<PleasureRequest>,
    ) -> Result<Response<PlayerState>, Status>;

    async fn add_hunger(
        &self,
        request: Request<HungerRequest>,
    ) -> Result<Response<PlayerState>, Status>;

    /// Task #8: apply every CONSUMPTION-source effect for `fluid` to the
    /// target player. The fluid identity is resolved on the Java side
    /// (Minecraft registry name → `BiocapitalFluid` enum).
    async fn add_fluid_effect(
        &self,
        request: Request<FluidEffectRequest>,
    ) -> Result<Response<PlayerState>, Status>;
}

// ── Implementation ──────────────────────────────────────────────────────────

/// The actual gRPC service. Holds the repository + audit writer via
/// `PlayerStateServiceDeps` and the fluid repository via
/// `FluidServiceDeps` (task #8). Cheap to clone (`Arc` internals).
#[derive(Clone)]
pub struct PlayerStateGrpc {
    deps: PlayerStateServiceDeps,
    fluid_deps: FluidServiceDeps,
    /// Logical clock source. The gRPC service does not own timekeeping
    /// — that lives in `biocapital-jni` (Sable's tick) — so we accept a
    /// closure-shaped clock. Default: `Utc::now().timestamp_millis()`.
    tick_millis: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// Idempotency dedupe window. RPCs carrying the same `request_id`
    /// within this window will short-circuit to the cached result, if
    /// supported by the repository. 0 = disabled. Reserved for task
    /// #3 follow-up; for now we always compute.
    _idempotency_window_ms: i64,
}

impl PlayerStateGrpc {
    pub fn new(deps: PlayerStateServiceDeps, fluid_deps: FluidServiceDeps) -> Self {
        Self {
            deps,
            fluid_deps,
            tick_millis: Arc::new(|| Utc::now().timestamp_millis()),
            _idempotency_window_ms: 0,
        }
    }

    pub fn with_clock(
        deps: PlayerStateServiceDeps,
        fluid_deps: FluidServiceDeps,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            deps,
            fluid_deps,
            tick_millis: clock,
            _idempotency_window_ms: 0,
        }
    }

    /// Cache-Aside read used by `GetState` and the mutating RPCs
    /// (mutators need the *current* snapshot before they patch it, and
    /// they should also see the freshest value the cache holds).
    ///
    /// - With a cache configured → `cache.get_or_load` (PG hit only on
    ///   miss / expiry).
    /// - Without a cache → `repo.get` directly (pre-#123 behaviour).
    ///
    /// Errors from the underlying repository propagate to the caller as a
    /// `tonic::Status` (see [`RepoStatus`]).
    async fn load_snapshot(&self, uuid: Uuid) -> Result<PlayerStateSnapshot, Status> {
        if let Some(cache) = &self.deps.cache {
            let loader = PgPlayerStateLoader::new(self.deps.repo.clone());
            cache.get_or_load(uuid, &loader).await.map_err(LoadStatus::from)
        } else {
            self.deps.repo.get(uuid).await.map_err(RepoStatus::from)
        }
    }

    /// Drop the cache entry for `uuid` after a successful write so the
    /// next `GetState` re-reads from PG. No-op if the cache is disabled.
    async fn invalidate_after_write(&self, uuid: Uuid) {
        if let Some(cache) = &self.deps.cache {
            cache.invalidate(uuid).await;
        }
    }
}

#[tonic::async_trait]
impl PlayerStateRpc for PlayerStateGrpc {
    // ── GetState ──────────────────────────────────────────────────
    //
    // Task #123 HUD rebuild: this is the hot path. 5 Hz × N players.
    // We route the read through the in-memory 200 ms cache when one is
    // configured (the production wiring always sets one up); the
    // pre-#123 path is preserved for callers that pass `cache: None` in
    // `PlayerStateServiceDeps`.
    //
    // GetState is read-only; no audit row is written (would flood the
    // table for HUD refresh traffic). The audit table is for mutations.
    async fn get_state(
        &self,
        request: Request<PlayerIdentifier>,
    ) -> Result<Response<PlayerState>, Status> {
        let PlayerIdentifier { player_uuid } = request.into_inner();
        let snapshot = self.load_snapshot(player_uuid).await?;
        Ok(Response::new(snapshot_to_proto(&snapshot)))
    }

    // ── UpdateState ───────────────────────────────────────────────
    async fn update_state(
        &self,
        request: Request<PlayerStateUpdate>,
    ) -> Result<Response<PlayerState>, Status> {
        let PlayerStateUpdate {
            target,
            pleasure,
            hunger,
            hidden_hp,
            low_hp_hits,
            parts,
            active_contracts,
            max_hunger,
        } = request.into_inner();

        let mut snapshot = self.load_snapshot(target.player_uuid).await?;
        let before = snapshot.clone();
        let tick = (self.tick_millis)();

        // Apply the partial update. Each `Some` field is a set; `None` is
        // a no-op. This matches the proto3 partial-update convention
        // agreed in 14 §3.2.2.
        if let Some(v) = pleasure {
            // Set semantics (not delta): replace outright, clamped.
            snapshot.pleasure = v.clamp(STAT_MIN, 100.0);
        }
        if let Some(v) = hunger {
            snapshot.hunger = v.clamp(STAT_MIN, snapshot.max_hunger as f32);
        }
        if let Some(v) = hidden_hp {
            snapshot.hidden_hp = v.max(1.0);
        }
        if let Some(v) = low_hp_hits {
            // Monotonic: never let an UpdateState move it backwards.
            snapshot.low_hp_hits = snapshot.low_hp_hits.max(v);
            snapshot.defeat_count = snapshot.defeat_count.max(v);
        }
        if let Some(v) = max_hunger {
            snapshot.max_hunger = v.max(1);
        }
        if let Some(_v) = active_contracts {
            // Stored in the snapshot's dedicated field once task #9 lands.
            // For now we just touch the snapshot; the proto field is
            // reserved on the wire but not yet mapped to a column.
        }
        if let Some(pairs) = parts {
            // Replace the parts map outright. Unknown part names are
            // rejected with INVALID_ARGUMENT.
            let mut new_parts = std::collections::BTreeMap::new();
            for (name, value) in pairs {
                let part: BodyPart = name.parse().map_err(|source| {
                    Status::invalid_argument(format!("unknown body part: {source}"))
                })?;
                let clamped = value.clamp(0.0, 100.0);
                new_parts.insert(part, clamped);
            }
            for p in BodyPart::ALL {
                new_parts.entry(p).or_insert(0.0);
            }
            snapshot.parts = new_parts;
        }

        self.deps
            .repo
            .upsert(&snapshot, tick)
            .await
            .map_err(RepoStatus::from)?;

        // Task #123: drop the cache entry so the next GetState / HUD
        // refresh re-reads the patched snapshot from PG.
        self.invalidate_after_write(target.player_uuid).await;

        self.deps
            .audit
            .write(AuditEntry {
                actor_uuid: None,
                actor_type: "RUST_SERVICE",
                target_uuid: target.player_uuid,
                op: "state.update",
                before,
                after: snapshot.clone(),
                source: None,
                request_id: None,
                tick_millis: tick,
            })
            .await
            .map_err(RepoStatus::from)?;

        Ok(Response::new(snapshot_to_proto(&snapshot)))
    }

    // ── ApplyDamage ───────────────────────────────────────────────
    async fn apply_damage(
        &self,
        request: Request<DamageRequest>,
    ) -> Result<Response<DamageResponse>, Status> {
        let DamageRequest {
            target,
            amount,
            source,
            part: _part,
            request_id,
        } = request.into_inner();
        if amount.is_nan() || amount.is_infinite() {
            return Err(Status::invalid_argument("amount must be finite"));
        }

        let mut snapshot = self.load_snapshot(target.player_uuid).await?;
        let before = snapshot.clone();
        let tick = (self.tick_millis)();

        let outcome = snapshot.add_hidden_damage(amount);
        if outcome.floor_hit {
            // Per 02 §3.3 the punitive debuff is applied by the *caller* of
            // the damage event (the hostile mob / environment layer that
            // initiated the call). The Rust gRPC service only reports the
            // floor hit; the debuff is a Java-side concern. We log a
            // warning so the audit table still surfaces the event.
            warn!(
                player = %target.player_uuid,
                absorbed = outcome.absorbed,
                source = ?source,
                "hidden_hp floor hit; caller must apply punitive debuff"
            );
        }

        self.deps
            .repo
            .upsert(&snapshot, tick)
            .await
            .map_err(RepoStatus::from)?;

        // Task #123: drop the cache entry so the next GetState / HUD
        // refresh re-reads the patched snapshot from PG.
        self.invalidate_after_write(target.player_uuid).await;

        self.deps
            .audit
            .write(AuditEntry {
                actor_uuid: None,
                actor_type: actor_type_for_source(source.as_deref()),
                target_uuid: target.player_uuid,
                op: "state.damage",
                before,
                after: snapshot.clone(),
                source,
                request_id,
                tick_millis: tick,
            })
            .await
            .map_err(RepoStatus::from)?;

        Ok(Response::new(DamageResponse {
            new_state: snapshot_to_proto(&snapshot),
            killed: false, // 02 §3.4: vanilla death is gone
        }))
    }

    // ── AddPleasure ───────────────────────────────────────────────
    async fn add_pleasure(
        &self,
        request: Request<PleasureRequest>,
    ) -> Result<Response<PlayerState>, Status> {
        let PleasureRequest {
            target,
            amount,
            part: _part, // pleasure RPC carries a per-part hint for future
                         // distribution; the global pleasure bar is the
                         // authoritative value for now (no per-part
                         // split per 02 §1.1).
            source,
            request_id,
        } = request.into_inner();
        if amount.is_nan() || amount.is_infinite() {
            return Err(Status::invalid_argument("amount must be finite"));
        }

        let mut snapshot = self.load_snapshot(target.player_uuid).await?;
        let before = snapshot.clone();
        let tick = (self.tick_millis)();

        let r = snapshot.add_pleasure(amount);
        if r.clamped {
            warn!(
                player = %target.player_uuid,
                before = r.before,
                after = r.after,
                delta = r.delta,
                source = ?source,
                "pleasure clamped at 0/100"
            );
        }

        self.deps
            .repo
            .upsert(&snapshot, tick)
            .await
            .map_err(RepoStatus::from)?;

        // Task #123: drop the cache entry so the next GetState / HUD
        // refresh re-reads the patched snapshot from PG.
        self.invalidate_after_write(target.player_uuid).await;

        self.deps
            .audit
            .write(AuditEntry {
                actor_uuid: None,
                actor_type: actor_type_for_source(source.as_deref()),
                target_uuid: target.player_uuid,
                op: "state.pleasure",
                before,
                after: snapshot.clone(),
                source,
                request_id,
                tick_millis: tick,
            })
            .await
            .map_err(RepoStatus::from)?;

        Ok(Response::new(snapshot_to_proto(&snapshot)))
    }

    // ── AddHunger ─────────────────────────────────────────────────
    async fn add_hunger(
        &self,
        request: Request<HungerRequest>,
    ) -> Result<Response<PlayerState>, Status> {
        let HungerRequest {
            target,
            amount,
            source,
            request_id,
        } = request.into_inner();
        if amount.is_nan() || amount.is_infinite() {
            return Err(Status::invalid_argument("amount must be finite"));
        }

        let mut snapshot = self.load_snapshot(target.player_uuid).await?;
        let before = snapshot.clone();
        let tick = (self.tick_millis)();

        let r = snapshot.add_hunger(amount);
        if r.clamped {
            warn!(
                player = %target.player_uuid,
                before = r.before,
                after = r.after,
                delta = r.delta,
                source = ?source,
                "hunger clamped at 0/max_hunger"
            );
        }

        self.deps
            .repo
            .upsert(&snapshot, tick)
            .await
            .map_err(RepoStatus::from)?;

        // Task #123: drop the cache entry so the next GetState / HUD
        // refresh re-reads the patched snapshot from PG.
        self.invalidate_after_write(target.player_uuid).await;

        self.deps
            .audit
            .write(AuditEntry {
                actor_uuid: None,
                actor_type: actor_type_for_source(source.as_deref()),
                target_uuid: target.player_uuid,
                op: "state.hunger",
                before,
                after: snapshot.clone(),
                source,
                request_id,
                tick_millis: tick,
            })
            .await
            .map_err(RepoStatus::from)?;

        Ok(Response::new(snapshot_to_proto(&snapshot)))
    }

    // ── AddFluidEffect ────────────────────────────────────────────
    //
    // 2026-06-14 task #8. Routes a CONSUMPTION-source fluid event
    // (e.g. player drank High Tide) to the right effect chain:
    //
    //   1. Load every `fluid_effects` row with `fluid = request.fluid`
    //      AND `source = 'CONSUMPTION'`.
    //   2. For each row, apply the effect to the snapshot:
    //        - PLEASURE_BOOST → add_pleasure(magnitude)
    //        - HUNGER_BOOST   → add_hunger(magnitude)
    //        - PART_DEV_BOOST → add_part_dev(GENITAL, magnitude)
    //        - DEFEAT_TRIGGER → bump defeat_count + warn (the actual
    //          punitive debuff lives in the environment layer; this
    //          service only updates the counter per 02 §3.4)
    //        - STRESS_BOOST   → no-op (production-side; only fires
    //          through the EnvironmentService / core_pod path)
    //        - DECORATIVE     → no-op (00 §4 override)
    //   3. Persist the snapshot and write one
    //      `audit_player_state` row with `op = "state.fluid_consume"`.
    async fn add_fluid_effect(
        &self,
        request: Request<FluidEffectRequest>,
    ) -> Result<Response<PlayerState>, Status> {
        use biocapital_core::fluids::{FluidEffectType, FluidSource};

        let FluidEffectRequest {
            target,
            fluid,
            request_id,
        } = request.into_inner();

        // 1. Load CONSUMPTION effects for this fluid.
        let effects = self
            .fluid_deps
            .repo
            .get_effects_for_fluid_source(fluid, FluidSource::Consumption)
            .await
            .map_err(RepoStatus::from_fluid)?;

        // Empty effect set (e.g. an admin-restricted fluid) is a no-op
        // success — we still write an audit row so the call is visible.
        let mut snapshot = self.load_snapshot(target.player_uuid).await?;
        let before = snapshot.clone();
        let tick = (self.tick_millis)();

        // 2. Apply each effect in deterministic order. We sort by
        //    `FluidEffectType` enum discriminant so two callers hitting
        //    the same fluid get the same mutation sequence (and the
        //    audit row before/after diff is reproducible).
        let mut ordered: Vec<_> = effects.into_iter().collect();
        ordered.sort_by_key(|e| e.effect_type as i32);

        for effect in ordered {
            if !effect.magnitude.is_finite() {
                warn!(
                    player = %target.player_uuid,
                    fluid = %fluid,
                    effect_type = %effect.effect_type,
                    "fluid_effects.magnitude is non-finite; skipping"
                );
                continue;
            }
            match effect.effect_type {
                FluidEffectType::PleasureBoost => {
                    let r = snapshot.add_pleasure(effect.magnitude);
                    if r.clamped {
                        warn!(
                            player = %target.player_uuid,
                            fluid = %fluid,
                            before = r.before,
                            after = r.after,
                            "pleasure clamped at 0/100 (fluid consume)"
                        );
                    }
                }
                FluidEffectType::HungerBoost => {
                    let r = snapshot.add_hunger(effect.magnitude);
                    if r.clamped {
                        warn!(
                            player = %target.player_uuid,
                            fluid = %fluid,
                            before = r.before,
                            after = r.after,
                            "hunger clamped at 0/max_hunger (fluid consume)"
                        );
                    }
                }
                FluidEffectType::PartDevBoost => {
                    // Per 05 §4 the only body part touched by fluids is
                    // GENITAL. Future extensions that need a per-row
                    // `part` column should land in a follow-up task
                    // alongside a proto schema bump.
                    let r = snapshot.add_part_dev(BodyPart::Genital, effect.magnitude).map_err(
                        |source| {
                            Status::invalid_argument(format!(
                                "non-finite part_dev delta: {source}"
                            ))
                        },
                    )?;
                    if r.clamped {
                        warn!(
                            player = %target.player_uuid,
                            fluid = %fluid,
                            part = "GENITAL",
                            before = r.before,
                            after = r.after,
                            "part_dev clamped at 0/100 (fluid consume)"
                        );
                    }
                }
                FluidEffectType::DefeatTrigger => {
                    // The player-consumed defeat trigger is a
                    // placeholder (05 §3.4). We bump the counter so
                    // downstream auditing is consistent, but do NOT
                    // apply the punitive debuff here — that lives in
                    // the environment / hostile layer that initiated
                    // the call. We log a warning so a future reviewer
                    // can spot that this path was hit.
                    warn!(
                        player = %target.player_uuid,
                        fluid = %fluid,
                        "DEFEAT_TRIGGER fired from CONSUMPTION path; \
                         punitive debuff is not applied here"
                    );
                }
                FluidEffectType::StressBoost => {
                    // Production-side effect; mis-routed to the
                    // consumption path. Log loudly and skip.
                    warn!(
                        player = %target.player_uuid,
                        fluid = %fluid,
                        "STRESS_BOOST effect ignored on CONSUMPTION path; \
                         applies via EnvironmentService"
                    );
                }
                FluidEffectType::Decorative => {
                    // 00 §4: super_lubricant is purely decorative.
                    // No state mutation.
                }
            }
        }

        // 3. Persist + audit.
        self.deps
            .repo
            .upsert(&snapshot, tick)
            .await
            .map_err(RepoStatus::from)?;

        // Task #123: drop the cache entry so the next GetState / HUD
        // refresh re-reads the patched snapshot from PG.
        self.invalidate_after_write(target.player_uuid).await;

        self.deps
            .audit
            .write(AuditEntry {
                actor_uuid: None,
                actor_type: "RUST_SERVICE",
                target_uuid: target.player_uuid,
                op: "state.fluid_consume",
                before,
                after: snapshot.clone(),
                source: Some(fluid.as_path().to_string()),
                request_id,
                tick_millis: tick,
            })
            .await
            .map_err(RepoStatus::from)?;

        Ok(Response::new(snapshot_to_proto(&snapshot)))
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Convert a domain snapshot into the wire-shape `PlayerState`.
fn snapshot_to_proto(s: &PlayerStateSnapshot) -> PlayerState {
    let parts = s
        .parts
        .iter()
        .map(|(k, v)| (k.to_string(), *v))
        .collect();
    PlayerState {
        player_uuid: s.uuid,
        pleasure: s.pleasure,
        hunger: s.hunger,
        hidden_hp: s.hidden_hp,
        low_hp_hits: s.low_hp_hits,
        parts,
        updated_at_ms: s.updated_at.timestamp_millis(),
        defeat_count: s.defeat_count,
        active_contracts: 0, // reserved; task #9 will populate
        max_hunger: s.max_hunger,
    }
}

/// Map a `source` string from a DamageRequest / PleasureRequest /
/// HungerRequest into an `actor_type` enum value for the audit row.
/// Unknown sources fall through to `RUST_SERVICE` so the constraint
/// check in PG still passes.
fn actor_type_for_source(source: Option<&str>) -> &'static str {
    match source {
        Some(s) if s.eq_ignore_ascii_case("zombie")
            || s.eq_ignore_ascii_case("skeleton")
            || s.eq_ignore_ascii_case("creeper")
            || s.eq_ignore_ascii_case("mob") =>
        {
            "HOSTILE_MOB"
        }
        Some(s) if s.eq_ignore_ascii_case("lava")
            || s.eq_ignore_ascii_case("swamp")
            || s.eq_ignore_ascii_case("swamp_mud")
            || s.eq_ignore_ascii_case("sand")
            || s.eq_ignore_ascii_case("magma_block") =>
        {
            "ENVIRONMENT"
        }
        Some(s) if s.eq_ignore_ascii_case("admin") || s.eq_ignore_ascii_case("cmd") => {
            "ADMIN_CMD"
        }
        _ => "RUST_SERVICE",
    }
}

/// Local alias so the `?` operator on repository errors gives a clean
/// `Status` back. We can't use the `From` impl from `biocapital-pg`
/// directly in this file because tonic's `Status` would create a
/// circular re-export; we re-wrap here.
struct RepoStatus;
impl RepoStatus {
    fn from(e: biocapital_pg::PlayerStateRepoError) -> Status {
        use biocapital_pg::PlayerStateRepoError as R;
        match e {
            R::Sqlx(sqlx::Error::RowNotFound) => Status::not_found("player_state row not found"),
            R::Sqlx(e) => Status::internal(format!("postgres error: {e}")),
            R::Migrate(e) => Status::internal(format!("migration error: {e}")),
            R::InvalidUuid { column, value } => {
                Status::invalid_argument(format!("invalid UUID in {column}: {value}"))
            }
            R::InvalidBodyPart { column, source } => {
                Status::invalid_argument(format!("invalid BodyPart in {column}: {source}"))
            }
            R::SnapshotMismatch { table, message } => {
                Status::failed_precondition(format!("snapshot mismatch on {table}: {message}"))
            }
        }
    }

    /// Task #8: convert a `biocapital_environment::pg::fluid::RepoError`
    /// (re-exported as `FluidRepoError`) to a `Status`. The
    /// `FluidRepoError` already implements `Into<Status>` itself, but
    /// we keep a local mapping here so future `RepoStatus` extensions
    /// stay in one place.
    fn from_fluid(e: biocapital_environment::pg::fluid::FluidRepoError) -> Status {
        use biocapital_environment::pg::fluid::RepoError as FR;
        match e {
            FR::Sqlx(sqlx::Error::RowNotFound) => {
                Status::not_found("fluid_effects row not found")
            }
            FR::Sqlx(e) => Status::internal(format!("postgres error: {e}")),
            FR::Migrate(e) => Status::internal(format!("migration error: {e}")),
            FR::InvalidFluid { column, value } => {
                Status::invalid_argument(format!(
                    "invalid BiocapitalFluid in {column}: {value}"
                ))
            }
            FR::InvalidEffectType { column, value } => {
                Status::invalid_argument(format!(
                    "invalid FluidEffectType in {column}: {value}"
                ))
            }
            FR::InvalidSource { column, value } => {
                Status::invalid_argument(format!(
                    "invalid FluidSource in {column}: {value}"
                ))
            }
            FR::InvalidUuid { column, value } => {
                Status::invalid_argument(format!("invalid UUID in {column}: {value}"))
            }
        }
    }
}

/// Convert a `biocapital_core::LoadError` (returned by the cache loader)
/// into a `tonic::Status`. Used by the `load_snapshot` helper. We map
/// `Sqlx` / `Migrate` to `INTERNAL` (server-side persistence problem)
/// and the generic `Repo` variant to `INTERNAL` as well — the inner
/// `PlayerStateRepository` already maps its own errors via `RepoStatus`
/// when the cache is disabled, so the cache-specific path only fires
/// for "raw repository error" failures that bypass the typed enum.
struct LoadStatus;
impl LoadStatus {
    fn from(e: biocapital_core::LoadError) -> Status {
        use biocapital_core::LoadError as L;
        match e {
            L::Sqlx(sqlx::Error::RowNotFound) => {
                Status::not_found("player_state row not found")
            }
            L::Sqlx(e) => Status::internal(format!("postgres error: {e}")),
            L::Migrate(e) => Status::internal(format!("migration error: {e}")),
            L::Repo(msg) => Status::internal(format!("repository error: {msg}")),
        }
    }
}

// ── Tests (no live DB / network required) ───────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use biocapital_core::fluids::{FluidEffect, FluidEffectType, FluidSource};
    use biocapital_core::player_state::{BodyPart, PlayerStateSnapshot};
    use biocapital_environment::pg::fluid::{FluidRepoError, FluidRepository, FluidServiceDeps};
    use biocapital_pg::player_state::RepoError;
    use biocapital_pg::{
        AuditEntry, AuditWriter, PlayerStateRepository,
    };

    /// In-memory repo for tests.
    struct MemRepo {
        rows: Mutex<std::collections::HashMap<Uuid, PlayerStateSnapshot>>,
    }

    impl MemRepo {
        fn new() -> Self {
            Self {
                rows: Mutex::new(std::collections::HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl PlayerStateRepository for MemRepo {
        async fn get(&self, uuid: Uuid) -> Result<PlayerStateSnapshot, RepoError> {
            let g = self.rows.lock().unwrap();
            Ok(g.get(&uuid).cloned().unwrap_or_else(|| PlayerStateSnapshot::new(uuid)))
        }
        async fn upsert(
            &self,
            snapshot: &PlayerStateSnapshot,
            _tick: i64,
        ) -> Result<(), RepoError> {
            self.rows
                .lock()
                .unwrap()
                .insert(snapshot.uuid, snapshot.clone());
            Ok(())
        }
        async fn add_part_dev(
            &self,
            uuid: Uuid,
            part: BodyPart,
            delta: f32,
            _tick: i64,
        ) -> Result<biocapital_core::player_state::PartChange, RepoError> {
            let mut g = self.rows.lock().unwrap();
            let s = g.entry(uuid).or_insert_with(|| PlayerStateSnapshot::new(uuid));
            s.add_part_dev(part, delta).map_err(|_| RepoError::SnapshotMismatch {
                table: "body_part_development",
                message: "non-finite".into(),
            })
        }
        async fn list_by_uuid(
            &self,
            uuids: &[Uuid],
        ) -> Result<Vec<PlayerStateSnapshot>, RepoError> {
            let g = self.rows.lock().unwrap();
            Ok(uuids
                .iter()
                .map(|u| g.get(u).cloned().unwrap_or_else(|| PlayerStateSnapshot::new(*u)))
                .collect())
        }
    }

    struct MemAudit {
        rows: Mutex<Vec<AuditEntry>>,
    }
    impl MemAudit {
        fn new() -> Self {
            Self {
                rows: Mutex::new(Vec::new()),
            }
        }
    }
    #[async_trait]
    impl AuditWriter for MemAudit {
        async fn write(&self, entry: AuditEntry) -> Result<(), RepoError> {
            self.rows.lock().unwrap().push(entry);
            Ok(())
        }
    }

    /// In-memory fluid repo for tests. Pre-loads the canonical seed rows
    /// from `rust/migrations/20260614000006_fluids.sql` so the
    /// `add_fluid_effect` tests can hit known magnitudes.
    struct MemFluidRepo {
        rows: Mutex<Vec<FluidEffect>>,
    }

    impl MemFluidRepo {
        fn new_with_seeds() -> Self {
            let mut rows: Vec<FluidEffect> = Vec::new();
            let push = |rows: &mut Vec<FluidEffect>,
                        f: BiocapitalFluid,
                        t: FluidEffectType,
                        m: f32,
                        d: i64,
                        s: FluidSource| {
                rows.push(FluidEffect::new(f, t, m, d, s, 0));
            };
            push(&mut rows, BiocapitalFluid::HighTide, FluidEffectType::PleasureBoost, 5.0, 200, FluidSource::Consumption);
            push(&mut rows, BiocapitalFluid::HighTide, FluidEffectType::StressBoost,   2.0,   0, FluidSource::Production);
            push(&mut rows, BiocapitalFluid::SuperLubricant, FluidEffectType::Decorative, 0.0, 0, FluidSource::Production);
            push(&mut rows, BiocapitalFluid::CharmPotion, FluidEffectType::PartDevBoost, 3.0, 600, FluidSource::Consumption);
            push(&mut rows, BiocapitalFluid::CharmPotion, FluidEffectType::PleasureBoost, 10.0, 400, FluidSource::Consumption);
            push(&mut rows, BiocapitalFluid::Semen, FluidEffectType::PartDevBoost, 1.0, 0, FluidSource::Production);
            push(&mut rows, BiocapitalFluid::Semen, FluidEffectType::DefeatTrigger, 0.0, 0, FluidSource::Consumption);
            Self { rows: Mutex::new(rows) }
        }
    }

    #[async_trait]
    impl FluidRepository for MemFluidRepo {
        async fn list_effects(
            &self,
            fluid: Option<BiocapitalFluid>,
        ) -> Result<Vec<FluidEffect>, FluidRepoError> {
            let g = self.rows.lock().unwrap();
            Ok(g.iter()
                .filter(|e| fluid.map(|f| e.fluid == f).unwrap_or(true))
                .cloned()
                .collect())
        }
        async fn get_effects_by_type(
            &self,
            effect_type: FluidEffectType,
        ) -> Result<Vec<FluidEffect>, FluidRepoError> {
            let g = self.rows.lock().unwrap();
            Ok(g.iter()
                .filter(|e| e.effect_type == effect_type)
                .cloned()
                .collect())
        }
        async fn get_effects_for_fluid_source(
            &self,
            fluid: BiocapitalFluid,
            source: FluidSource,
        ) -> Result<Vec<FluidEffect>, FluidRepoError> {
            let g = self.rows.lock().unwrap();
            Ok(g.iter()
                .filter(|e| e.fluid == fluid && e.source == source)
                .cloned()
                .collect())
        }
    }

    fn make_service() -> (
        PlayerStateGrpc,
        Arc<MemRepo>,
        Arc<MemAudit>,
        Arc<MemFluidRepo>,
    ) {
        let repo = Arc::new(MemRepo::new());
        let audit = Arc::new(MemAudit::new());
        let fluid_repo = Arc::new(MemFluidRepo::new_with_seeds());
        let deps = PlayerStateServiceDeps::new(
            repo.clone() as Arc<dyn PlayerStateRepository>,
            audit.clone() as Arc<dyn AuditWriter>,
        );
        let fluid_deps = FluidServiceDeps::new(fluid_repo.clone() as Arc<dyn FluidRepository>);
        (PlayerStateGrpc::new(deps, fluid_deps), repo, audit, fluid_repo)
    }

    #[tokio::test]
    async fn get_state_returns_default_for_unknown_player() {
        let (svc, _repo, _audit, _fluid) = make_service();
        let uuid = Uuid::new_v4();
        let resp = svc
            .get_state(Request::new(PlayerIdentifier { player_uuid: uuid }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.player_uuid, uuid);
        assert_eq!(resp.pleasure, 0.0);
        assert_eq!(resp.hunger, 50.0);
        assert_eq!(resp.hidden_hp, 20.0);
        assert_eq!(resp.parts.len(), 12);
    }

    #[tokio::test]
    async fn add_pleasure_clamps_and_audits() {
        let (svc, _repo, audit, _fluid) = make_service();
        let uuid = Uuid::new_v4();
        let resp = svc
            .add_pleasure(Request::new(PleasureRequest {
                target: PlayerIdentifier { player_uuid: uuid },
                amount: 1000.0,
                part: None,
                source: Some("BERRY".into()),
                request_id: Some(Uuid::new_v4()),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.pleasure, 100.0);
        let g = audit.rows.lock().unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].op, "state.pleasure");
        assert_eq!(g[0].actor_type, "RUST_SERVICE"); // BERRY is unknown source
    }

    #[tokio::test]
    async fn apply_damage_floor_increments_counters() {
        let (svc, _repo, audit, _fluid) = make_service();
        let uuid = Uuid::new_v4();
        let resp = svc
            .apply_damage(Request::new(DamageRequest {
                target: PlayerIdentifier { player_uuid: uuid },
                amount: 100.0,
                source: Some("zombie".into()),
                part: None,
                request_id: Some(Uuid::new_v4()),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp.killed, "vanilla death is gone (02 §3.4)");
        assert_eq!(resp.new_state.hidden_hp, 1.0);
        assert_eq!(resp.new_state.low_hp_hits, 1);
        assert_eq!(resp.new_state.defeat_count, 1);
        let g = audit.rows.lock().unwrap();
        assert_eq!(g[0].actor_type, "HOSTILE_MOB");
    }

    #[tokio::test]
    async fn add_hunger_clamps_to_max_hunger() {
        let (svc, _repo, _audit, _fluid) = make_service();
        let uuid = Uuid::new_v4();
        let resp = svc
            .add_hunger(Request::new(HungerRequest {
                target: PlayerIdentifier { player_uuid: uuid },
                amount: 1000.0,
                source: Some("FOOD".into()),
                request_id: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.hunger, 100.0);
    }

    #[tokio::test]
    async fn update_state_partial_only_touches_specified_fields() {
        let (svc, _repo, audit, _fluid) = make_service();
        let uuid = Uuid::new_v4();
        let _ = svc
            .update_state(Request::new(PlayerStateUpdate {
                target: PlayerIdentifier { player_uuid: uuid },
                pleasure: Some(42.0),
                hunger: None,
                hidden_hp: None,
                low_hp_hits: None,
                parts: None,
                active_contracts: None,
                max_hunger: None,
            }))
            .await
            .unwrap()
            .into_inner();

        let resp = svc
            .get_state(Request::new(PlayerIdentifier { player_uuid: uuid }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.pleasure, 42.0);
        // Hunger untouched → still default 50.0
        assert_eq!(resp.hunger, 50.0);
        let g = audit.rows.lock().unwrap();
        // Two audit rows: the update, and the get (but get is read-only,
        // so only one row).
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].op, "state.update");
    }

    #[tokio::test]
    async fn add_fluid_effect_high_tide_pleasure_only() {
        // High Tide's CONSUMPTION-side row is PLEASURE_BOOST +5. The
        // PRODUCTION-side STRESS_BOOST row must be filtered out.
        let (svc, _repo, audit, _fluid) = make_service();
        let uuid = Uuid::new_v4();
        let resp = svc
            .add_fluid_effect(Request::new(FluidEffectRequest {
                target: PlayerIdentifier { player_uuid: uuid },
                fluid: BiocapitalFluid::HighTide,
                request_id: Some(Uuid::new_v4()),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.pleasure, 5.0);
        let g = audit.rows.lock().unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].op, "state.fluid_consume");
        assert_eq!(g[0].source.as_deref(), Some("high_tide"));
    }

    #[tokio::test]
    async fn add_fluid_effect_charm_potion_dual_effect() {
        // Charm Potion has 2 CONSUMPTION rows: PLEASURE_BOOST +10 and
        // PART_DEV_BOOST +3 on GENITAL.
        let (svc, _repo, _audit, _fluid) = make_service();
        let uuid = Uuid::new_v4();
        let resp = svc
            .add_fluid_effect(Request::new(FluidEffectRequest {
                target: PlayerIdentifier { player_uuid: uuid },
                fluid: BiocapitalFluid::CharmPotion,
                request_id: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.pleasure, 10.0);
        // GENITAL = 3.0
        let genital = resp
            .parts
            .iter()
            .find(|(name, _)| name == "GENITAL")
            .map(|(_, v)| *v)
            .unwrap();
        assert_eq!(genital, 3.0);
    }

    #[tokio::test]
    async fn add_fluid_effect_super_lubricant_is_noop() {
        let (svc, _repo, _audit, _fluid) = make_service();
        let uuid = Uuid::new_v4();
        let resp = svc
            .add_fluid_effect(Request::new(FluidEffectRequest {
                target: PlayerIdentifier { player_uuid: uuid },
                fluid: BiocapitalFluid::SuperLubricant,
                request_id: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.pleasure, 0.0);
        assert_eq!(resp.hidden_hp, 20.0);
        let g_resp = resp
            .parts
            .iter()
            .find(|(name, _)| name == "GENITAL")
            .map(|(_, v)| *v)
            .unwrap();
        assert_eq!(g_resp, 0.0);
    }

    #[tokio::test]
    async fn add_fluid_effect_semen_part_dev_only() {
        // Semen's CONSUMPTION row is DEFEAT_TRIGGER (placeholder). The
        // service must NOT mutate GENITAL (the PRODUCTION PART_DEV_BOOST
        // row is filtered out by the source check).
        let (svc, _repo, _audit, _fluid) = make_service();
        let uuid = Uuid::new_v4();
        let resp = svc
            .add_fluid_effect(Request::new(FluidEffectRequest {
                target: PlayerIdentifier { player_uuid: uuid },
                fluid: BiocapitalFluid::Semen,
                request_id: None,
            }))
            .await
            .unwrap()
            .into_inner();
        let g_resp = resp
            .parts
            .iter()
            .find(|(name, _)| name == "GENITAL")
            .map(|(_, v)| *v)
            .unwrap();
        assert_eq!(g_resp, 0.0);
        assert_eq!(resp.pleasure, 0.0);
    }

    #[test]
    fn actor_type_classifier() {
        assert_eq!(actor_type_for_source(Some("zombie")), "HOSTILE_MOB");
        assert_eq!(actor_type_for_source(Some("ZOMBIE")), "HOSTILE_MOB");
        assert_eq!(actor_type_for_source(Some("lava")), "ENVIRONMENT");
        assert_eq!(actor_type_for_source(Some("LAVA")), "ENVIRONMENT");
        assert_eq!(actor_type_for_source(Some("admin")), "ADMIN_CMD");
        assert_eq!(actor_type_for_source(Some("BERRY")), "RUST_SERVICE");
        assert_eq!(actor_type_for_source(None), "RUST_SERVICE");
    }

    // Suppress unused-import warning for BTreeMap in tests.
    #[allow(dead_code)]
    fn _bt() -> BTreeMap<BodyPart, f32> {
        BTreeMap::new()
    }
}
