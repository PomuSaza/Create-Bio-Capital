//! PostgreSQL persistence for the environment-effect module
//! (`doc/07-environment.md` §8 + `doc/99-integration-matrix.md` §5.1.2 +
//! §5.1).
//!
//! Companion to the domain type in
//! `biocapital_environment::domain::environment::EnvironmentEffectRule`.
//! This module is the only piece of code that talks to the
//! `environment_default_rules` + `audit_environment` tables.
//!
//! Public surface:
//! - [`EnvironmentRepository`] — trait the gRPC service
//!   (`biocapital-grpc::environment_service`) depends on; tests can
//!   substitute an in-memory implementation.
//! - [`PgEnvironmentRepository`] — production implementation
//!   backed by `sqlx::PgPool`.
//! - [`EnvironmentRepoError`] — typed error surface; mirrors the
//!   `biocapital-pg::player_state::RepoError` pattern. The
//!   `tonic::Status` `From` impl is implemented so the gRPC
//!   layer can `?`-bubble errors without an extra match arm.
//! - [`EnvironmentServiceDeps`] — composite handle passed into
//!   `EnvironmentService` (mirrors `BankServiceDeps` /
//!   `FluidServiceDeps`).
//! - [`EnvironmentEffectLog`] — write payload for
//!   `audit_environment` (mirrors `audit_player_state` 99 §2.2
//!   relaxed-form contract).
//!
//! Idempotency: `audit_environment` is append-only; the gRPC layer
//! supplies `request_id` (proto `EnvironmentEffectRequest.request_id`)
//! for replay-dedupe. The partial unique index on
//! `environment_default_rules(environment) WHERE enabled = TRUE`
//! guarantees the read path returns at most one row per canonical
//! environment in the steady state.

pub mod fluid;
pub use fluid::{
    FluidRepository, FluidRepoError, FluidServiceDeps, PgFluidRepository, RepoError as FluidRepoErrorBase,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

use crate::domain::environment::{
    EnvironmentEffectRule, EnvironmentModifier, EnvironmentSource, EnvironmentType,
    IntensityFormula,
};

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("postgres error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("invalid UUID in column {column}: {value}")]
    InvalidUuid { column: &'static str, value: String },

    #[error("invalid environment type in column {column}: {value}")]
    InvalidEnvironmentType { column: &'static str, value: String },

    #[error("invalid environment modifier in column {column}: {value}")]
    InvalidEnvironmentModifier { column: &'static str, value: String },

    #[error("invalid intensity formula in column {column}: {value}")]
    InvalidIntensityFormula { column: &'static str, value: String },

    #[error("invalid environment source in column {column}: {value}")]
    InvalidEnvironmentSource { column: &'static str, value: String },
}

/// Re-export alias so callers can disambiguate from
/// `bank::RepoError` / `player_state::RepoError` /
/// `core_pod::RepoError` / `dglab::RepoError` /
/// `contract::RepoError` / `fluid::RepoError` /
/// `mob_replacement::RepoError`. The enums are kept distinct so
/// the gRPC layer can `From`-convert each to the right
/// tonic status without an extra match arm.
pub type EnvironmentRepoError = RepoError;

impl From<RepoError> for tonic::Status {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::Sqlx(sqlx::Error::RowNotFound) => {
                tonic::Status::not_found("environment_default_rules row not found")
            }
            RepoError::Sqlx(e) => {
                tonic::Status::internal(format!("postgres error: {e}"))
            }
            RepoError::Migrate(e) => {
                tonic::Status::internal(format!("migration error: {e}"))
            }
            RepoError::InvalidUuid { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid UUID in {column}: {value}"
                ))
            }
            RepoError::InvalidEnvironmentType { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid environment type in {column}: {value}"
                ))
            }
            RepoError::InvalidEnvironmentModifier { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid environment modifier in {column}: {value}"
                ))
            }
            RepoError::InvalidIntensityFormula { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid intensity formula in {column}: {value}"
                ))
            }
            RepoError::InvalidEnvironmentSource { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid environment source in {column}: {value}"
                ))
            }
        }
    }
}

// ── EnvironmentEffectLog (audit row payload) ────────────────────────────────

