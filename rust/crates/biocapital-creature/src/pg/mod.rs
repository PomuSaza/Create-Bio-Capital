//! PostgreSQL persistence for the creature-customization
//! module (`doc/13-bio-customization.md` §6.3 +
//! `doc/99-integration-matrix.md` §5).
//!
//! Companion to the domain type in
//! `biocapital_creature::domain::creature::CreatureConfig`.
//! This module is the only piece of code that talks to the
//! `creature_configs` table.
//!
//! Public surface:
//! - [`CreatureConfigRepository`] — trait the gRPC service
//!   depends on (in `biocapital-grpc::creature_service`);
//!   tests can substitute an in-memory implementation.
//! - [`PgCreatureConfigRepository`] — production implementation
//!   backed by `sqlx::PgPool`.
//! - [`CreatureAuditWriter`] — append-only audit trait; the
//!   `creature.reload` / `creature.unload` /
//!   `creature.reload_failed` / `creature.reload_all` ops
//!   each go through this trait.
//! - [`CreatureRepoError`] — typed error surface; the
//!   `tonic::Status` `From` impl is implemented so the gRPC
//!   layer can `?`-bubble errors without an extra match arm.
//! - [`MobReplacementRepository`] — the
//!   `mob_replacements` persistence contract (re-exported via
//!   [`mob_replacement`]). See [`mob_replacement`] for the
//!   trait / service-deps / Pg stub surface.
//!
//! Idempotency: the `creature_configs` table is the canonical
//! truth. `upsert` keys on the `creature_id` PK. There is no
//! separate `request_id` on the upsert path; the gRPC layer
//! uses the hot-reload tick's `tick_millis` for ordering.

pub mod mob_replacement;

pub use mob_replacement::{
    MobReplacementRepository, MobReplacementServiceDeps, MobRepoError,
    PgMobReplacementRepository, RepoError as MobReplacementRepoError,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

use crate::domain::CreatureConfig;

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("postgres error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("invalid JSONB payload: {0}")]
    InvalidJson(String),

    #[error("creature_configs row not found for creature_id={0}")]
    NotFound(String),
}

/// Re-export alias so callers can disambiguate from
/// `bank::RepoError` / `player_state::RepoError` /
/// `core_pod::RepoError` / `dglab::RepoError` /
/// `contract::RepoError` / `fluid::RepoError` /
/// `mob_replacement::RepoError`. The enums are kept distinct
/// so the gRPC layer can `From`-convert each to the right
/// tonic status without an extra match arm.
pub type CreatureRepoError = RepoError;

impl From<RepoError> for tonic::Status {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::Sqlx(sqlx::Error::RowNotFound) => {
                tonic::Status::not_found("creature_configs row not found")
            }
            RepoError::Sqlx(e) => {
                tonic::Status::internal(format!("postgres error: {e}"))
            }
            RepoError::Migrate(e) => {
                tonic::Status::internal(format!("migration error: {e}"))
            }
            RepoError::InvalidJson(detail) => {
                tonic::Status::invalid_argument(format!(
                    "creature_configs invalid JSONB payload: {detail}"
                ))
            }
            RepoError::NotFound(creature_id) => {
                tonic::Status::not_found(format!(
                    "creature_configs row not found for creature_id={creature_id}"
                ))
            }
        }
    }
}

// ── CreatureConfigRecord ────────────────────────────────────────────────────

/// One row in the `creature_configs` table. Mirrors
/// `biocapital_creature::domain::CreatureConfig` plus the
/// cache + bookkeeping columns
/// (`display_name_zh` / `display_name_en` /
/// `last_loaded_at` / `last_loaded_tick` / `source_path` /
/// `source_mtime` / `reload_failed_count` / `notes`).
///
/// The full `CreatureConfig` lives inside `config_json`; the
/// struct here keeps that as a `serde_json::Value` so the
/// repository layer never has to re-parse it (the gRPC
/// service does the parse on read).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatureConfigRecord {
    pub creature_id: String,
    /// Full `CreatureConfig` projection. Held as raw
    /// `serde_json::Value` so the repo does not need to depend
    /// on the domain deserialiser (keeps the layering
    /// one-way).
    pub config_json: serde_json::Value,

    /// Denormalised cache columns (mirror
    /// `config_json -> 'display_name' -> 'zh_cn' / 'en_us'`).
    /// May be empty when the underlying config has no
    /// `display_name` entry at all.
    pub display_name_zh: Option<String>,
    pub display_name_en: Option<String>,

    pub enabled: bool,

    pub last_loaded_at: DateTime<Utc>,
    pub last_loaded_tick: i64,

    pub source_path: String,
    pub source_mtime: i64,

    pub reload_failed_count: i32,
    pub notes: Option<serde_json::Value>,
}

