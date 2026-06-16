//! PostgreSQL persistence for the fluid-effect module
//! (`doc/05-byproducts-fluids.md` §1 + §4).
//!
//! Companion to the domain types in
//! `biocapital_core::fluids`. This module is the only
//! piece of code that talks to the `fluid_effects`
//! table.
//!
//! Public surface:
//! - [`FluidRepository`] — trait the gRPC layer depends on
//!   (in `biocapital-grpc::player_state_service` for the
//!   CONSUMPTION path and `biocapital-environment` for
//!   the ENVIRONMENT path); tests can substitute an
//!   in-memory implementation.
//! - [`PgFluidRepository`] — production implementation
//!   backed by `sqlx::PgPool`.
//! - [`FluidRepoError`] — typed error surface; mirrors
//!   the `biocapital-pg::player_state::RepoError`
//!   pattern.
//!
//! Idempotency: the `fluid_effects` table is read-only at
//! runtime. The seed INSERTs in
//! `rust/migrations/20260614000006_fluids.sql` are the
//! authoritative default payload; player-authored
//! overrides land in a future `create_biocapital.toml`
//! `[Fluids]` section (task #11 / #14, 99 §6).

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use thiserror::Error;

use biocapital_core::fluids::{
    BiocapitalFluid, FluidEffect, FluidEffectType, FluidSource,
};

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("postgres error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("invalid BiocapitalFluid in column {column}: {value}")]
    InvalidFluid { column: &'static str, value: String },

    #[error("invalid FluidEffectType in column {column}: {value}")]
    InvalidEffectType { column: &'static str, value: String },

    #[error("invalid FluidSource in column {column}: {value}")]
    InvalidSource { column: &'static str, value: String },

    #[error("invalid UUID in column {column}: {value}")]
    InvalidUuid { column: &'static str, value: String },
}

/// Re-export alias so callers can disambiguate from
/// `bank::RepoError` / `player_state::RepoError` /
/// `core_pod::RepoError` / `dglab::RepoError` /
/// `contract::RepoError`. The enums are kept distinct so
/// the gRPC layer can `From`-convert each to the right
/// tonic status without an extra match arm.
pub type FluidRepoError = RepoError;

impl From<RepoError> for tonic::Status {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::Sqlx(sqlx::Error::RowNotFound) => {
                tonic::Status::not_found("fluid_effects row not found")
            }
            RepoError::Sqlx(e) => {
                tonic::Status::internal(format!("postgres error: {e}"))
            }
            RepoError::Migrate(e) => {
                tonic::Status::internal(format!("migration error: {e}"))
            }
            RepoError::InvalidFluid { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid BiocapitalFluid in {column}: {value}"
                ))
            }
            RepoError::InvalidEffectType { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid FluidEffectType in {column}: {value}"
                ))
            }
            RepoError::InvalidSource { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid FluidSource in {column}: {value}"
                ))
            }
            RepoError::InvalidUuid { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid UUID in {column}: {value}"
                ))
            }
        }
    }
}

// ── Repository trait ────────────────────────────────────────────────────────

/// Persistence contract for `fluid_effects`. The table is
/// effectively read-only at runtime; the trait exposes the
/// two read paths the gRPC layer needs and a small
/// housekeeping helper for the seed reload path
/// (task #11/14).
#[async_trait]
pub trait FluidRepository: Send + Sync {
    /// List all effects, optionally filtered by fluid. When
    /// `fluid` is `None` this returns "every row in the table"
    /// — only safe for admin paths or the seed reload.
    async fn list_effects(
        &self,
        fluid: Option<BiocapitalFluid>,
    ) -> Result<Vec<FluidEffect>, RepoError>;

    /// Filter by effect_type across all fluids. Used by the
    /// ENVIRONMENT routing path when `EnvironmentService`
    /// needs every row with `source = ENVIRONMENT`.
    async fn get_effects_by_type(
        &self,
        effect_type: FluidEffectType,
    ) -> Result<Vec<FluidEffect>, RepoError>;

    /// Filter by both fluid and source. The CONSUMPTION path
    /// (`PlayerStateService::add_fluid_effect`) calls this with
    /// `source = CONSUMPTION` so it sees only the player-driven
    /// effects for that fluid.
    async fn get_effects_for_fluid_source(
        &self,
        fluid: BiocapitalFluid,
        source: FluidSource,
    ) -> Result<Vec<FluidEffect>, RepoError>;
}

// ── Postgres implementation ────────────────────────────────────────────────

#[derive(Clone)]
pub struct PgFluidRepository {
    pool: PgPool,
}

