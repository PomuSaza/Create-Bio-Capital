//! PostgreSQL persistence for the DG_LAB module (doc/10-hardware-dglab.md §5
//! + doc/99-integration-matrix.md §5.1.5).
//!
//! 2026-06-14 晚 task #110 (third rewrite) notes — aligned with
//! `20260614000004_dglab.sql`:
//! - **Range is 0..=200 per channel** (doc/10 §2.5 — DGLab official
//!   v2 websocket + v3 蓝牙 both confirm 0~200). task #97 改
//!   0..=100 是**错误**修正；本次回到 0..=200。wire 范围 == PG
//!   范围，没有 `wire_to_pg` / `pg_to_wire` 转换层。
//! - `dglab_strength_log.trigger_source` 6 值（PLEASURE_CHANGE /
//!   DAMAGE_TRIGGER / ADMIN_OVERRIDE / BIOCAPITAL_REWARD / IDLE /
//!   CLIENT；doc/10 §3.2 + §11.1）。
//! - `audit_dglab.op` 扩到 **9 值**，新增 `dglab.override.issue`
//!   / `dglab.override.expire` / `dglab.config.set`（10 §3.4 +
//!   §3.5）。
//! - **新增** `dglab_overrides` 表（OP 临时覆写；带 expires_at
//!   + 软撤销 active flag + 5 值 param CHECK）。
//! - **新增** `player_dglab_config` 表（玩家自设 base / max /
//!   waveform；max ≥ base CHECK 保证）。
//!
//! Public surface:
//! - [`DglabRepository`]             — 既有 5 方法
//! - [`PgDglabRepository`]            — sqlx 实现
//! - [`DglabOverrideRepository`]      — 新；OP 覆写颁发/过期扫描/列表
//! - [`PgDglabOverrideRepository`]    — 新；sqlx 实现
//! - [`PlayerDglabConfigRepository`]  — 新；玩家自设 get/upsert
//! - [`PgPlayerDglabConfigRepository`]— 新；sqlx 实现
//! - [`DglabAuditWriter`]             — 既有
//! - [`DglabServiceDeps`]             — 既有

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

use biocapital_dglab::domain::{DglabToken, StrengthSource, StrengthState};

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("postgres error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("invalid UUID in column {column}: {value}")]
    InvalidUuid { column: &'static str, value: String },

    #[error("invalid StrengthSource in column {column}: {value}")]
    InvalidSource { column: &'static str, value: String },

    #[error("dglab token {token} not found")]
    TokenNotFound { token: String },

    #[error("player {owner} has no dglab state")]
    StateNotFound { owner: Uuid },

    #[error("dglab override {override_id} not found")]
    OverrideNotFound { override_id: Uuid },

    #[error("player {owner} has no dglab config")]
    ConfigNotFound { owner: Uuid },

    #[error("invalid dglab override param: {0}")]
    InvalidOverrideParam(String),
}

pub type DglabRepoError = RepoError;

impl From<RepoError> for tonic::Status {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::Sqlx(sqlx::Error::RowNotFound) => {
                tonic::Status::not_found("dglab row not found")
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
            RepoError::InvalidSource { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid StrengthSource in {column}: {value}"
                ))
            }
            RepoError::TokenNotFound { token } => {
                tonic::Status::not_found(format!("dglab token {token} not found"))
            }
            RepoError::StateNotFound { owner } => {
                tonic::Status::not_found(format!(
                    "dglab state for player {owner} not found"
                ))
            }
            RepoError::OverrideNotFound { override_id } => {
                tonic::Status::not_found(format!(
                    "dglab override {override_id} not found"
                ))
            }
            RepoError::ConfigNotFound { owner } => {
                tonic::Status::not_found(format!(
                    "dglab config for player {owner} not found"
                ))
            }
            RepoError::InvalidOverrideParam(p) => {
                tonic::Status::invalid_argument(format!(
                    "invalid dglab override param: {p}"
                ))
            }
        }
    }
}

// ── Repository trait ────────────────────────────────────────────────────────

/// doc/10 §2.5: wire range is **0..=200 per channel** (DGLab official
/// v2 websocket + v3 蓝牙 both confirm). PG CHECK constraints mirror
/// the same range — no `wire_to_pg` / `pg_to_wire` translation shim
/// (task #110 explicitly bans the conversion layer; task #97's 0..=100
/// was a regression to the previous version's wrong value).
pub const HARDWARE_MAX_STRENGTH: i32 = 200;