impl CreatureConfigRecord {
    /// Build a record from a freshly-parsed `CreatureConfig`
    /// + the file-system / tick metadata the loader knows.
    /// Convenience used by the hot-reload code path.
    pub fn from_loaded(
        config: &CreatureConfig,
        source_path: impl Into<String>,
        source_mtime: i64,
        last_loaded_tick: i64,
        now: DateTime<Utc>,
    ) -> Result<Self, RepoError> {
        let config_json = serde_json::to_value(config).map_err(|e| {
            RepoError::InvalidJson(format!("failed to serialise CreatureConfig: {e}"))
        })?;
        let display_name_zh = config.display_name.get("zh_cn").cloned();
        let display_name_en = config.display_name.get("en_us").cloned();
        Ok(Self {
            creature_id: config.id.clone(),
            config_json,
            display_name_zh,
            display_name_en,
            enabled: config.enabled,
            last_loaded_at: now,
            last_loaded_tick,
            source_path: source_path.into(),
            source_mtime,
            reload_failed_count: 0,
            notes: None,
        })
    }
}

// ── Audit payload ───────────────────────────────────────────────────────────

/// One row written to `audit_creature_config`. Mirrors the
/// 99 §2.2 baseline (actor / actor_type / target / op /
/// before / after / tick / request_id / notes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatureAuditEntry {
    pub log_id: Uuid,
    pub actor_uuid: Option<Uuid>,
    /// One of `"RUST_SERVICE"` / `"ADMIN_CMD"`. The PG CHECK
    /// constraint mirrors these two values exactly.
    pub actor_type: String,
    pub target_creature_id: Option<String>,
    /// One of `"creature.load"` / `"creature.reload"` /
    /// `"creature.unload"` / `"creature.reload_failed"` /
    /// `"creature.reload_all"`.
    pub op: String,
    pub before_json: Option<serde_json::Value>,
    pub after_json: Option<serde_json::Value>,
    pub tick_millis: i64,
    pub request_id: Option<Uuid>,
    pub notes: Option<serde_json::Value>,
}

impl CreatureAuditEntry {
    pub fn reload_all(
        actor_uuid: Option<Uuid>,
        actor_type: impl Into<String>,
        tick_millis: i64,
        request_id: Option<Uuid>,
        notes: serde_json::Value,
    ) -> Self {
        Self {
            log_id: Uuid::new_v4(),
            actor_uuid,
            actor_type: actor_type.into(),
            target_creature_id: None,
            op: "creature.reload_all".to_string(),
            before_json: None,
            after_json: None,
            tick_millis,
            request_id,
            notes: Some(notes),
        }
    }

    pub fn loaded(
        actor_uuid: Option<Uuid>,
        actor_type: impl Into<String>,
        target_creature_id: impl Into<String>,
        after_json: serde_json::Value,
        tick_millis: i64,
        request_id: Option<Uuid>,
    ) -> Self {
        Self {
            log_id: Uuid::new_v4(),
            actor_uuid,
            actor_type: actor_type.into(),
            target_creature_id: Some(target_creature_id.into()),
            op: "creature.load".to_string(),
            before_json: None,
            after_json: Some(after_json),
            tick_millis,
            request_id,
            notes: None,
        }
    }

    pub fn reloaded(
        actor_uuid: Option<Uuid>,
        actor_type: impl Into<String>,
        target_creature_id: impl Into<String>,
        before_json: serde_json::Value,
        after_json: serde_json::Value,
        tick_millis: i64,
        request_id: Option<Uuid>,
    ) -> Self {
        Self {
            log_id: Uuid::new_v4(),
            actor_uuid,
            actor_type: actor_type.into(),
            target_creature_id: Some(target_creature_id.into()),
            op: "creature.reload".to_string(),
            before_json: Some(before_json),
            after_json: Some(after_json),
            tick_millis,
            request_id,
            notes: None,
        }
    }