impl PgFluidRepository {
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

fn row_to_effect(row: &sqlx::postgres::PgRow) -> Result<FluidEffect, RepoError> {
    let effect_id: uuid::Uuid = row.try_get("effect_id").map_err(|e| {
        RepoError::InvalidUuid {
            column: "effect_id",
            value: e.to_string(),
        }
    })?;
    let fluid_wire: String = row.try_get("fluid").map_err(|e| {
        RepoError::InvalidFluid {
            column: "fluid",
            value: e.to_string(),
        }
    })?;
    let fluid: BiocapitalFluid = fluid_wire.parse().map_err(|_| {
        RepoError::InvalidFluid {
            column: "fluid",
            value: fluid_wire.clone(),
        }
    })?;
    let effect_type_wire: String = row.try_get("effect_type").map_err(|e| {
        RepoError::InvalidEffectType {
            column: "effect_type",
            value: e.to_string(),
        }
    })?;
    let effect_type: FluidEffectType = effect_type_wire.parse().map_err(|_| {
        RepoError::InvalidEffectType {
            column: "effect_type",
            value: effect_type_wire.clone(),
        }
    })?;
    let magnitude: f32 = row.try_get("magnitude").unwrap_or(0.0);
    let duration_ticks: i64 = row.try_get("duration_ticks").unwrap_or(0);
    let source_wire: String = row.try_get("source").map_err(|e| {
        RepoError::InvalidSource {
            column: "source",
            value: e.to_string(),
        }
    })?;
    let source: FluidSource = source_wire.parse().map_err(|_| {
        RepoError::InvalidSource {
            column: "source",
            value: source_wire.clone(),
        }
    })?;
    let created_tick: i64 = row.try_get("created_tick").unwrap_or(0);

    Ok(FluidEffect {
        effect_id,
        fluid,
        effect_type,
        magnitude,
        duration_ticks,
        source,
        created_tick,
    })
}

#[async_trait]
impl FluidRepository for PgFluidRepository {
    async fn list_effects(
        &self,
        fluid: Option<BiocapitalFluid>,
    ) -> Result<Vec<FluidEffect>, RepoError> {
        let rows = match fluid {
            Some(f) => {
                sqlx::query(
                    r#"
                    SELECT effect_id, fluid, effect_type, magnitude,
                           duration_ticks, source, created_tick
                      FROM fluid_effects
                     WHERE fluid = $1
                     ORDER BY effect_type, source
                    "#,
                )
                .bind(f.as_path())
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    r#"
                    SELECT effect_id, fluid, effect_type, magnitude,
                           duration_ticks, source, created_tick
                      FROM fluid_effects
                     ORDER BY fluid, effect_type, source
                    "#,
                )
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.iter().map(row_to_effect).collect()
    }

    async fn get_effects_by_type(
        &self,
        effect_type: FluidEffectType,
    ) -> Result<Vec<FluidEffect>, RepoError> {
        let rows = sqlx::query(
            r#"
            SELECT effect_id, fluid, effect_type, magnitude,
                   duration_ticks, source, created_tick
              FROM fluid_effects
             WHERE effect_type = $1
             ORDER BY fluid, source
            "#,
        )
        .bind(effect_type.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_effect).collect()
    }

    async fn get_effects_for_fluid_source(
        &self,
        fluid: BiocapitalFluid,
        source: FluidSource,
    ) -> Result<Vec<FluidEffect>, RepoError> {
        let rows = sqlx::query(
            r#"
            SELECT effect_id, fluid, effect_type, magnitude,
                   duration_ticks, source, created_tick
              FROM fluid_effects
             WHERE fluid = $1 AND source = $2
             ORDER BY effect_type
            "#,
        )
        .bind(fluid.as_path())
        .bind(source.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_effect).collect()
    }
}

// ── Composite service deps ─────────────────────────────────────────────────

/// Composite handle passed into the gRPC services that need
/// fluid-effect reads. Currently consumed by
/// `PlayerStateGrpc::add_fluid_effect` (CONSUMPTION path).
/// The ENVIRONMENT path lives in `biocapital-environment`
/// and has its own composite; this struct is the one shared
/// shape across the workspace.
#[derive(Clone)]
pub struct FluidServiceDeps {
    pub repo: std::sync::Arc<dyn FluidRepository>,
}

impl FluidServiceDeps {
    pub fn new(repo: std::sync::Arc<dyn FluidRepository>) -> Self {
        Self { repo }
    }
}

// ── Sanity test (no live DB) ────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trait_is_object_safe() {
        fn _assert_object_safe(_: std::sync::Arc<dyn FluidRepository>) {}
    }
}