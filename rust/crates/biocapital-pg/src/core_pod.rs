//! PostgreSQL persistence for the Core Pod module (04 §6.3).
//!
//! Companion to the domain types in `biocapital_pod::domain`. This
//! module is the only piece of code that talks to `core_pods` and
//! `audit_core_pod`.
//!
//! Public surface:
//! - [`CorePodRepository`] — trait the gRPC service depends on; tests
//!   can substitute an in-memory implementation.
//! - [`PgCorePodRepository`] — production implementation backed by
//!   `sqlx::PgPool`.
//! - [`CorePodAuditWriter`] — append-only writer for `audit_core_pod`,
//!   wired into the gRPC service via [`CorePodServiceDeps`].
//! - [`PodRepoError`] — typed error surface returned to callers
//!   (also implements `Into<tonic::Status>` for clean `?` usage in
//!   the gRPC layer).
//!
//! Idempotency: the gRPC service routes every mutation through
//! `upsert_pod` keyed on the 5-tuple primary key. We do **not**
//! enforce request_id-based dedupe at the repo layer (unlike bank);
//! the cooldown machinery on `core_pods` already prevents double-tick
//! races in the production cycle.

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

use biocapital_pod::domain::{CorePod, FluidStack};

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("postgres error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("core pod not found at world {world_uuid} / {dimension} / ({pos_x},{pos_y},{pos_z})")]
    PodNotFound {
        world_uuid: Uuid,
        dimension: String,
        pos_x: i64,
        pos_y: i64,
        pos_z: i64,
    },
}

/// Re-export alias for callers that want to disambiguate from
/// `bank::RepoError` / `player_state::RepoError`.
pub type PodRepoError = RepoError;

impl From<RepoError> for tonic::Status {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::Sqlx(sqlx::Error::RowNotFound) => {
                tonic::Status::not_found("core pod row not found")
            }
            RepoError::Sqlx(e) => {
                tonic::Status::internal(format!("postgres error: {e}"))
            }
            RepoError::Migrate(e) => {
                tonic::Status::internal(format!("migration error: {e}"))
            }
            RepoError::PodNotFound { world_uuid, dimension, pos_x, pos_y, pos_z } => {
                tonic::Status::not_found(format!(
                    "core pod {world_uuid}/{dimension}/({pos_x},{pos_y},{pos_z}) not found"
                ))
            }
        }
    }
}

// ── PodIdentifier ───────────────────────────────────────────────────────────

/// 5-tuple identifier for a core pod. Mirrors the proto `PodIdentifier`
/// message (`doc/14-rust-services.md` §3.2).
///
/// `Serialize` + `Deserialize` are derived so the gRPC layer
/// can embed the id inside `serde_json::json!({ "pod": id, … })`
/// event-meta payloads (see `biocapital-grpc::core_pod_service`
/// `tick` / `force_deplete` audit rows). We deliberately do
/// **not** add a `prost`/`tonic` derive: the on-wire type is
/// the proto `PodIdentifier` message, which the gRPC layer
/// maps to this struct in a hand-rolled `From` impl. Keeping
/// `biocapital-pg` free of `prost` also avoids a cycle with
/// the PG migration tooling.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PodIdentifier {
    pub world_uuid: Uuid,
    pub dimension: String,
    pub pos_x: i64,
    pub pos_y: i64,
    pub pos_z: i64,
}

impl PodIdentifier {
    /// Build a fresh identifier from the world / dimension / position.
    pub fn new(
        world_uuid: Uuid,
        dimension: impl Into<String>,
        pos_x: i64,
        pos_y: i64,
        pos_z: i64,
    ) -> Self {
        Self {
            world_uuid,
            dimension: dimension.into(),
            pos_x,
            pos_y,
            pos_z,
        }
    }
}

// ── Repository trait ────────────────────────────────────────────────────────

#[async_trait]
pub trait CorePodRepository: Send + Sync {
    /// Read one pod by its 5-tuple. Returns `PodNotFound` when the row
    /// doesn't exist yet.
    async fn get_pod(&self, id: &PodIdentifier) -> Result<CorePod, RepoError>;

    /// Insert-or-update a pod row. The 5-tuple is the primary key, so
    /// the second call against the same id overwrites all mutable
    /// columns.
    async fn upsert_pod(&self, pod: &CorePod) -> Result<(), RepoError>;