    pub fn unloaded(
        actor_uuid: Option<Uuid>,
        actor_type: impl Into<String>,
        target_creature_id: impl Into<String>,
        before_json: serde_json::Value,
        tick_millis: i64,
        request_id: Option<Uuid>,
    ) -> Self {
        Self {
            log_id: Uuid::new_v4(),
            actor_uuid,
            actor_type: actor_type.into(),
            target_creature_id: Some(target_creature_id.into()),
            op: "creature.unload".to_string(),
            before_json: Some(before_json),
            after_json: None,
            tick_millis,
            request_id,
            notes: None,
        }
    }

    pub fn reload_failed(
        actor_uuid: Option<Uuid>,
        actor_type: impl Into<String>,
        target_creature_id: impl Into<String>,
        tick_millis: i64,
        request_id: Option<Uuid>,
        error: impl Into<String>,
    ) -> Self {
        let notes = serde_json::json!({ "error": error.into() });
        Self {
            log_id: Uuid::new_v4(),
            actor_uuid,
            actor_type: actor_type.into(),
            target_creature_id: Some(target_creature_id.into()),
            op: "creature.reload_failed".to_string(),
            before_json: None,
            after_json: None,
            tick_millis,
            request_id,
            notes: Some(notes),
        }
    }
}

// ── Repository trait ────────────────────────────────────────────────────────

/// Persistence contract for `creature_configs`. The trait
/// exposes the read paths the gRPC `CreatureService` needs
/// (`get` / `list`) and the housekeeping write paths the
/// `CreatureHotReloader` (task #11) drives.
#[async_trait]
pub trait CreatureConfigRepository: Send + Sync {
    /// Insert-or-update one row keyed on `creature_id`.
    /// Mirrors `mob_replacements.upsert`: on conflict the
    /// `config_json` / display names / `enabled` / `notes` /
    /// `source_*` / `last_loaded_*` columns are overwritten,
    /// `reload_failed_count` is reset to 0.
    async fn upsert(&self, record: &CreatureConfigRecord) -> Result<(), RepoError>;

    /// Read one row by `creature_id`. Returns `None` when no
    /// such row exists.
    async fn get(&self, creature_id: &str) -> Result<Option<CreatureConfigRecord>, RepoError>;

    /// List rows. When `enabled_only = true`, only `enabled =
    /// TRUE` rows are returned (the hot path for
    /// `ListCreatures`).
    async fn list(
        &self,
        enabled_only: bool,
    ) -> Result<Vec<CreatureConfigRecord>, RepoError>;

    /// Delete one row by `creature_id`. Returns `Err(NotFound)`
    /// when no such row exists. Used by the hot-reload watcher
    /// when the .json file is removed from disk.
    async fn delete(&self, creature_id: &str) -> Result<(), RepoError>;

    /// Hot-reload probe: list all rows whose `source_mtime <
    /// older_than` (epoch seconds). The reloader passes the
    /// current epoch and walks the result to figure out which
    /// files are stale.
    ///
    /// Implementation note: rows whose `source_mtime` is
    /// already up-to-date appear in the result only when the
    /// .json file has been **deleted** from disk (the reloader
    /// cross-references the result against the live directory
    /// listing). The index `idx_creature_configs_source_mtime`
    /// keeps this scan cheap.
    async fn list_by_source_mtime(
        &self,
        older_than: i64,
    ) -> Result<Vec<CreatureConfigRecord>, RepoError>;

    /// Bump `reload_failed_count` for one row (called when a
    /// parse / validate failure happens during a hot-reload
    /// tick). The current value is incremented by 1.
    async fn increment_reload_failed_count(
        &self,
        creature_id: &str,
    ) -> Result<(), RepoError>;
}

// ── Audit writer trait ──────────────────────────────────────────────────────

/// Append-only audit writer for `audit_creature_config`. The
/// gRPC service + the hot-reload watcher both write through
/// this trait.
#[async_trait]
pub trait CreatureAuditWriter: Send + Sync {
    async fn write(&self, entry: &CreatureAuditEntry) -> Result<(), RepoError>;
}

// ── Postgres implementation ────────────────────────────────────────────────

#[derive(Clone)]
pub struct PgCreatureConfigRepository {
    pool: PgPool,
}