/// Append-only payload for `audit_environment` (07 §6 + 99 §2.2
/// relaxed-form contract). Mirrors the proto
/// `EnvironmentEffectRequest` envelope + a few extra columns the
/// server-side dispatcher needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentEffectLog {
    /// UUIDv7 primary key. The gRPC layer mints a fresh value
    /// per call (the table has no natural unique key).
    pub log_id: Uuid,

    /// Operation initiator. `None` for system-initiated writes
    /// (the Java-tick-dispatched calls). 99 §2.2 says this is
    /// nullable.
    pub actor_uuid: Option<Uuid>,

    /// `PLAYER` / `ADMIN_CMD` / `RUST_SERVICE` /
    /// `ENVIRONMENT` (the in-DB CHECK constraint; 99 §2.2
    /// "audit").
    pub actor_type: String,

    /// The affected player. Nullable only for admin / system
    /// pings.
    pub target_player_uuid: Option<Uuid>,

    /// `LAVA` / `SWAMP_MUD` / `SAND` / `MAGMA_BLOCK` /
    /// `FLUID_<X>`. Mirrors `EnvironmentType::as_str()`.
    pub environment: String,

    /// `world_uuid` of the player's current world. The Java
    /// side fills this in for the `LivingTickEvent`-driven
    /// path; nullable for offline / unit-test paths.
    pub world_uuid: Option<Uuid>,

    /// Dimension (`"minecraft:overworld"` etc). Nullable.
    pub dimension: Option<String>,

    /// Position triple. Nullable.
    pub pos_x: Option<i64>,
    pub pos_y: Option<i64>,
    pub pos_z: Option<i64>,

    /// The `EnvironmentEffectRequest.intensity` value
    /// (07 §8 formula multiplier; `1.0` = default).
    pub intensity: Option<f32>,

    /// The `EnvironmentEffectRequest.duration_ticks` value
    /// (`0` = instant).
    pub duration_ticks: Option<i64>,

    /// Actual pleasure delta applied (post-clamp,
    /// post-intensity-scale).
    pub pleasure_delta: Option<f32>,

    /// Actual hunger delta applied (post-clamp,
    /// post-intensity-scale).
    pub hunger_delta: Option<f32>,

    /// True if this effect triggered a defeat-state entry
    /// (07 §6).
    pub triggered_defeat: bool,

    /// True if the rule replaced a fatal-damage tick
    /// (07 §2.1 LAVA + §3.2 SWAMP_MUD).
    pub no_fatal_damage: bool,

    /// Server tick counter at write time. The Sable tick loop
    /// exposes this directly (NOT a millisecond timestamp).
    pub tick_millis: i64,

    /// Idempotency dedupe (proto
    /// `EnvironmentEffectRequest.request_id`).
    pub request_id: Option<Uuid>,

    /// Free-form JSON notes. Typically carries the
    /// `EnvironmentEffectResponse.triggered_defeat` flag plus
    /// any `EnvironmentModifier` that fired (the column is
    /// `JSONB`; we default to `None`).
    pub notes: Option<serde_json::Value>,
}

// ── Repository trait ────────────────────────────────────────────────────────

/// Persistence contract for `environment_default_rules` +
/// `audit_environment`. The trait exposes the 3 hot paths the
/// gRPC `EnvironmentService` needs.
#[async_trait]
pub trait EnvironmentRepository: Send + Sync {
    /// All enabled rows, sorted by `priority DESC, rule_id DESC`.
    /// The `GetEnvironmentModifiers` RPC iterates this list to
    /// shape the per-block `EnvironmentModifiers` response.
    async fn list_default_rules(&self) -> Result<Vec<EnvironmentEffectRule>, RepoError>;

    /// The single enabled row for `environment`, sorted by
    /// `priority DESC, rule_id DESC` and limited to the first
    /// row. The `ApplyEnvironmentEffect` RPC hot path goes
    /// through this method for the 4 canonical environments
    /// (the FLUID_<X> tokens are dispatched to the fluid
    /// path instead).
    async fn get_rule(
        &self,
        environment: &EnvironmentType,
    ) -> Result<Option<EnvironmentEffectRule>, RepoError>;

    /// Append-only insert into `audit_environment`. The gRPC
    /// layer calls this once per `ApplyEnvironmentEffect`
    /// request. The 3 indexes on the table
    /// (`target_player_uuid` / `environment` / `request_id`)
    /// power the per-player history view + the per-environment
    /// dashboard + the request-id idempotency lookup.
    async fn record_effect(&self, effect: &EnvironmentEffectLog) -> Result<(), RepoError>;
}

