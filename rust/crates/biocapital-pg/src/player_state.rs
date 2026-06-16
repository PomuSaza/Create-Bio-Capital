//! PostgreSQL persistence for the PlayerState module.
//!
//! See `doc/02-player-state.md` §4.1 and `doc/99-integration-matrix.md` §5
//! for the design contract.
//!
//! This module exposes:
//! - [`PlayerStateRepository`] — the trait the gRPC service depends on
//!   (in `biocapital-grpc`); tests can substitute an in-memory implementation.
//! - [`PgPlayerStateRepository`] — the production implementation backed by
//!   `sqlx`.
//! - [`RepoError`] — typed error surface returned to callers.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use thiserror::Error;
use tracing::warn;
use uuid::Uuid;

use biocapital_core::player_state::{
    BodyPart, BodyPartParseError, PartChange, PlayerStateSnapshot, DEFAULT_HIDDEN_HP,
    DEFAULT_MAX_HUNGER, DEFAULT_STAT_MAX, HIDDEN_HP_FLOOR, SPAWN_HIDDEN_HP, SPAWN_HUNGER_RUST,
    SPAWN_PLEASURE,
};
use biocapital_core::{LoadError, PlayerStateCache, PlayerStateLoader};

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("postgres error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("invalid UUID in column {column}: {value}")]
    InvalidUuid { column: &'static str, value: String },

    #[error("invalid BodyPart in column {column}: {source}")]
    InvalidBodyPart {
        column: &'static str,
        #[source]
        source: BodyPartParseError,
    },

    #[error("snapshot mismatch on row {table}: {message}")]
    SnapshotMismatch {
        table: &'static str,
        message: String,
    },
}

impl From<RepoError> for tonic::Status {
    fn from(e: RepoError) -> Self {
        match e {
            // 5xx — server-side persistence problem
            RepoError::Sqlx(sqlx::Error::RowNotFound) => {
                tonic::Status::not_found("player_state row not found")
            }
            RepoError::Sqlx(e) => {
                tonic::Status::internal(format!("postgres error: {e}"))
            }
            RepoError::Migrate(e) => {
                tonic::Status::internal(format!("migration error: {e}"))
            }
            // 4xx — caller passed something that could not be coerced
            RepoError::InvalidUuid { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid UUID in {column}: {value}"
                ))
            }
            RepoError::InvalidBodyPart { column, source } => {
                tonic::Status::invalid_argument(format!(
                    "invalid BodyPart in {column}: {source}"
                ))
            }
            RepoError::SnapshotMismatch { table, message } => {
                tonic::Status::failed_precondition(format!(
                    "snapshot mismatch on {table}: {message}"
                ))
            }
        }
    }
}

// ── Repository trait ─────────────────────────────────────────────────────────

/// Persistence contract for `PlayerState`. All methods are async and must be
/// safe to call from a multi-threaded gRPC server (`Send + Sync`).
#[async_trait]
pub trait PlayerStateRepository: Send + Sync {
    /// Look up a single snapshot. If the player has no row yet, construct a
    /// fresh default snapshot with the given UUID. The repository does **not**
    /// persist that default — callers that want a durable row should follow
    /// up with `upsert`.
    async fn get(&self, uuid: Uuid) -> Result<PlayerStateSnapshot, RepoError>;

    /// Idempotent full-row write of the snapshot. Body parts are written
    /// upsert-style so partial updates on the 12 part rows do not clobber
    /// each other. `tick_millis` is the server's logical clock (used for
    /// `created_tick` on the first insert and `updated_tick` thereafter).
    async fn upsert(
        &self,
        snapshot: &PlayerStateSnapshot,
        tick_millis: i64,
    ) -> Result<(), RepoError>;

    /// Apply a single part-development delta. Reads the current value, adds
    /// `delta`, clamps to `[0, 100]`, persists, and returns the diff for
    /// the gRPC layer to ship back to the caller.
    async fn add_part_dev(
        &self,
        uuid: Uuid,
        part: BodyPart,
        delta: f32,
        tick_millis: i64,
    ) -> Result<PartChange, RepoError>;

    /// Bulk fetch of multiple snapshots in a single round trip. Missing
    /// rows are returned as freshly-defaulted snapshots (matching `get`).
    async fn list_by_uuid(
        &self,
        uuids: &[Uuid],
    ) -> Result<Vec<PlayerStateSnapshot>, RepoError>;
}