impl PgCreatureConfigRepository {
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

#[derive(Clone)]
pub struct PgCreatureAuditWriter {
    pool: PgPool,
}

impl PgCreatureAuditWriter {
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

fn row_to_record(row: &sqlx::postgres::PgRow) -> Result<CreatureConfigRecord, RepoError> {
    let creature_id: String = row.try_get("creature_id").map_err(|e| {
        RepoError::InvalidJson(format!("creature_id: {e}"))
    })?;
    let config_json: serde_json::Value = row.try_get("config_json").map_err(|e| {
        RepoError::InvalidJson(format!("config_json: {e}"))
    })?;
    let display_name_zh: Option<String> = row.try_get("display_name_zh").ok();
    let display_name_en: Option<String> = row.try_get("display_name_en").ok();
    let enabled: bool = row.try_get("enabled").unwrap_or(true);
    let last_loaded_at: DateTime<Utc> = row
        .try_get("last_loaded_at")
        .map_err(|e| RepoError::InvalidJson(format!("last_loaded_at: {e}")))?;
    let last_loaded_tick: i64 = row.try_get("last_loaded_tick").unwrap_or(0);
    let source_path: String = row.try_get("source_path").unwrap_or_default();
    let source_mtime: i64 = row.try_get("source_mtime").unwrap_or(0);
    let reload_failed_count: i32 = row.try_get("reload_failed_count").unwrap_or(0);
    let notes: Option<serde_json::Value> = row.try_get("notes").ok();

    Ok(CreatureConfigRecord {
        creature_id,
        config_json,
        display_name_zh,
        display_name_en,
        enabled,
        last_loaded_at,
        last_loaded_tick,
        source_path,
        source_mtime,
        reload_failed_count,
        notes,
    })
}

#[async_trait]
impl CreatureConfigRepository for PgCreatureConfigRepository {
    async fn upsert(&self, record: &CreatureConfigRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
            INSERT INTO creature_configs (
                creature_id, config_json, display_name_zh, display_name_en,
                enabled, last_loaded_at, last_loaded_tick,
                source_path, source_mtime, reload_failed_count, notes
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (creature_id) DO UPDATE
              SET config_json         = EXCLUDED.config_json,
                  display_name_zh     = EXCLUDED.display_name_zh,
                  display_name_en     = EXCLUDED.display_name_en,
                  enabled             = EXCLUDED.enabled,
                  last_loaded_at      = EXCLUDED.last_loaded_at,
                  last_loaded_tick    = EXCLUDED.last_loaded_tick,
                  source_path         = EXCLUDED.source_path,
                  source_mtime        = EXCLUDED.source_mtime,
                  reload_failed_count = 0,
                  notes               = EXCLUDED.notes
            "#,
        )
        .bind(&record.creature_id)
        .bind(&record.config_json)
        .bind(&record.display_name_zh)
        .bind(&record.display_name_en)
        .bind(record.enabled)
        .bind(record.last_loaded_at)
        .bind(record.last_loaded_tick)
        .bind(&record.source_path)
        .bind(record.source_mtime)
        .bind(record.reload_failed_count)
        .bind(&record.notes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(
        &self,
        creature_id: &str,
    ) -> Result<Option<CreatureConfigRecord>, RepoError> {
        let row = sqlx::query(
            r#"
            SELECT creature_id, config_json, display_name_zh, display_name_en,
                   enabled, last_loaded_at, last_loaded_tick,
                   source_path, source_mtime, reload_failed_count, notes
              FROM creature_configs
             WHERE creature_id = $1
            "#,
        )
        .bind(creature_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| row_to_record(&r)).transpose()
    }

    async fn list(
        &self,
        enabled_only: bool,
    ) -> Result<Vec<CreatureConfigRecord>, RepoError> {
        let rows = if enabled_only {
            sqlx::query(
                r#"
                SELECT creature_id, config_json, display_name_zh, display_name_en,
                       enabled, last_loaded_at, last_loaded_tick,
                       source_path, source_mtime, reload_failed_count, notes
                  FROM creature_configs
                 WHERE enabled = TRUE
                 ORDER BY creature_id ASC
                "#,
            )
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT creature_id, config_json, display_name_zh, display_name_en,
                       enabled, last_loaded_at, last_loaded_tick,
                       source_path, source_mtime, reload_failed_count, notes
                  FROM creature_configs
                 ORDER BY creature_id ASC
                "#,
            )
            .fetch_all(&self.pool)
            .await?
        };
        rows.iter().map(row_to_record).collect()
    }

    async fn delete(&self, creature_id: &str) -> Result<(), RepoError> {
        let result = sqlx::query(
            r#"
            DELETE FROM creature_configs
             WHERE creature_id = $1
            "#,
        )
        .bind(creature_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(RepoError::NotFound(creature_id.to_string()));
        }
        Ok(())
    }

    async fn list_by_source_mtime(
        &self,
        older_than: i64,
    ) -> Result<Vec<CreatureConfigRecord>, RepoError> {
        let rows = sqlx::query(
            r#"
            SELECT creature_id, config_json, display_name_zh, display_name_en,
                   enabled, last_loaded_at, last_loaded_tick,
                   source_path, source_mtime, reload_failed_count, notes
              FROM creature_configs
             WHERE source_mtime < $1
             ORDER BY source_mtime ASC
            "#,
        )
        .bind(older_than)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_record).collect()
    }