// ── Postgres implementation ────────────────────────────────────────────────

#[derive(Clone)]
pub struct PgEnvironmentRepository {
    pool: PgPool,
}

impl PgEnvironmentRepository {
    pub async fn connect(database_url: &str) -> Result<Self, RepoError> {
        let pool = PgPoolOptions::new()
            .max_connections(16)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

// ── Row-decoder helpers ─────────────────────────────────────────────────────

fn row_to_rule(row: &sqlx::postgres::PgRow) -> Result<EnvironmentEffectRule, RepoError> {
    let rule_id: Uuid = row.try_get("rule_id").map_err(|e| {
        RepoError::InvalidUuid {
            column: "rule_id",
            value: e.to_string(),
        }
    })?;

    let env_str: String = row.try_get("environment").map_err(|e| {
        RepoError::InvalidEnvironmentType {
            column: "environment",
            value: e.to_string(),
        }
    })?;
    // The repository only loads the 4 canonical envs. FLUID_<X>
    // tokens are not stored in this table — they are routed via
    // the `fluid_effects` table.
    let environment: EnvironmentType = match env_str.as_str() {
        "LAVA" => EnvironmentType::Lava,
        "SWAMP_MUD" => EnvironmentType::SwampMud,
        "SAND" => EnvironmentType::Sand,
        "MAGMA_BLOCK" => EnvironmentType::MagmaBlock,
        other => {
            return Err(RepoError::InvalidEnvironmentType {
                column: "environment",
                value: other.to_string(),
            })
        }
    };

    let modifier_str: String = row.try_get("primary_modifier").map_err(|e| {
        RepoError::InvalidEnvironmentModifier {
            column: "primary_modifier",
            value: e.to_string(),
        }
    })?;
    let magnitude: f32 = row
        .try_get("magnitude")
        .map_err(|e| RepoError::InvalidEnvironmentModifier {
            column: "magnitude",
            value: e.to_string(),
        })?;
    let duration_ticks: i64 = row
        .try_get("duration_ticks")
        .map_err(|e| RepoError::InvalidEnvironmentModifier {
            column: "duration_ticks",
            value: e.to_string(),
        })?;
    let primary_modifier = match modifier_str.as_str() {
        "PLEASURE_DELTA" => EnvironmentModifier::PleasureDelta(magnitude),
        "HUNGER_DELTA" => EnvironmentModifier::HungerDelta(magnitude),
        "MOVEMENT_MODIFIER" => EnvironmentModifier::MovementModifier(magnitude),
        "NO_FATAL_DAMAGE" => EnvironmentModifier::NoFatalDamage,
        "TRIGGER_DEFEAT" => EnvironmentModifier::TriggerDefeat,
        "VISUAL_ONLY" => EnvironmentModifier::VisualOnly,
        other => {
            return Err(RepoError::InvalidEnvironmentModifier {
                column: "primary_modifier",
                value: other.to_string(),
            })
        }
    };

    let formula_str: String = row.try_get("intensity_formula").map_err(|e| {
        RepoError::InvalidIntensityFormula {
            column: "intensity_formula",
            value: e.to_string(),
        }
    })?;
    let intensity_formula = match formula_str.as_str() {
        "FIXED" => IntensityFormula::Fixed(magnitude),
        "LINEAR_DISTANCE" => {
            // The 4 seeded rules all use FIXED_DURATION; the
            // LINEAR_DISTANCE form carries its own `base` /
            // `decay_per_block` / `max_blocks` triple which is
            // *not* in the current schema. We surface the
            // migration-time error here so a future widening
            // migration is forced to update this decoder.
            return Err(RepoError::InvalidIntensityFormula {
                column: "intensity_formula",
                value: "LINEAR_DISTANCE rows are not supported by the current schema; widen the migration".to_string(),
            });
        }
        "FIXED_DURATION" => IntensityFormula::FixedDuration {
            base: magnitude,
            duration_ticks,
        },
        other => {
            return Err(RepoError::InvalidIntensityFormula {
                column: "intensity_formula",
                value: other.to_string(),
            })
        }
    };

    let source_str: String = row.try_get("source").map_err(|e| {
        RepoError::InvalidEnvironmentSource {
            column: "source",
            value: e.to_string(),
        }
    })?;
    let source: EnvironmentSource = match source_str.as_str() {
        "BLOCK_CONTACT" => EnvironmentSource::BlockContact,
        "FLUID_IMMERSION" => EnvironmentSource::FluidImmersion,
        "AIR_EXPOSURE" => EnvironmentSource::AirExposure,
        other => {
            return Err(RepoError::InvalidEnvironmentSource {
                column: "source",
                value: other.to_string(),
            })
        }
    };

    let enabled: bool = row.try_get("enabled").unwrap_or(true);
    let created_tick: i64 = row.try_get("created_tick").unwrap_or(0);
    let priority: i32 = row.try_get("priority").unwrap_or(0);

    Ok(EnvironmentEffectRule::with_id(
        rule_id,
        environment,
        primary_modifier,
        intensity_formula,
        source,
        enabled,
        created_tick,
        priority,
    ))
}

#[async_trait]
impl EnvironmentRepository for PgEnvironmentRepository {
    async fn list_default_rules(&self) -> Result<Vec<EnvironmentEffectRule>, RepoError> {
        let rows = sqlx::query(
            r#"
            SELECT rule_id, environment, primary_modifier, magnitude,
                   duration_ticks, intensity_formula, source, enabled,
                   created_tick, priority
              FROM environment_default_rules
             WHERE enabled = TRUE
             ORDER BY priority DESC, rule_id DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_rule).collect()
    }

    async fn get_rule(
        &self,
        environment: &EnvironmentType,
    ) -> Result<Option<EnvironmentEffectRule>, RepoError> {
        let env_str = environment.as_str();
        let rows = sqlx::query(
            r#"
            SELECT rule_id, environment, primary_modifier, magnitude,
                   duration_ticks, intensity_formula, source, enabled,
                   created_tick, priority
              FROM environment_default_rules
             WHERE environment = $1
               AND enabled = TRUE
             ORDER BY priority DESC, rule_id DESC
             LIMIT 1
            "#,
        )
        .bind(&env_str)
        .fetch_all(&self.pool)
        .await?;
        match rows.first() {
            Some(row) => Ok(Some(row_to_rule(row)?)),
            None => Ok(None),
        }
    }

    async fn record_effect(&self, effect: &EnvironmentEffectLog) -> Result<(), RepoError> {
        sqlx::query(
            r#"
            INSERT INTO audit_environment (
                log_id, actor_uuid, actor_type, target_player_uuid,
                environment, world_uuid, dimension, pos_x, pos_y, pos_z,
                intensity, duration_ticks,
                pleasure_delta, hunger_delta,
                triggered_defeat, no_fatal_damage,
                tick_millis, request_id, notes
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                    $11, $12, $13, $14, $15, $16, $17, $18, $19)
            "#,
        )
        .bind(effect.log_id)
        .bind(effect.actor_uuid)
        .bind(&effect.actor_type)
        .bind(effect.target_player_uuid)
        .bind(&effect.environment)
        .bind(effect.world_uuid)
        .bind(&effect.dimension)
        .bind(effect.pos_x)
        .bind(effect.pos_y)
        .bind(effect.pos_z)
        .bind(effect.intensity)
        .bind(effect.duration_ticks)
        .bind(effect.pleasure_delta)
        .bind(effect.hunger_delta)
        .bind(effect.triggered_defeat)
        .bind(effect.no_fatal_damage)
        .bind(effect.tick_millis)
        .bind(effect.request_id)
        .bind(&effect.notes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// ── Composite service deps ─────────────────────────────────────────────────

/// Composite handle passed into `EnvironmentService` (task #82).
/// Mirrors the `BankServiceDeps` / `FluidServiceDeps` /
/// `MobReplacementServiceDeps` shape.
#[derive(Clone)]
pub struct EnvironmentServiceDeps {
    pub repo: std::sync::Arc<dyn EnvironmentRepository>,
}

impl EnvironmentServiceDeps {
    pub fn new(repo: std::sync::Arc<dyn EnvironmentRepository>) -> Self {
        Self { repo }
    }
}

// ── Sanity test (no live DB) ────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trait_is_object_safe() {
        fn _assert_object_safe(_: std::sync::Arc<dyn EnvironmentRepository>) {}
    }
}