    /// List every pod currently hosting a given player. Backed by the
    /// `idx_core_pods_host` partial index.
    async fn list_pods_by_host(
        &self,
        host_uuid: Uuid,
    ) -> Result<Vec<CorePod>, RepoError>;

    /// List pods within a Manhattan / chunk radius around `center`.
    /// The radius is in **blocks** — implementations translate to the
    /// appropriate `BETWEEN` predicate. A radius of `0` returns just
    /// the pod exactly at `center`.
    async fn list_pods_in_chunk(
        &self,
        world: Uuid,
        dimension: &str,
        center: (i64, i64, i64),
        radius: i32,
    ) -> Result<Vec<CorePod>, RepoError>;
}

// ── Audit writer ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CorePodAuditEntry {
    pub log_id: Uuid,
    pub actor_uuid: Uuid,
    pub actor_type: &'static str,
    pub target_pod_world_uuid: Uuid,
    pub target_pod_dimension: String,
    pub target_pod_pos_x: i64,
    pub target_pod_pos_y: i64,
    pub target_pod_pos_z: i64,
    /// One of `"pod.tick"` / `"pod.enter"` / `"pod.exit"` /
    /// `"pod.stress_compute"` / `"pod.produce"`. Mirrors the
    /// `audit_core_pod.op` CHECK constraint.
    pub op: &'static str,
    pub stress_units: Option<f32>,
    pub rpm: Option<f32>,
    pub input_fluid_mb: Option<i32>,
    pub output_fluid_mb: Option<i32>,
    pub byproduct_count: Option<i64>,
    pub endurance_after: Option<f32>,
    pub tick_millis: i64,
    pub request_id: Option<Uuid>,
    pub notes: Option<serde_json::Value>,
}

#[async_trait]
pub trait CorePodAuditWriter: Send + Sync {
    async fn write(&self, entry: CorePodAuditEntry) -> Result<(), RepoError>;
}

// ── Postgres implementation ────────────────────────────────────────────────

#[derive(Clone)]
pub struct PgCorePodRepository {
    pool: PgPool,
}

impl PgCorePodRepository {
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

fn row_to_pod(row: &sqlx::postgres::PgRow) -> Result<CorePod, RepoError> {
    let world_uuid: Uuid = row.try_get("world_uuid").map_err(|e| {
        RepoError::Sqlx(sqlx::Error::Protocol(format!("world_uuid: {e}")))
    })?;
    let dimension: String = row
        .try_get("dimension")
        .map_err(|e| RepoError::Sqlx(sqlx::Error::Protocol(format!("dimension: {e}"))))?;
    let pos_x: i64 = row.try_get("pos_x").unwrap_or(0);
    let pos_y: i64 = row.try_get("pos_y").unwrap_or(0);
    let pos_z: i64 = row.try_get("pos_z").unwrap_or(0);
    let host_uuid: Option<Uuid> = row.try_get("host_uuid").ok();
    let endurance: f32 = row.try_get("endurance").unwrap_or(0.0);
    let recipe_cooldown: i64 = row.try_get("recipe_cooldown").unwrap_or(0);
    let input_fluid_id: Option<String> = row.try_get("input_fluid_id").ok();
    let input_fluid_mb: i32 = row.try_get("input_fluid_mb").unwrap_or(0);
    let output_fluid_id: Option<String> = row.try_get("output_fluid_id").ok();
    let output_fluid_mb: i32 = row.try_get("output_fluid_mb").unwrap_or(0);
    let byproduct_count: i64 = row.try_get("byproduct_count").unwrap_or(0);
    let created_tick: i64 = row.try_get("created_tick").unwrap_or(0);
    let updated_tick: i64 = row.try_get("updated_tick").unwrap_or(0);

    let input_fluid = input_fluid_id.map(|id| FluidStack {
        fluid_id: id,
        amount_mb: input_fluid_mb,
    });
    let output_fluid = output_fluid_id.map(|id| FluidStack {
        fluid_id: id,
        amount_mb: output_fluid_mb,
    });

    Ok(CorePod {
        world_uuid,
        dimension,
        pos_x,
        pos_y,
        pos_z,
        host_uuid,
        // Clamp at the SQL CHECK bound (defensive — a poisoned row
        // should not crash the gRPC server).
        endurance: endurance.clamp(0.0, 100.0),
        recipe_cooldown,
        input_fluid,
        output_fluid,
        byproduct_count,
        created_tick,
        updated_tick,
    })
}

#[async_trait]
impl CorePodRepository for PgCorePodRepository {
    async fn get_pod(&self, id: &PodIdentifier) -> Result<CorePod, RepoError> {
        let row_opt = sqlx::query(
            r#"
            SELECT world_uuid, dimension, pos_x, pos_y, pos_z,
                   host_uuid, endurance, recipe_cooldown,
                   input_fluid_id, input_fluid_mb,
                   output_fluid_id, output_fluid_mb,
                   byproduct_count, created_tick, updated_tick
              FROM core_pods
             WHERE world_uuid = $1
               AND dimension  = $2
               AND pos_x      = $3
               AND pos_y      = $4
               AND pos_z      = $5
            "#,
        )
        .bind(id.world_uuid)
        .bind(&id.dimension)
        .bind(id.pos_x)
        .bind(id.pos_y)
        .bind(id.pos_z)
        .fetch_optional(&self.pool)
        .await?;

        match row_opt {
            None => Err(RepoError::PodNotFound {
                world_uuid: id.world_uuid,
                dimension: id.dimension.clone(),
                pos_x: id.pos_x,
                pos_y: id.pos_y,
                pos_z: id.pos_z,
            }),
            Some(row) => row_to_pod(&row),
        }
    }