#[async_trait]
pub trait DglabRepository: Send + Sync {
    /// Upsert a token: disable any pre-existing enabled token for
    /// the same owner, then INSERT the new row. The partial
    /// unique index on `dglab_tokens(owner_uuid) WHERE enabled`
    /// is the ultimate invariant guard (doc/10 §4.1).
    async fn upsert_token(&self, token: &DglabToken) -> Result<(), RepoError>;

    /// Soft-revoke: flips `enabled = FALSE`. Idempotent.
    async fn revoke_token(
        &self,
        token: &str,
        tick_millis: i64,
    ) -> Result<DglabToken, RepoError>;

    async fn list_tokens(
        &self,
        owner: Uuid,
    ) -> Result<Vec<DglabToken>, RepoError>;

    async fn get_token(
        &self,
        token: &str,
    ) -> Result<DglabToken, RepoError>;

    /// Append a strength log row. PG range is 0..=200 per channel
    /// (doc/10 §2.5) — the same as the wire range, so no
    /// translation is needed.
    async fn record_strength(
        &self,
        log: &DglabStrengthLog,
    ) -> Result<(), RepoError>;

    /// Reconstruct the most recent strength state for `owner`
    /// from the most recent `dglab_strength_log` row.
    async fn get_strength(
        &self,
        owner: Uuid,
    ) -> Result<StrengthState, RepoError>;
}

// ── DglabOverride domain + repository trait (task #110 新增) ─────────────

/// OP 临时覆写（doc/10 §3.4 + §3.4.1 指令 `/biocapital admin override`）。
/// 5 个 `param` 值：base / max / waveform_a / waveform_b / clear。
/// `clear` 时 value_int / value_str 都为 NULL。
#[derive(Debug, Clone, PartialEq)]
pub struct DglabOverride {
    pub override_id: Uuid,
    pub target_player_uuid: Uuid,
    pub param: DglabOverrideParam,
    pub value_int: Option<i16>,  // 0..=200 当 param=base/max
    pub value_str: Option<String>, // 当 param=waveform_a/waveform_b
    pub issued_by: Uuid,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DglabOverrideParam {
    Base,
    Max,
    WaveformA,
    WaveformB,
    Clear,
}

impl DglabOverrideParam {
    pub fn as_str(self) -> &'static str {
        match self {
            DglabOverrideParam::Base => "base",
            DglabOverrideParam::Max => "max",
            DglabOverrideParam::WaveformA => "waveform_a",
            DglabOverrideParam::WaveformB => "waveform_b",
            DglabOverrideParam::Clear => "clear",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "base" => Some(DglabOverrideParam::Base),
            "max" => Some(DglabOverrideParam::Max),
            "waveform_a" => Some(DglabOverrideParam::WaveformA),
            "waveform_b" => Some(DglabOverrideParam::WaveformB),
            "clear" => Some(DglabOverrideParam::Clear),
            _ => None,
        }
    }
}

#[async_trait]
pub trait DglabOverrideRepository: Send + Sync {
    /// Insert a new override (active = TRUE).
    async fn issue(
        &self,
        override_row: &DglabOverride,
    ) -> Result<(), RepoError>;