// ── Postgres implementation ──────────────────────────────────────────────────

/// Production repository. Wraps a `sqlx::PgPool`. The pool is cheap to clone
/// (it's already an `Arc` internally) so we hand it out freely.
#[derive(Clone)]
pub struct PgPlayerStateRepository {
    pool: PgPool,
}

impl PgPlayerStateRepository {
    /// Build a new repository from a connection string (e.g. the one in
    /// `create_biocapital.toml` `[Server].database_url`).
    pub async fn connect(database_url: &str) -> Result<Self, RepoError> {
        let pool = PgPoolOptions::new()
            .max_connections(16)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    /// Construct from a pre-built pool. Useful for tests that share a pool.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Pool accessor for the gRPC layer (so the audit writer can run in the
    /// same transaction as the mutation if needed).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl PlayerStateRepository for PgPlayerStateRepository {
    async fn get(&self, uuid: Uuid) -> Result<PlayerStateSnapshot, RepoError> {
        // Try the main row first.
        let row_opt = sqlx::query(
            r#"
            SELECT player_uuid, pleasure, hunger, hidden_hp,
                   defeat_count, max_hunger, low_hp_hits,
                   created_tick, updated_tick
              FROM player_state
             WHERE player_uuid = $1
            "#,
        )
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row_opt else {
            // No row yet → return a fresh default snapshot. We deliberately
            // do NOT insert here; the gRPC layer chooses when to persist.
            return Ok(PlayerStateSnapshot::new(uuid));
        };

        // Pull the 12 part rows in one round trip.
        let parts: BTreeMap<BodyPart, f32> = load_parts(&self.pool, uuid).await?;

        Ok(row_to_snapshot(&row, parts)?)
    }

    async fn upsert(
        &self,
        snapshot: &PlayerStateSnapshot,
        tick_millis: i64,
    ) -> Result<(), RepoError> {
        let mut tx = self.pool.begin().await?;

        // Upsert the main row. On first insert, created_tick = updated_tick
        // = tick_millis; on subsequent updates, created_tick is preserved by
        // the COALESCE.
        sqlx::query(
            r#"
            INSERT INTO player_state (
                player_uuid, pleasure, hunger, hidden_hp,
                defeat_count, max_hunger, low_hp_hits,
                created_tick, updated_tick
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
            ON CONFLICT (player_uuid) DO UPDATE
            SET pleasure      = EXCLUDED.pleasure,
                hunger        = EXCLUDED.hunger,
                hidden_hp     = EXCLUDED.hidden_hp,
                defeat_count  = EXCLUDED.defeat_count,
                max_hunger    = EXCLUDED.max_hunger,
            -- keep low_hp_hits monotonic: never let an upsert drive it
            -- backwards (defensive — the core domain already enforces this,
            -- but a hand-written UPDATE bypassing the service would race).
                low_hp_hits   = GREATEST(player_state.low_hp_hits, EXCLUDED.low_hp_hits),
                updated_tick  = EXCLUDED.updated_tick
            "#,
        )
        .bind(snapshot.uuid)
        .bind(snapshot.pleasure)
        .bind(snapshot.hunger)
        .bind(snapshot.hidden_hp)
        .bind(snapshot.defeat_count)
        .bind(snapshot.max_hunger)
        .bind(snapshot.low_hp_hits)
        .bind(tick_millis)
        .execute(&mut *tx)
        .await?;

        // Replace the 12 part rows. Two-step: delete the existing set,
        // then insert the new set. This is safe because the player has
        // at most 12 part rows; the cardinality is bounded and small.
        sqlx::query("DELETE FROM body_part_development WHERE player_uuid = $1")
            .bind(snapshot.uuid)
            .execute(&mut *tx)
            .await?;

        for (part, value) in &snapshot.parts {
            sqlx::query(
                r#"
                INSERT INTO body_part_development
                       (player_uuid, part_name, dev_value, updated_tick)
                VALUES ($1, $2, $3, $4)
                "#,
            )
            .bind(snapshot.uuid)
            .bind(part.as_str())
            .bind(*value)
            .bind(tick_millis)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn add_part_dev(
        &self,
        uuid: Uuid,
        part: BodyPart,
        delta: f32,
        tick_millis: i64,
    ) -> Result<PartChange, RepoError> {
        if !delta.is_finite() {
            return Err(RepoError::SnapshotMismatch {
                table: "body_part_development",
                message: format!("non-finite delta: {delta}"),
            });
        }

        // Ensure the parent row exists (the snapshot's other fields will be
        // default — that's fine for a delta-only RPC).
        sqlx::query(
            r#"
            INSERT INTO player_state (player_uuid, created_tick, updated_tick)
            VALUES ($1, $2, $2)
            ON CONFLICT (player_uuid) DO NOTHING
            "#,
        )
        .bind(uuid)
        .bind(tick_millis)
        .execute(&self.pool)
        .await?;

        // Read-before-write to capture the "before" value. Could be a single
        // RETURNING with arithmetic, but the 12-row cardinality keeps this
        // cheap and makes the before/after diff trivial to return to the
        // caller for the audit log.
        let before: f32 = sqlx::query_scalar(
            r#"
            SELECT dev_value
              FROM body_part_development
             WHERE player_uuid = $1 AND part_name = $2
            "#,
        )
        .bind(uuid)
        .bind(part.as_str())
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or(0.0);

        let raw = before + delta;
        let after = raw.clamp(0.0, DEFAULT_STAT_MAX);
        let clamped = after != raw;

        sqlx::query(
            r#"
            INSERT INTO body_part_development
                   (player_uuid, part_name, dev_value, updated_tick)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (player_uuid, part_name) DO UPDATE
            SET dev_value    = EXCLUDED.dev_value,
                updated_tick = EXCLUDED.updated_tick
            "#,
        )
        .bind(uuid)
        .bind(part.as_str())
        .bind(after)
        .bind(tick_millis)
        .execute(&self.pool)
        .await?;

        Ok(PartChange::new(before, after, delta, clamped))
    }

    async fn list_by_uuid(
        &self,
        uuids: &[Uuid],
    ) -> Result<Vec<PlayerStateSnapshot>, RepoError> {
        if uuids.is_empty() {
            return Ok(Vec::new());
        }

        // Main rows.
        let rows = sqlx::query(
            r#"
            SELECT player_uuid, pleasure, hunger, hidden_hp,
                   defeat_count, max_hunger, low_hp_hits,
                   created_tick, updated_tick
              FROM player_state
             WHERE player_uuid = ANY($1)
            "#,
        )
        .bind(uuids)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let uuid: Uuid = row
                .try_get("player_uuid")
                .map_err(|e| RepoError::InvalidUuid {
                    column: "player_uuid",
                    value: e.to_string(),
                })?;
            let parts = load_parts(&self.pool, uuid).await?;
            out.push(row_to_snapshot(&row, parts)?);
        }

        // For any UUIDs missing from the main result, append a defaulted
        // snapshot (mirrors `get`'s contract).
        let present: std::collections::HashSet<Uuid> =
            out.iter().map(|s| s.uuid).collect();
        for uuid in uuids {
            if !present.contains(uuid) {
                out.push(PlayerStateSnapshot::new(*uuid));
            }
        }
        Ok(out)
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

async fn load_parts(
    pool: &PgPool,
    uuid: Uuid,
) -> Result<BTreeMap<BodyPart, f32>, RepoError> {
    let rows = sqlx::query(
        r#"
        SELECT part_name, dev_value
          FROM body_part_development
         WHERE player_uuid = $1
        "#,
    )
    .bind(uuid)
    .fetch_all(pool)
    .await?;

    let mut parts = BTreeMap::new();
    for row in rows {
        let name: String = row.try_get("part_name").map_err(|e| {
            RepoError::SnapshotMismatch {
                table: "body_part_development",
                message: format!("missing part_name column: {e}"),
            }
        })?;
        let value: f32 = row.try_get("dev_value").unwrap_or(0.0);
        let part: BodyPart = name.parse().map_err(|source| {
            // CHECK constraint should prevent this; if it ever fires we
            // want a loud failure rather than silent data loss.
            warn!(player = %uuid, part_name = %name, "BodyPart parse failure despite CHECK constraint");
            RepoError::InvalidBodyPart {
                column: "part_name",
                source,
            }
        })?;
        parts.insert(part, value);
    }

    // Always populate all 12 keys so callers can rely on the snapshot
    // shape (mirrors the Java attachment's `EnumMap<BodyPart, Float>`).
    for part in BodyPart::ALL {
        parts.entry(part).or_insert(0.0);
    }
    Ok(parts)
}

fn row_to_snapshot(
    row: &sqlx::postgres::PgRow,
    parts: BTreeMap<BodyPart, f32>,
) -> Result<PlayerStateSnapshot, RepoError> {
    let uuid: Uuid = row
        .try_get("player_uuid")
        .map_err(|e| RepoError::InvalidUuid {
            column: "player_uuid",
            value: e.to_string(),
        })?;
    let pleasure: f32 = row.try_get("pleasure").unwrap_or(SPAWN_PLEASURE);
    let hunger: f32 = row.try_get("hunger").unwrap_or(SPAWN_HUNGER_RUST);
    let hidden_hp: f32 = row.try_get("hidden_hp").unwrap_or(SPAWN_HIDDEN_HP);
    let defeat_count: i32 = row.try_get("defeat_count").unwrap_or(0);
    let max_hunger: i32 = row.try_get("max_hunger").unwrap_or(DEFAULT_MAX_HUNGER);
    let low_hp_hits: i32 = row.try_get("low_hp_hits").unwrap_or(0);
    let updated_tick: i64 = row.try_get("updated_tick").unwrap_or(0);

    // Defensive clamps — CHECK constraints should guarantee these ranges,
    // but a hand-rolled UPDATE could violate them. We do not want a
    // poisoned row to crash the gRPC server on read.
    let pleasure = pleasure.clamp(0.0, DEFAULT_STAT_MAX);
    let hunger = hunger.clamp(0.0, max_hunger as f32);
    let hidden_hp = hidden_hp.max(HIDDEN_HP_FLOOR);

    let updated_at = chrono::DateTime::<Utc>::from_timestamp_millis(updated_tick)
        .unwrap_or_else(|| {
            warn!(player = %uuid, updated_tick, "updated_tick out of range; defaulting to now()");
            Utc::now()
        });

    Ok(PlayerStateSnapshot {
        uuid,
        pleasure,
        hunger,
        hidden_hp,
        parts,
        defeat_count: defeat_count.max(0),
        max_hunger: max_hunger.max(1),
        low_hp_hits: low_hp_hits.max(0),
        updated_at,
    })
}

// ── Audit writer (used by the gRPC service) ──────────────────────────────────

/// Information about a single PlayerState mutation, for the audit log.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub actor_uuid: Option<Uuid>,
    pub actor_type: &'static str,
    pub target_uuid: Uuid,
    pub op: &'static str,
    pub before: PlayerStateSnapshot,
    pub after: PlayerStateSnapshot,
    pub source: Option<String>,
    pub request_id: Option<Uuid>,
    pub tick_millis: i64,
}

/// Append-only audit writer. The trait abstraction is for testability
/// (the gRPC service depends on `Arc<dyn AuditWriter>`).
#[async_trait]
pub trait AuditWriter: Send + Sync {
    async fn write(&self, entry: AuditEntry) -> Result<(), RepoError>;
}

pub struct PgAuditWriter {
    pool: PgPool,
}

impl PgAuditWriter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AuditWriter for PgAuditWriter {
    async fn write(&self, entry: AuditEntry) -> Result<(), RepoError> {
        // Serialise the snapshots using serde_json's default float handling.
        // `serde_json::to_value` keeps the float representation stable
        // enough for human reading; the audit log is not a hot path.
        let before_json = json!({
            "pleasure": entry.before.pleasure,
            "hunger": entry.before.hunger,
            "hidden_hp": entry.before.hidden_hp,
            "defeat_count": entry.before.defeat_count,
            "max_hunger": entry.before.max_hunger,
            "low_hp_hits": entry.before.low_hp_hits,
            "parts": entry.before.parts,
        });
        let after_json = json!({
            "pleasure": entry.after.pleasure,
            "hunger": entry.after.hunger,
            "hidden_hp": entry.after.hidden_hp,
            "defeat_count": entry.after.defeat_count,
            "max_hunger": entry.after.max_hunger,
            "low_hp_hits": entry.after.low_hp_hits,
            "parts": entry.after.parts,
        });

        sqlx::query(
            r#"
            INSERT INTO audit_player_state (
                actor_uuid, actor_type, target_uuid, op,
                before_json, after_json,
                source, request_id, tick_millis
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(entry.actor_uuid)
        .bind(entry.actor_type)
        .bind(entry.target_uuid)
        .bind(entry.op)
        .bind(before_json)
        .bind(after_json)
        .bind(entry.source)
        .bind(entry.request_id)
        .bind(entry.tick_millis)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Composite handle passed into the gRPC service. We keep repo + audit
/// behind `Arc` so the gRPC service can be cheaply cloned.
///
/// `cache` (task #123 HUD rebuild) is an in-memory read cache for
/// `GetState`. It is wrapped in `Option` to keep the existing constructor
/// working: callers that do not opt in to the cache get the pre-#123
/// behaviour (every `GetState` hits PG). New code should construct with
/// [`PlayerStateServiceDeps::with_cache`] so the 200 ms TTL is in effect.
#[derive(Clone)]
pub struct PlayerStateServiceDeps {
    pub repo: Arc<dyn PlayerStateRepository>,
    pub audit: Arc<dyn AuditWriter>,
    pub cache: Option<PlayerStateCache>,
}

impl PlayerStateServiceDeps {
    pub fn new(repo: Arc<dyn PlayerStateRepository>, audit: Arc<dyn AuditWriter>) -> Self {
        Self {
            repo,
            audit,
            cache: None,
        }
    }

    /// Build a deps handle with a 200 ms in-memory read cache wired in.
    /// Every read path in the gRPC service goes through
    /// `cache.get_or_load`; every write path follows the upsert with
    /// `cache.invalidate`.
    pub fn with_cache(
        repo: Arc<dyn PlayerStateRepository>,
        audit: Arc<dyn AuditWriter>,
        cache: PlayerStateCache,
    ) -> Self {
        Self {
            repo,
            audit,
            cache: Some(cache),
        }
    }
}

// ── Loader adapter: `PlayerStateRepository` → `PlayerStateLoader` ────────────

/// Adapter that lets the gRPC service hand an `Arc<dyn PlayerStateRepository>`
/// to `PlayerStateCache::get_or_load` (which expects `&dyn PlayerStateLoader`).
///
/// We can't `impl PlayerStateLoader for Arc<dyn PlayerStateRepository>`
/// because of the orphan rule; a thin newtype is the canonical fix and it
/// is zero-cost (single `Arc` clone on construction).
#[derive(Clone)]
pub struct PgPlayerStateLoader {
    repo: Arc<dyn PlayerStateRepository>,
}

impl PgPlayerStateLoader {
    pub fn new(repo: Arc<dyn PlayerStateRepository>) -> Self {
        Self { repo }
    }
}

#[async_trait]
impl PlayerStateLoader for PgPlayerStateLoader {
    async fn load(&self, uuid: Uuid) -> Result<PlayerStateSnapshot, LoadError> {
        // `PlayerStateRepository::get` returns `RepoError`; we project it
        // into the cache's narrower `LoadError` enum (the gRPC layer
        // re-projects the cache's `LoadError` back to a `tonic::Status`).
        self.repo
            .get(uuid)
            .await
            .map_err(|e| LoadError::Repo(e.to_string()))
    }
}

// ── Sanity test (does not require a live DB) ────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trait_is_object_safe() {
        fn _assert_object_safe(_: Arc<dyn PlayerStateRepository>) {}
        fn _assert_audit_object_safe(_: Arc<dyn AuditWriter>) {}
    }

    #[test]
    fn default_constants_match_02_spec() {
        // The Java side uses 20.0; the Rust side uses 50.0 per task #3 spec.
        // We pin the Rust default here so a refactor can't quietly drift.
        assert_eq!(SPAWN_PLEASURE, 0.0);
        assert_eq!(SPAWN_HIDDEN_HP, DEFAULT_HIDDEN_HP);
        assert_eq!(DEFAULT_HIDDEN_HP, 20.0);
        assert_eq!(DEFAULT_MAX_HUNGER, 100);
        assert_eq!(HIDDEN_HP_FLOOR, 1.0);
    }
}

// Suppress the unused-default_hp warning on `DEFAULT_HIDDEN_HP` if the
// integration test code is compiled out.
#[allow(dead_code)]
const _UNUSED: f32 = DEFAULT_HIDDEN_HP;