    async fn upsert_pod(&self, pod: &CorePod) -> Result<(), RepoError> {
        let (input_id, input_mb) = match &pod.input_fluid {
            Some(f) => (Some(f.fluid_id.clone()), f.amount_mb),
            None => (None, 0),
        };
        let (output_id, output_mb) = match &pod.output_fluid {
            Some(f) => (Some(f.fluid_id.clone()), f.amount_mb),
            None => (None, 0),
        };

        sqlx::query(
            r#"
            INSERT INTO core_pods
                   (world_uuid, dimension, pos_x, pos_y, pos_z,
                    host_uuid, endurance, recipe_cooldown,
                    input_fluid_id, input_fluid_mb,
                    output_fluid_id, output_fluid_mb,
                    byproduct_count, created_tick, updated_tick)
            VALUES ($1, $2, $3, $4, $5,
                    $6, $7, $8,
                    $9, $10,
                    $11, $12,
                    $13, $14, $15)
            ON CONFLICT (world_uuid, dimension, pos_x, pos_y, pos_z) DO UPDATE
            SET host_uuid        = EXCLUDED.host_uuid,
                endurance        = EXCLUDED.endurance,
                recipe_cooldown  = EXCLUDED.recipe_cooldown,
                input_fluid_id   = EXCLUDED.input_fluid_id,
                input_fluid_mb   = EXCLUDED.input_fluid_mb,
                output_fluid_id  = EXCLUDED.output_fluid_id,
                output_fluid_mb  = EXCLUDED.output_fluid_mb,
                byproduct_count  = EXCLUDED.byproduct_count,
                updated_tick     = EXCLUDED.updated_tick
            "#,
        )
        .bind(pod.world_uuid)
        .bind(&pod.dimension)
        .bind(pod.pos_x)
        .bind(pod.pos_y)
        .bind(pod.pos_z)
        .bind(pod.host_uuid)
        .bind(pod.endurance)
        .bind(pod.recipe_cooldown)
        .bind(input_id)
        .bind(input_mb)
        .bind(output_id)
        .bind(output_mb)
        .bind(pod.byproduct_count)
        .bind(pod.created_tick)
        .bind(pod.updated_tick)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_pods_by_host(
        &self,
        host_uuid: Uuid,
    ) -> Result<Vec<CorePod>, RepoError> {
        let rows = sqlx::query(
            r#"
            SELECT world_uuid, dimension, pos_x, pos_y, pos_z,
                   host_uuid, endurance, recipe_cooldown,
                   input_fluid_id, input_fluid_mb,
                   output_fluid_id, output_fluid_mb,
                   byproduct_count, created_tick, updated_tick
              FROM core_pods
             WHERE host_uuid = $1
             ORDER BY updated_tick DESC
            "#,
        )
        .bind(host_uuid)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_pod).collect()
    }