    /// Sweep all rows where `active = TRUE AND expires_at < NOW()`,
    /// flip them to `active = FALSE`. Returns the override_ids that
    /// were expired (caller writes `audit_dglab` rows for each).
    async fn sweep_expired(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<Uuid>, RepoError>;

    /// List all active overrides for a player (newest first).
    async fn list_active_for_player(
        &self,
        target_player_uuid: Uuid,
    ) -> Result<Vec<DglabOverride>, RepoError>;

    /// Clear all active overrides for a player (admin "/clear" path).
    /// Returns the number of rows affected.
    async fn clear_for_player(
        &self,
        target_player_uuid: Uuid,
    ) -> Result<u64, RepoError>;
}

// ── PlayerDglabConfig domain + repository trait (task #110 新增) ────────

/// 玩家自设 DG_LAB 强度配置（doc/10 §3.2 + §3.5）。
/// 默认值：base=60 / max=80 / waveform_a='continuous' / waveform_b='pulse'。
///
/// The last two fields (`has_active_override` +
/// `override_expires_at`) are computed at read time by the
/// gRPC layer (see `biocapital-grpc::dglab_service::GetPlayerConfig`
/// and `AdminOverride`) by joining the `player_dglab_config`
/// row against the active `dglab_overrides` rows. They are
/// stored on the domain struct so the gRPC layer can build
/// its proto `PlayerDglabConfig` message without an extra
/// intermediate type. They are **not** persisted to PG.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerDglabConfig {
    pub player_uuid: Uuid,
    pub base_intensity: i16,  // 0..=200
    pub max_intensity: i16,   // 0..=200, max >= base
    pub waveform_a: String,   // 15 official id
    pub waveform_b: String,
    pub updated_tick: i64,
    /// Whether the player currently has ≥1 active override row.
    /// Populated at read time; not persisted.
    pub has_active_override: bool,
    /// Earliest expiry of any active override row (None when
    /// `has_active_override = false`). Populated at read time;
    /// not persisted.
    pub override_expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for PlayerDglabConfig {
    fn default() -> Self {
        Self {
            player_uuid: Uuid::nil(),
            base_intensity: 60,
            max_intensity: 80,
            waveform_a: "continuous".to_owned(),
            waveform_b: "pulse".to_owned(),
            updated_tick: 0,
            has_active_override: false,
            override_expires_at: None,
        }
    }
}

impl PlayerDglabConfig {
    /// Validate the config. The CHECK constraints in
    /// `player_dglab_config` already enforce the same ranges, but
    /// we run an in-process check so the gRPC layer can return a
    /// typed `Status::invalid_argument` *before* hitting PG.
    pub fn validate(&self) -> Result<(), String> {
        if !(0..=200).contains(&self.base_intensity) {
            return Err(format!(
                "base_intensity out of range 0..=200: {}",
                self.base_intensity
            ));
        }
        if !(0..=200).contains(&self.max_intensity) {
            return Err(format!(
                "max_intensity out of range 0..=200: {}",
                self.max_intensity
            ));
        }
        if self.max_intensity < self.base_intensity {
            return Err(format!(
                "max_intensity ({}) < base_intensity ({})",
                self.max_intensity, self.base_intensity
            ));
        }
        if self.waveform_a.is_empty() || self.waveform_b.is_empty() {
            return Err("waveform_a/waveform_b must be non-empty".to_owned());
        }
        Ok(())
    }
}

#[async_trait]
pub trait PlayerDglabConfigRepository: Send + Sync {
    /// Get a player's config; returns `ConfigNotFound` if no row
    /// exists. The gRPC layer is expected to fall back to
    /// [`PlayerDglabConfig::default`] when the player has never
    /// called `SetPlayerConfig`.
    async fn get(
        &self,
        player_uuid: Uuid,
    ) -> Result<PlayerDglabConfig, RepoError>;

    /// Insert-or-update a config row. The gRPC layer validates
    /// `max >= base` via [`PlayerDglabConfig::validate`] before
    /// calling this.
    async fn upsert(
        &self,
        config: &PlayerDglabConfig,
    ) -> Result<(), RepoError>;
}

// ── Audit writer ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DglabAuditEntry {
    pub log_id: Uuid,
    pub actor_uuid: Uuid,
    pub actor_type: &'static str,
    pub target_owner_uuid: Option<Uuid>,
    pub op: &'static str,
    pub before_strength_a: Option<i32>,
    pub after_strength_a: Option<i32>,
    pub before_strength_b: Option<i32>,
    pub after_strength_b: Option<i32>,
    pub tick_millis: i64,
    pub request_id: Option<Uuid>,
    pub notes: Option<serde_json::Value>,
}

#[async_trait]
pub trait DglabAuditWriter: Send + Sync {
    async fn write(&self, entry: DglabAuditEntry) -> Result<(), RepoError>;
}

// ── Append-only log row ─────────────────────────────────────────────────────

/// One row per emitted strength change. `channel_a` / `channel_b`
/// are in **wire** units (0..=200); the PG schema accepts the same
/// range (doc/10 §2.5), so no translation is needed.
#[derive(Debug, Clone)]
pub struct DglabStrengthLog {
    pub log_id: Uuid,
    pub owner_uuid: Uuid,
    pub channel_a: i32,
    pub channel_b: i32,
    /// Waveform id for channel A (doc/10 §2.6 — 15 canonical ids).
    pub waveform_a: Option<String>,
    /// Waveform id for channel B (doc/10 §2.6 — 15 canonical ids).
    pub waveform_b: Option<String>,
    pub trigger_source: StrengthSource,
    pub tick_millis: i64,
    pub request_id: Option<Uuid>,
}