    async fn increment_reload_failed_count(
        &self,
        creature_id: &str,
    ) -> Result<(), RepoError> {
        sqlx::query(
            r#"
            UPDATE creature_configs
               SET reload_failed_count = reload_failed_count + 1
             WHERE creature_id = $1
            "#,
        )
        .bind(creature_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[async_trait]
impl CreatureAuditWriter for PgCreatureAuditWriter {
    async fn write(&self, entry: &CreatureAuditEntry) -> Result<(), RepoError> {
        sqlx::query(
            r#"
            INSERT INTO audit_creature_config (
                log_id, actor_uuid, actor_type, target_creature_id, op,
                before_json, after_json, tick_millis, request_id, notes
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(entry.log_id)
        .bind(entry.actor_uuid)
        .bind(&entry.actor_type)
        .bind(&entry.target_creature_id)
        .bind(&entry.op)
        .bind(&entry.before_json)
        .bind(&entry.after_json)
        .bind(entry.tick_millis)
        .bind(entry.request_id)
        .bind(&entry.notes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// ── Composite service deps ─────────────────────────────────────────────────

/// Composite handle passed into `CreatureService` (task #11).
/// Mirrors `MobReplacementServiceDeps` shape.
#[derive(Clone)]
pub struct CreatureConfigServiceDeps {
    pub repo: std::sync::Arc<dyn CreatureConfigRepository>,
    pub audit: std::sync::Arc<dyn CreatureAuditWriter>,
}

impl CreatureConfigServiceDeps {
    pub fn new(
        repo: std::sync::Arc<dyn CreatureConfigRepository>,
        audit: std::sync::Arc<dyn CreatureAuditWriter>,
    ) -> Self {
        Self { repo, audit }
    }
}

// ── Sanity test (no live DB) ────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trait_is_object_safe() {
        fn _assert_object_safe_repo(_: std::sync::Arc<dyn CreatureConfigRepository>) {}
        fn _assert_object_safe_audit(_: std::sync::Arc<dyn CreatureAuditWriter>) {}
    }

    #[test]
    fn from_loaded_populates_caches() {
        use crate::domain::CreatureConfig;
        let cfg = CreatureConfig::placeholder_variant_zombie();
        let rec = CreatureConfigRecord::from_loaded(
            &cfg,
            "/config/biocapital/creatures/variant_zombie/creatures.json",
            1_700_000_000,
            12345,
            chrono::Utc::now(),
        )
        .expect("must serialise");
        assert_eq!(rec.creature_id, "variant_zombie");
        assert_eq!(rec.display_name_zh.as_deref(), Some("变体僵尸"));
        assert_eq!(rec.display_name_en.as_deref(), Some("Variant Zombie"));
        assert!(rec.enabled);
        assert_eq!(rec.reload_failed_count, 0);
        assert_eq!(rec.source_mtime, 1_700_000_000);
        assert_eq!(rec.last_loaded_tick, 12345);
    }

    #[test]
    fn audit_entry_helpers_populate_op_and_notes() {
        let e = CreatureAuditEntry::reload_all(
            None,
            "RUST_SERVICE",
            999,
            None,
            serde_json::json!({"files_scanned": 7, "reloaded": 3}),
        );
        assert_eq!(e.op, "creature.reload_all");
        assert!(e.before_json.is_none());
        assert!(e.after_json.is_none());
        assert_eq!(e.notes.unwrap()["files_scanned"], 7);

        let e = CreatureAuditEntry::reload_failed(
            None,
            "RUST_SERVICE",
            "variant_zombie",
            1000,
            None,
            "JSON parse error at line 7",
        );
        assert_eq!(e.op, "creature.reload_failed");
        assert_eq!(
            e.target_creature_id.as_deref(),
            Some("variant_zombie")
        );
        assert_eq!(e.notes.unwrap()["error"], "JSON parse error at line 7");
    }
}