    async fn list_pods_in_chunk(
        &self,
        world: Uuid,
        dimension: &str,
        center: (i64, i64, i64),
        radius: i32,
    ) -> Result<Vec<CorePod>, RepoError> {
        let r = radius.max(0) as i64;
        let rows = sqlx::query(
            r#"
            SELECT world_uuid, dimension, pos_x, pos_y, pos_z,
                   host_uuid, endurance, recipe_cooldown,
                   input_fluid_id, input_fluid_mb,
                   output_fluid_id, output_fluid_mb,
                   byproduct_count, created_tick, updated_tick
              FROM core_pods
             WHERE world_uuid = $1
               AND dimension  = $2
               AND pos_x BETWEEN $3 AND $4
               AND pos_y BETWEEN $5 AND $6
               AND pos_z BETWEEN $7 AND $8
             ORDER BY updated_tick DESC
            "#,
        )
        .bind(world)
        .bind(dimension)
        .bind(center.0 - r)
        .bind(center.0 + r)
        .bind(center.1 - r)
        .bind(center.1 + r)
        .bind(center.2 - r)
        .bind(center.2 + r)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_pod).collect()
    }
}

// ── Audit writer ────────────────────────────────────────────────────────────

pub struct PgCorePodAuditWriter {
    pool: PgPool,
}

impl PgCorePodAuditWriter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CorePodAuditWriter for PgCorePodAuditWriter {
    async fn write(&self, entry: CorePodAuditEntry) -> Result<(), RepoError> {
        let notes = entry.notes.unwrap_or_else(|| serde_json::json!({}));
        sqlx::query(
            r#"
            INSERT INTO audit_core_pod
                   (log_id, actor_uuid, actor_type,
                    target_pod_world_uuid, target_pod_dimension,
                    target_pod_pos_x, target_pod_pos_y, target_pod_pos_z,
                    op, stress_units, rpm,
                    input_fluid_mb, output_fluid_mb, byproduct_count,
                    endurance_after, tick_millis, request_id, notes)
            VALUES ($1, $2, $3,
                    $4, $5,
                    $6, $7, $8,
                    $9, $10, $11,
                    $12, $13, $14,
                    $15, $16, $17, $18)
            "#,
        )
        .bind(entry.log_id)
        .bind(entry.actor_uuid)
        .bind(entry.actor_type)
        .bind(entry.target_pod_world_uuid)
        .bind(&entry.target_pod_dimension)
        .bind(entry.target_pod_pos_x)
        .bind(entry.target_pod_pos_y)
        .bind(entry.target_pod_pos_z)
        .bind(entry.op)
        .bind(entry.stress_units)
        .bind(entry.rpm)
        .bind(entry.input_fluid_mb)
        .bind(entry.output_fluid_mb)
        .bind(entry.byproduct_count)
        .bind(entry.endurance_after)
        .bind(entry.tick_millis)
        .bind(entry.request_id)
        .bind(notes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// ── Composite service deps ─────────────────────────────────────────────────

#[derive(Clone)]
pub struct CorePodServiceDeps {
    pub repo: std::sync::Arc<dyn CorePodRepository>,
    pub audit: std::sync::Arc<dyn CorePodAuditWriter>,
}

impl CorePodServiceDeps {
    pub fn new(
        repo: std::sync::Arc<dyn CorePodRepository>,
        audit: std::sync::Arc<dyn CorePodAuditWriter>,
    ) -> Self {
        Self { repo, audit }
    }
}

// ── Sanity tests (no live DB) ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trait_is_object_safe() {
        fn _assert_object_safe(_: std::sync::Arc<dyn CorePodRepository>) {}
        fn _assert_audit_object_safe(_: std::sync::Arc<dyn CorePodAuditWriter>) {}
    }

    #[test]
    fn pod_identifier_construction() {
        let world = Uuid::new_v4();
        let id = PodIdentifier::new(world, "minecraft:overworld", 1, 64, -1);
        assert_eq!(id.world_uuid, world);
        assert_eq!(id.dimension, "minecraft:overworld");
        assert_eq!(id.pos_x, 1);
        assert_eq!(id.pos_y, 64);
        assert_eq!(id.pos_z, -1);
    }
}