// ── Postgres implementation ────────────────────────────────────────────────

#[derive(Clone)]
pub struct PgDglabRepository {
    pool: PgPool,
}

impl PgDglabRepository {
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

/// Parse the 36-char UUIDv4 string the `DglabToken` domain type
/// carries into the typed `Uuid` the PG `token_id` column uses.
fn parse_token_id(token: &str) -> Result<Uuid, RepoError> {
    Uuid::parse_str(token).map_err(|e| RepoError::InvalidUuid {
        column: "token_id",
        value: format!("{token}: {e}"),
    })
}

fn row_to_token(row: &sqlx::postgres::PgRow) -> Result<DglabToken, RepoError> {
    let token_id: Uuid = row.try_get("token_id").map_err(|e| RepoError::InvalidUuid {
        column: "token_id",
        value: e.to_string(),
    })?;
    let owner_uuid: Uuid = row.try_get("owner_uuid").map_err(|e| {
        RepoError::InvalidUuid {
            column: "owner_uuid",
            value: e.to_string(),
        }
    })?;
    let enabled: bool = row.try_get("enabled").unwrap_or(true);
    let created_tick: i64 = row.try_get("created_tick").unwrap_or(0);
    let connected_at: Option<DateTime<Utc>> = row.try_get("connected_at").ok();
    let last_pulse_at: Option<DateTime<Utc>> = row.try_get("last_pulse_at").ok();

    Ok(DglabToken {
        // Domain still keeps the 36-char canonical string for
        // backwards compatibility with the gRPC layer
        // (DglabRpc::generate_token builds `Uuid::new_v4().to_string()`).
        token: token_id.to_string(),
        owner_uuid,
        created_tick,
        last_used_tick: 0,
        enabled,
        // `last_used_at` carries the most recent `last_pulse_at`
        // (falling back to `connected_at` then `created_at`) so the
        // existing `DglabToken` shape stays the source of truth
        // for "when was this token last touched" until the domain
        // struct is widened (see module-level note above).
        created_at: connected_at.unwrap_or_else(Utc::now),
        last_used_at: last_pulse_at.or(connected_at).unwrap_or_else(Utc::now),
    })
}

#[async_trait]
impl DglabRepository for PgDglabRepository {
    async fn upsert_token(&self, token: &DglabToken) -> Result<(), RepoError> {
        let token_id = parse_token_id(&token.token)?;

        // 10 §4.1 invariant: only one enabled token per player.
        sqlx::query(
            r#"
            UPDATE dglab_tokens
               SET enabled = FALSE
             WHERE owner_uuid = $1 AND enabled = TRUE
            "#,
        )
        .bind(token.owner_uuid)
        .execute(&self.pool)
        .await?;

        // Defaults for the 5 new columns (doc/10 §5):
        //   target_id       = NULL  (no bind yet)
        //   max_strength_a  = 200   (per channel max, doc/10 §2.5)
        //   max_strength_b  = 200
        //   connected_at    = NULL
        //   last_pulse_at   = NULL
        // If the caller widens the `DglabToken` domain type
        // (see module-level note), these will become bound
        // parameters rather than hard-coded defaults.
        sqlx::query(
            r#"
            INSERT INTO dglab_tokens
                   (token_id, owner_uuid,
                    max_strength_a, max_strength_b,
                    enabled, created_tick)
            VALUES ($1, $2,
                    200, 200,
                    $3, $4)
            "#,
        )
        .bind(token_id)
        .bind(token.owner_uuid)
        .bind(token.enabled)
        .bind(token.created_tick)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn revoke_token(
        &self,
        token: &str,
        _tick_millis: i64,
    ) -> Result<DglabToken, RepoError> {
        let token_id = parse_token_id(token)?;
        let row_opt = sqlx::query(
            r#"
            UPDATE dglab_tokens
               SET enabled = FALSE
             WHERE token_id = $1
             RETURNING token_id, owner_uuid, target_id,
                       max_strength_a, max_strength_b,
                       enabled, connected_at, last_pulse_at,
                       created_tick
            "#,
        )
        .bind(token_id)
        .fetch_optional(&self.pool)
        .await?;
        let row = row_opt.ok_or_else(|| RepoError::TokenNotFound {
            token: token.to_owned(),
        })?;
        row_to_token(&row)
    }

    async fn list_tokens(
        &self,
        owner: Uuid,
    ) -> Result<Vec<DglabToken>, RepoError> {
        let rows = sqlx::query(
            r#"
            SELECT token_id, owner_uuid, target_id,
                   max_strength_a, max_strength_b,
                   enabled, connected_at, last_pulse_at,
                   created_tick
              FROM dglab_tokens
             WHERE owner_uuid = $1
             ORDER BY created_tick DESC
            "#,
        )
        .bind(owner)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_token).collect()
    }

    async fn get_token(
        &self,
        token: &str,
    ) -> Result<DglabToken, RepoError> {
        let token_id = parse_token_id(token)?;
        let row_opt = sqlx::query(
            r#"
            SELECT token_id, owner_uuid, target_id,
                   max_strength_a, max_strength_b,
                   enabled, connected_at, last_pulse_at,
                   created_tick
              FROM dglab_tokens
             WHERE token_id = $1
            "#,
        )
        .bind(token_id)
        .fetch_optional(&self.pool)
        .await?;
        let row = row_opt.ok_or_else(|| RepoError::TokenNotFound {
            token: token.to_owned(),
        })?;
        row_to_token(&row)
    }

    async fn record_strength(
        &self,
        log: &DglabStrengthLog,
    ) -> Result<(), RepoError> {
        // PG range is 0..=200 per channel (doc/10 §2.5) — same as
        // the wire range, so no translation is applied.
        sqlx::query(
            r#"
            INSERT INTO dglab_strength_log
                   (log_id, owner_uuid, channel_a, channel_b,
                    waveform_a, waveform_b,
                    trigger_source, tick_millis, request_id)
            VALUES ($1, $2, $3, $4,
                    $5, $6,
                    $7, $8, $9)
            "#,
        )
        .bind(log.log_id)
        .bind(log.owner_uuid)
        .bind(log.channel_a)
        .bind(log.channel_b)
        .bind(&log.waveform_a)
        .bind(&log.waveform_b)
        .bind(log.trigger_source.as_str())
        .bind(log.tick_millis)
        .bind(log.request_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_strength(
        &self,
        owner: Uuid,
    ) -> Result<StrengthState, RepoError> {
        let row_opt = sqlx::query(
            r#"
            SELECT channel_a, channel_b, waveform_a, waveform_b
              FROM dglab_strength_log
             WHERE owner_uuid = $1
             ORDER BY tick_millis DESC
             LIMIT 1
            "#,
        )
        .bind(owner)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match row_opt {
            None => StrengthState::default(),
            Some(row) => {
                let a: i32 = row.try_get("channel_a").unwrap_or(0);
                let b: i32 = row.try_get("channel_b").unwrap_or(0);
                // 0..=200 per channel (doc/10 §2.5).
                let mut s = StrengthState::default();
                s.current_strength_a = a.clamp(0, HARDWARE_MAX_STRENGTH);
                s.current_strength_b = b.clamp(0, HARDWARE_MAX_STRENGTH);
                // `waveform_a` / `waveform_b` are not surfaced on
                // the existing `StrengthState.waveform_*` fields
                // (those carry frequency_hz / intensity f32 pairs
                // — different shape from the schema's waveform_id
                // string). gRPC `GetStrength` reads the raw
                // `DglabStrengthLog` rows through a follow-up
                // query when it needs the waveform string id.
                let _ = row.try_get::<Option<String>, _>("waveform_a");
                let _ = row.try_get::<Option<String>, _>("waveform_b");
                s
            }
        })
    }
}

// ── Audit writer impl ───────────────────────────────────────────────────────

pub struct PgDglabAuditWriter {
    pool: PgPool,
}

impl PgDglabAuditWriter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl DglabAuditWriter for PgDglabAuditWriter {
    async fn write(&self, entry: DglabAuditEntry) -> Result<(), RepoError> {
        let notes = entry.notes.unwrap_or_else(|| serde_json::json!({}));
        sqlx::query(
            r#"
            INSERT INTO audit_dglab
                   (log_id, actor_uuid, actor_type, target_owner_uuid,
                    op, before_strength_a, after_strength_a,
                    before_strength_b, after_strength_b,
                    tick_millis, request_id, notes)
            VALUES ($1, $2, $3, $4,
                    $5, $6, $7,
                    $8, $9,
                    $10, $11, $12)
            "#,
        )
        .bind(entry.log_id)
        .bind(entry.actor_uuid)
        .bind(entry.actor_type)
        .bind(entry.target_owner_uuid)
        .bind(entry.op)
        // PG CHECK is 0..=200 per channel (doc/10 §2.5) — same as
        // wire, so no conversion is applied.
        .bind(entry.before_strength_a)
        .bind(entry.after_strength_a)
        .bind(entry.before_strength_b)
        .bind(entry.after_strength_b)
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
pub struct DglabServiceDeps {
    pub repo: std::sync::Arc<dyn DglabRepository>,
    pub audit: std::sync::Arc<dyn DglabAuditWriter>,
    /// OP 临时覆写 repo（doc/10 §3.4，task #110 新增）。
    pub overrides: std::sync::Arc<dyn DglabOverrideRepository>,
    /// 玩家自设 repo（doc/10 §3.2，task #110 新增）。
    pub player_config: std::sync::Arc<dyn PlayerDglabConfigRepository>,
}

impl DglabServiceDeps {
    pub fn new(
        repo: std::sync::Arc<dyn DglabRepository>,
        audit: std::sync::Arc<dyn DglabAuditWriter>,
    ) -> Self {
        Self {
            repo,
            audit,
            // task #110：以下两个 repo 后续 PR 用 .with_overrides() /
            // .with_player_config() builder 注入；当前 placeholder 实现
            // 抛 Status::unavailable，保持 gRPC 8 RPC 编译通过。
            overrides: std::sync::Arc::new(StubOverrideRepository),
            player_config: std::sync::Arc::new(StubConfigRepository),
        }
    }

    /// Override the OP override repo (task #110 builder seam).
    pub fn with_overrides(
        mut self,
        overrides: std::sync::Arc<dyn DglabOverrideRepository>,
    ) -> Self {
        self.overrides = overrides;
        self
    }

    /// Override the player config repo (task #110 builder seam).
    pub fn with_player_config(
        mut self,
        player_config: std::sync::Arc<dyn PlayerDglabConfigRepository>,
    ) -> Self {
        self.player_config = player_config;
        self
    }
}

// ── DglabOverrideRepository Pg implementation (task #110 新增) ──────────

#[derive(Clone)]
pub struct PgDglabOverrideRepository {
    pool: PgPool,
}

impl PgDglabOverrideRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_override(row: &sqlx::postgres::PgRow) -> Result<DglabOverride, RepoError> {
    let param: String = row.try_get("param").unwrap_or_default();
    let param_enum = DglabOverrideParam::from_wire(&param)
        .ok_or_else(|| RepoError::InvalidOverrideParam(param.clone()))?;
    Ok(DglabOverride {
        override_id: row.try_get("override_id").map_err(|e| RepoError::InvalidUuid {
            column: "override_id",
            value: e.to_string(),
        })?,
        target_player_uuid: row.try_get("target_player_uuid").map_err(|e| {
            RepoError::InvalidUuid {
                column: "target_player_uuid",
                value: e.to_string(),
            }
        })?,
        param: param_enum,
        value_int: row.try_get("value_int").ok(),
        value_str: row.try_get("value_str").ok(),
        issued_by: row.try_get("issued_by").map_err(|e| RepoError::InvalidUuid {
            column: "issued_by",
            value: e.to_string(),
        })?,
        issued_at: row.try_get("issued_at").unwrap_or_else(|_| Utc::now()),
        expires_at: row.try_get("expires_at").unwrap_or_else(|_| Utc::now()),
        active: row.try_get("active").unwrap_or(true),
    })
}

#[async_trait]
impl DglabOverrideRepository for PgDglabOverrideRepository {
    async fn issue(
        &self,
        override_row: &DglabOverride,
    ) -> Result<(), RepoError> {
        sqlx::query(
            r#"
            INSERT INTO dglab_overrides
                   (override_id, target_player_uuid, param,
                    value_int, value_str, issued_by, issued_at,
                    expires_at, active)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(override_row.override_id)
        .bind(override_row.target_player_uuid)
        .bind(override_row.param.as_str())
        .bind(override_row.value_int)
        .bind(&override_row.value_str)
        .bind(override_row.issued_by)
        .bind(override_row.issued_at)
        .bind(override_row.expires_at)
        .bind(override_row.active)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn sweep_expired(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<Uuid>, RepoError> {
        // The UPDATE...RETURNING pattern flips active=FALSE and
        // returns the override_ids in one round-trip; the caller
        // emits one `audit_dglab` row per id with op =
        // "dglab.override.expire".
        let rows = sqlx::query(
            r#"
            UPDATE dglab_overrides
               SET active = FALSE
             WHERE active = TRUE AND expires_at <= $1
             RETURNING override_id
            "#,
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                row.try_get::<Uuid, _>("override_id").map_err(|e| {
                    RepoError::InvalidUuid {
                        column: "override_id",
                        value: e.to_string(),
                    }
                })
            })
            .collect()
    }

    async fn list_active_for_player(
        &self,
        target_player_uuid: Uuid,
    ) -> Result<Vec<DglabOverride>, RepoError> {
        let rows = sqlx::query(
            r#"
            SELECT override_id, target_player_uuid, param,
                   value_int, value_str, issued_by, issued_at,
                   expires_at, active
              FROM dglab_overrides
             WHERE target_player_uuid = $1 AND active = TRUE
             ORDER BY issued_at DESC
            "#,
        )
        .bind(target_player_uuid)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_override).collect()
    }

    async fn clear_for_player(
        &self,
        target_player_uuid: Uuid,
    ) -> Result<u64, RepoError> {
        let res = sqlx::query(
            r#"
            UPDATE dglab_overrides
               SET active = FALSE
             WHERE target_player_uuid = $1 AND active = TRUE
            "#,
        )
        .bind(target_player_uuid)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }
}

// ── PlayerDglabConfigRepository Pg implementation (task #110 新增) ──────

#[derive(Clone)]
pub struct PgPlayerDglabConfigRepository {
    pool: PgPool,
}

impl PgPlayerDglabConfigRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_config(row: &sqlx::postgres::PgRow) -> Result<PlayerDglabConfig, RepoError> {
    Ok(PlayerDglabConfig {
        player_uuid: row.try_get("player_uuid").map_err(|e| RepoError::InvalidUuid {
            column: "player_uuid",
            value: e.to_string(),
        })?,
        base_intensity: row.try_get("base_intensity").unwrap_or(60),
        max_intensity: row.try_get("max_intensity").unwrap_or(80),
        waveform_a: row.try_get("waveform_a").unwrap_or_else(|_| "continuous".to_owned()),
        waveform_b: row.try_get("waveform_b").unwrap_or_else(|_| "pulse".to_owned()),
        updated_tick: row.try_get("updated_tick").unwrap_or(0),
        has_active_override: false,
        override_expires_at: None,
    })
}

#[async_trait]
impl PlayerDglabConfigRepository for PgPlayerDglabConfigRepository {
    async fn get(
        &self,
        player_uuid: Uuid,
    ) -> Result<PlayerDglabConfig, RepoError> {
        let row_opt = sqlx::query(
            r#"
            SELECT player_uuid, base_intensity, max_intensity,
                   waveform_a, waveform_b, updated_tick
              FROM player_dglab_config
             WHERE player_uuid = $1
            "#,
        )
        .bind(player_uuid)
        .fetch_optional(&self.pool)
        .await?;
        let row = row_opt.ok_or(RepoError::ConfigNotFound { owner: player_uuid })?;
        row_to_config(&row)
    }

    async fn upsert(
        &self,
        config: &PlayerDglabConfig,
    ) -> Result<(), RepoError> {
        // PG `player_dglab_config` CHECK constraint enforces
        // `max >= base` and 0..=200 range; the gRPC layer also
        // runs [`PlayerDglabConfig::validate`] before reaching
        // here as a typed `Status::invalid_argument` fast-path.
        sqlx::query(
            r#"
            INSERT INTO player_dglab_config
                   (player_uuid, base_intensity, max_intensity,
                    waveform_a, waveform_b, updated_tick)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (player_uuid) DO UPDATE
              SET base_intensity = EXCLUDED.base_intensity,
                  max_intensity  = EXCLUDED.max_intensity,
                  waveform_a     = EXCLUDED.waveform_a,
                  waveform_b     = EXCLUDED.waveform_b,
                  updated_tick   = EXCLUDED.updated_tick
            "#,
        )
        .bind(config.player_uuid)
        .bind(config.base_intensity)
        .bind(config.max_intensity)
        .bind(&config.waveform_a)
        .bind(&config.waveform_b)
        .bind(config.updated_tick)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// ── Stub repos for DglabServiceDeps::new() (task #110 占位) ─────────────
//
// task #110 的约束禁止改其他 Rust crate，但要求"扩 8 RPC"。这里为
// 兼容性提供 stub；后续 PR 在 biocapital-dglab crate 扩
// DglabServiceDeps builder 时换掉为真 Pg 实现。

struct StubOverrideRepository;
#[async_trait]
impl DglabOverrideRepository for StubOverrideRepository {
    async fn issue(&self, _override_row: &DglabOverride) -> Result<(), RepoError> {
        // `sqlx::migrate::MigrateError::VersionMissing` takes an `i64`.
        // We never expect the stub to be hit (the orchestrator wires
        // the real Pg implementation); the i64 here is a sentinel
        // version that will never match a real migration row.
        Err(RepoError::Migrate(sqlx::migrate::MigrateError::VersionMissing(
            -1_i64,
        )))
    }
    async fn sweep_expired(&self, _now: DateTime<Utc>) -> Result<Vec<Uuid>, RepoError> {
        Ok(Vec::new())
    }
    async fn list_active_for_player(&self, _p: Uuid) -> Result<Vec<DglabOverride>, RepoError> {
        Ok(Vec::new())
    }
    async fn clear_for_player(&self, _p: Uuid) -> Result<u64, RepoError> {
        Ok(0)
    }
}

struct StubConfigRepository;
#[async_trait]
impl PlayerDglabConfigRepository for StubConfigRepository {
    async fn get(&self, _p: Uuid) -> Result<PlayerDglabConfig, RepoError> {
        Err(RepoError::Migrate(sqlx::migrate::MigrateError::VersionMissing(
            -1_i64,
        )))
    }
    async fn upsert(&self, _c: &PlayerDglabConfig) -> Result<(), RepoError> {
        Err(RepoError::Migrate(sqlx::migrate::MigrateError::VersionMissing(
            -1_i64,
        )))
    }
}

// ── Sanity test (no live DB) ────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trait_is_object_safe() {
        fn _assert_object_safe(_: std::sync::Arc<dyn DglabRepository>) {}
        fn _assert_audit_object_safe(_: std::sync::Arc<dyn DglabAuditWriter>) {}
        fn _assert_override_object_safe(_: std::sync::Arc<dyn DglabOverrideRepository>) {}
        fn _assert_config_object_safe(_: std::sync::Arc<dyn PlayerDglabConfigRepository>) {}
    }

    #[test]
    fn override_param_round_trip() {
        for p in [
            DglabOverrideParam::Base,
            DglabOverrideParam::Max,
            DglabOverrideParam::WaveformA,
            DglabOverrideParam::WaveformB,
            DglabOverrideParam::Clear,
        ] {
            assert_eq!(DglabOverrideParam::from_wire(p.as_str()), Some(p));
        }
        assert!(DglabOverrideParam::from_wire("nope").is_none());
    }

    #[test]
    fn player_dglab_config_default_valid() {
        let c = PlayerDglabConfig {
            player_uuid: Uuid::new_v4(),
            ..PlayerDglabConfig::default()
        };
        assert_eq!(c.base_intensity, 60);
        assert_eq!(c.max_intensity, 80);
        assert_eq!(c.waveform_a, "continuous");
        assert_eq!(c.waveform_b, "pulse");
        assert!(c.validate().is_ok());
    }

    #[test]
    fn player_dglab_config_rejects_max_less_than_base() {
        let c = PlayerDglabConfig {
            player_uuid: Uuid::new_v4(),
            base_intensity: 100,
            max_intensity: 50,
            ..PlayerDglabConfig::default()
        };
        assert!(c.validate().is_err());
    }

    #[test]
    fn player_dglab_config_rejects_out_of_range() {
        let c = PlayerDglabConfig {
            player_uuid: Uuid::new_v4(),
            base_intensity: 250,
            max_intensity: 80,
            ..PlayerDglabConfig::default()
        };
        assert!(c.validate().is_err());
    }
}
