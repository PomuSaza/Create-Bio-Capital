//! PostgreSQL persistence for the hardware-token subsystem
//! (`doc/18-tg-whitelist.md` §5 + `doc/99-integration-matrix.md` §5).
//!
//! Companion to the domain types in
//! `biocapital_bank::domain::hardware_token`. This module is the
//! only piece of code that talks to `hardware_tokens` and
//! `audit_hardware_token`.
//!
//! Public surface:
//! - [`HardwareTokenRepository`] — trait the gRPC service depends
//!   on; tests can substitute an in-memory implementation.
//! - [`PgHardwareTokenRepository`] — production implementation
//!   backed by `sqlx::PgPool`.
//! - [`HardwareTokenAuditWriter`] — append-only writer for
//!   `audit_hardware_token`, wired into the gRPC service via
//!   [`HardwareTokenServiceDeps`].
//! - [`HardwareTokenRepoError`] — typed error surface; mirrors the
//!   `biocapital-pg::environment::RepoError` shape. The
//!   `tonic::Status` `From` impl is implemented so the gRPC
//!   layer can `?`-bubble errors.
//! - [`HardwareTokenServiceDeps`] — composite handle passed into
//!   the gRPC service (mirrors `BankServiceDeps` /
//!   `EnvironmentServiceDeps`).
//!
//! FIFO enforcement: the 3-slot cap is enforced by a PG trigger
//! (see `rust/migrations/20260614000009_hardware_token.sql`).
//! The repository's `create_token` is a straight `INSERT`; the
//! trigger evicts the oldest non-terminal row when the cap is
//! reached. The repository does **not** need to count rows for
//! cap enforcement — single source of truth lives in the trigger.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

// `HARDWARE_TOKEN_EXPIRY_DAYS` is only referenced by the test
// module below; `use super::*` re-exports the lib's top-level
// imports, so we keep it here under an explicit `#[allow]`.
#[allow(unused_imports)]
use biocapital_bank::domain::hardware_token::{
    HardwareToken, HardwareTokenError, HardwareTokenStatus, HARDWARE_TOKEN_EXPIRY_DAYS,
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

    #[error("invalid HardwareTokenStatus in column {column}: {value}")]
    InvalidStatus { column: &'static str, value: String },

    #[error("hardware token {token_id} not found for owner {owner_uuid}")]
    TokenNotFound { token_id: Uuid, owner_uuid: Uuid },
}

/// Re-export alias so callers can disambiguate from
/// `bank::RepoError` / `player_state::RepoError` /
/// `environment::RepoError`. The enums are kept distinct so the
/// gRPC layer can `From`-convert each to the right tonic status
/// without an extra match arm.
pub type HardwareTokenRepoError = RepoError;

impl From<RepoError> for tonic::Status {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::Sqlx(sqlx::Error::RowNotFound) => {
                tonic::Status::not_found("hardware_token row not found")
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
            RepoError::InvalidStatus { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid HardwareTokenStatus in {column}: {value}"
                ))
            }
            RepoError::TokenNotFound { token_id, owner_uuid } => {
                tonic::Status::not_found(format!(
                    "hardware token {token_id} not found for owner {owner_uuid}"
                ))
            }
        }
    }
}

impl From<HardwareTokenError> for RepoError {
    fn from(e: HardwareTokenError) -> Self {
        match e {
            HardwareTokenError::TokenNotFound { token_id, owner_uuid } => {
                RepoError::TokenNotFound { token_id, owner_uuid }
            }
            HardwareTokenError::InvalidHardwareIdHash(_)
            | HardwareTokenError::ExpiresAtNotAfterIssued { .. }
            | HardwareTokenError::NotBindable { .. } => {
                // These are domain-validation errors; the gRPC
                // layer maps them to `invalid_argument` directly.
                // For the repository we surface as a generic
                // validation error so the gRPC layer can keep its
                // single `?` bubble.
                RepoError::Sqlx(sqlx::Error::Protocol(format!(
                    "domain validation: {e}"
                )))
            }
            HardwareTokenError::InvalidStatus(s) => RepoError::InvalidStatus {
                column: "status",
                value: s,
            },
        }
    }
}

// ── Audit writer payload ────────────────────────────────────────────────────

/// Append-only payload for `audit_hardware_token` (18 §9.2 +
/// 99 §2.2 relaxed-form contract). The full set of 9 ops:
/// `token.request` / `token.bind` / `token.expire` / `token.revoke` /
/// `token.replace` / `token.fifo_evict` / `auth.whitelist_pass` /
/// `auth.hardware_pass` / `auth.deny`.
#[derive(Debug, Clone)]
pub struct HardwareTokenAuditEntry {
    pub log_id: Uuid,
    pub actor_uuid: Option<Uuid>,
    pub actor_type: &'static str,
    pub target_token_id: Option<Uuid>,
    pub target_owner_uuid: Option<Uuid>,
    pub op: &'static str,
    pub hardware_id_hash: Option<String>,
    pub reason: Option<String>,
    pub tick_millis: i64,
    pub request_id: Option<Uuid>,
}

#[async_trait]
pub trait HardwareTokenAuditWriter: Send + Sync {
    async fn write(&self, entry: HardwareTokenAuditEntry) -> Result<(), RepoError>;
}

// ── Repository trait ────────────────────────────────────────────────────────

#[async_trait]
pub trait HardwareTokenRepository: Send + Sync {
    /// Insert a new ACTIVE token for `owner`. The PG trigger
    /// enforces the 3-slot FIFO cap by flipping the oldest
    /// non-terminal row to REPLACED when the cap is reached.
    /// `expires_at` is computed by the caller (= `issued_at +
    /// 30d`); the repository performs no time arithmetic.
    async fn create_token(
        &self,
        owner: Uuid,
        token_id: Uuid,
        issued_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<HardwareToken, RepoError>;

    /// Bind `hardware_id_hash` to `token_id`. The token must be
    /// in `Active` state and must belong to `owner`. On success
    /// the row is flipped to `Bound` and `bound_at = NOW()`.
    async fn bind_hardware(
        &self,
        token_id: Uuid,
        owner: Uuid,
        hardware_id_hash: String,
    ) -> Result<HardwareToken, RepoError>;

    /// Mark `token_id` as `Revoked` (player or admin-initiated
    /// `RevokeHardware` RPC). The token must belong to `owner`.
    async fn revoke_token(
        &self,
        token_id: Uuid,
        owner: Uuid,
        revoked_at: DateTime<Utc>,
    ) -> Result<HardwareToken, RepoError>;

    /// List all tokens for `owner`, newest first. When
    /// `include_replaced` is `false`, terminal rows (REPLACED /
    /// REVOKED / EXPIRED) are filtered out — the gRPC
    /// `ListHardware` RPC passes `true` (so the Web UI sees
    /// history) and the `Authenticate` hot path passes
    /// `false`.
    async fn list_for_owner(
        &self,
        owner: Uuid,
        include_replaced: bool,
    ) -> Result<Vec<HardwareToken>, RepoError>;

    /// Find the (single) BOUND, non-expired token for `owner`
    /// whose `hardware_id_hash` matches. The gRPC `Authenticate`
    /// hot path calls this once per login. Returns `Ok(None)` if
    /// no match exists; the gRPC layer then falls through to the
    /// "no_hardware_token" / "hardware_id_mismatch" branch
    /// (18 §3.1).
    async fn find_valid_token(
        &self,
        owner: Uuid,
        hardware_id_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<HardwareToken>, RepoError>;

    /// Sweep: mark any `ACTIVE` or `BOUND` rows whose
    /// `expires_at < now` as `EXPIRED`. The gRPC layer schedules
    /// this as a periodic task (or the Rust CLI runs it on
    /// startup). Returns the number of rows updated.
    async fn expire_overdue(&self, now: DateTime<Utc>) -> Result<u64, RepoError>;
}

// ── Postgres implementation ────────────────────────────────────────────────

#[derive(Clone)]
pub struct PgHardwareTokenRepository {
    pool: PgPool,
}

impl PgHardwareTokenRepository {
    pub async fn connect(database_url: &str) -> Result<Self, RepoError> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
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

fn row_to_token(row: &sqlx::postgres::PgRow) -> Result<HardwareToken, RepoError> {
    let token_id: Uuid = row.try_get("token_id").map_err(|e| RepoError::InvalidUuid {
        column: "token_id",
        value: e.to_string(),
    })?;
    let owner_uuid: Uuid = row.try_get("owner_uuid").map_err(|e| RepoError::InvalidUuid {
        column: "owner_uuid",
        value: e.to_string(),
    })?;
    let hardware_id_hash: Option<String> = row.try_get("hardware_id_hash").ok();
    let status_wire: String = row.try_get("status").map_err(|e| RepoError::InvalidStatus {
        column: "status",
        value: e.to_string(),
    })?;
    let status = HardwareTokenStatus::from_wire(&status_wire).ok_or_else(|| {
        RepoError::InvalidStatus {
            column: "status",
            value: status_wire.clone(),
        }
    })?;
    let issued_at: DateTime<Utc> = row
        .try_get("issued_at")
        .map_err(|e| RepoError::Sqlx(e))?;
    let expires_at: DateTime<Utc> = row
        .try_get("expires_at")
        .map_err(|e| RepoError::Sqlx(e))?;
    let bound_at: Option<DateTime<Utc>> = row.try_get("bound_at").ok();
    let replaced_at: Option<DateTime<Utc>> = row.try_get("replaced_at").ok();
    let revoked_at: Option<DateTime<Utc>> = row.try_get("revoked_at").ok();

    Ok(HardwareToken {
        token_id,
        owner_uuid,
        hardware_id_hash,
        status,
        issued_at,
        expires_at,
        bound_at,
        replaced_at,
        revoked_at,
    })
}

#[async_trait]
impl HardwareTokenRepository for PgHardwareTokenRepository {
    async fn create_token(
        &self,
        owner: Uuid,
        token_id: Uuid,
        issued_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<HardwareToken, RepoError> {
        // Defensive: don't insert rows whose expiry is not after
        // the issue time. The domain layer already enforces
        // this; we double-check here as a SQL contract guard.
        if expires_at <= issued_at {
            return Err(RepoError::Sqlx(sqlx::Error::Protocol(format!(
                "expires_at ({expires_at}) must be > issued_at ({issued_at})"
            ))));
        }

        // The PG trigger fires here. If the owner already has 3
        // non-terminal tokens, the trigger flips the oldest to
        // REPLACED before this INSERT completes.
        let row = sqlx::query(
            r#"
            INSERT INTO hardware_tokens
                   (token_id, owner_uuid, status, issued_at, expires_at)
            VALUES ($1, $2, 'ACTIVE', $3, $4)
            RETURNING token_id, owner_uuid, hardware_id_hash, status,
                      issued_at, expires_at, bound_at, replaced_at, revoked_at
            "#,
        )
        .bind(token_id)
        .bind(owner)
        .bind(issued_at)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await?;

        row_to_token(&row)
    }

    async fn bind_hardware(
        &self,
        token_id: Uuid,
        owner: Uuid,
        hardware_id_hash: String,
    ) -> Result<HardwareToken, RepoError> {
        // Validate the 52-char base32 form (18 §4.2). The
        // repository enforces the wire contract; the gRPC
        // layer does the same check earlier.
        if hardware_id_hash.len() != 52 {
            return Err(RepoError::Sqlx(sqlx::Error::Protocol(format!(
                "hardware_id_hash must be 52 chars, got {}",
                hardware_id_hash.len()
            ))));
        }

        // Atomic update: flip ACTIVE → BOUND only. Any other
        // status (already bound / expired / replaced / revoked)
        // is a 0-row UPDATE → surfaced as TokenNotFound so the
        // gRPC layer's "reason" mapping is consistent.
        let row_opt = sqlx::query(
            r#"
            UPDATE hardware_tokens
               SET status = 'BOUND',
                   bound_at = NOW(),
                   hardware_id_hash = $3
             WHERE token_id = $1
               AND owner_uuid = $2
               AND status = 'ACTIVE'
            RETURNING token_id, owner_uuid, hardware_id_hash, status,
                      issued_at, expires_at, bound_at, replaced_at, revoked_at
            "#,
        )
        .bind(token_id)
        .bind(owner)
        .bind(&hardware_id_hash)
        .fetch_optional(&self.pool)
        .await?;

        match row_opt {
            Some(row) => row_to_token(&row),
            None => {
                // Disambiguate "token not found" from "token in
                // wrong state" by reading the row without the
                // status filter.
                let existing = sqlx::query(
                    r#"
                    SELECT token_id, owner_uuid, hardware_id_hash, status,
                           issued_at, expires_at, bound_at, replaced_at, revoked_at
                      FROM hardware_tokens
                     WHERE token_id = $1 AND owner_uuid = $2
                    "#,
                )
                .bind(token_id)
                .bind(owner)
                .fetch_optional(&self.pool)
                .await?;
                match existing {
                    None => Err(RepoError::TokenNotFound { token_id, owner_uuid: owner }),
                    Some(row) => {
                        let t = row_to_token(&row)?;
                        // The gRPC layer maps `NotBindable` to
                        // `failed_precondition` so the Java side
                        // gets a clean "this token can't be bound
                        // right now" message.
                        Err(RepoError::Sqlx(sqlx::Error::Protocol(format!(
                            "token {} is in non-bindable state {}",
                            t.token_id,
                            t.status.as_str()
                        ))))
                    }
                }
            }
        }
    }

    async fn revoke_token(
        &self,
        token_id: Uuid,
        owner: Uuid,
        revoked_at: DateTime<Utc>,
    ) -> Result<HardwareToken, RepoError> {
        let row_opt = sqlx::query(
            r#"
            UPDATE hardware_tokens
               SET status = 'REVOKED',
                   revoked_at = $3
             WHERE token_id = $1
               AND owner_uuid = $2
               AND status NOT IN ('REVOKED', 'REPLACED', 'EXPIRED')
            RETURNING token_id, owner_uuid, hardware_id_hash, status,
                      issued_at, expires_at, bound_at, replaced_at, revoked_at
            "#,
        )
        .bind(token_id)
        .bind(owner)
        .bind(revoked_at)
        .fetch_optional(&self.pool)
        .await?;

        match row_opt {
            Some(row) => row_to_token(&row),
            None => Err(RepoError::TokenNotFound { token_id, owner_uuid: owner }),
        }
    }

    async fn list_for_owner(
        &self,
        owner: Uuid,
        include_replaced: bool,
    ) -> Result<Vec<HardwareToken>, RepoError> {
        let rows = if include_replaced {
            sqlx::query(
                r#"
                SELECT token_id, owner_uuid, hardware_id_hash, status,
                       issued_at, expires_at, bound_at, replaced_at, revoked_at
                  FROM hardware_tokens
                 WHERE owner_uuid = $1
                 ORDER BY issued_at DESC
                "#,
            )
            .bind(owner)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT token_id, owner_uuid, hardware_id_hash, status,
                       issued_at, expires_at, bound_at, replaced_at, revoked_at
                  FROM hardware_tokens
                 WHERE owner_uuid = $1
                   AND status NOT IN ('REPLACED', 'REVOKED', 'EXPIRED')
                 ORDER BY issued_at DESC
                "#,
            )
            .bind(owner)
            .fetch_all(&self.pool)
            .await?
        };
        rows.iter().map(row_to_token).collect()
    }

    async fn find_valid_token(
        &self,
        owner: Uuid,
        hardware_id_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<HardwareToken>, RepoError> {
        // The query is the 18 §3.1 hot path:
        //   owner = $1 AND status = 'BOUND'
        //   AND expires_at > $3
        //   AND hardware_id_hash = $2
        // The partial UNIQUE index
        // `idx_hardware_tokens_owner_hash_bound` on
        // (owner, hash) WHERE status = 'BOUND' keeps the
        // predicate tight.
        let row_opt = sqlx::query(
            r#"
            SELECT token_id, owner_uuid, hardware_id_hash, status,
                   issued_at, expires_at, bound_at, replaced_at, revoked_at
              FROM hardware_tokens
             WHERE owner_uuid = $1
               AND status = 'BOUND'
               AND hardware_id_hash = $2
               AND expires_at > $3
             ORDER BY issued_at DESC
             LIMIT 1
            "#,
        )
        .bind(owner)
        .bind(hardware_id_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        match row_opt {
            None => Ok(None),
            Some(row) => Ok(Some(row_to_token(&row)?)),
        }
    }

    async fn expire_overdue(&self, now: DateTime<Utc>) -> Result<u64, RepoError> {
        // Bulk update: flip every ACTIVE/BOUND row whose
        // `expires_at < now` to EXPIRED. We intentionally do
        // NOT set `replaced_at` / `revoked_at` (those are
        // orthogonal to expiry). The result count feeds the
        // periodic sweep metric; the gRPC layer logs it.
        let result = sqlx::query(
            r#"
            UPDATE hardware_tokens
               SET status = 'EXPIRED'
             WHERE status IN ('ACTIVE', 'BOUND')
               AND expires_at <= $1
            "#,
        )
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

// ── Audit writer ────────────────────────────────────────────────────────────

pub struct PgHardwareTokenAuditWriter {
    pool: PgPool,
}

impl PgHardwareTokenAuditWriter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl HardwareTokenAuditWriter for PgHardwareTokenAuditWriter {
    async fn write(&self, entry: HardwareTokenAuditEntry) -> Result<(), RepoError> {
        sqlx::query(
            r#"
            INSERT INTO audit_hardware_token (
                log_id, actor_uuid, actor_type,
                target_token_id, target_owner_uuid,
                op, hardware_id_hash, reason,
                tick_millis, request_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(entry.log_id)
        .bind(entry.actor_uuid)
        .bind(entry.actor_type)
        .bind(entry.target_token_id)
        .bind(entry.target_owner_uuid)
        .bind(entry.op)
        .bind(entry.hardware_id_hash.as_deref())
        .bind(entry.reason.as_deref())
        .bind(entry.tick_millis)
        .bind(entry.request_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// ── Composite service deps ─────────────────────────────────────────────────

#[derive(Clone)]
pub struct HardwareTokenServiceDeps {
    pub repo: std::sync::Arc<dyn HardwareTokenRepository>,
    pub audit: std::sync::Arc<dyn HardwareTokenAuditWriter>,
}

impl HardwareTokenServiceDeps {
    pub fn new(
        repo: std::sync::Arc<dyn HardwareTokenRepository>,
        audit: std::sync::Arc<dyn HardwareTokenAuditWriter>,
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
        fn _assert_object_safe(
            _: std::sync::Arc<dyn HardwareTokenRepository>,
        ) {
        }
        fn _assert_audit_object_safe(
            _: std::sync::Arc<dyn HardwareTokenAuditWriter>,
        ) {
        }
    }

    #[test]
    fn expiry_days_constant_is_pinned_to_thirty() {
        // Pinned to 30 by 18 §1.2; widening is a wire-incompatible
        // change.
        assert_eq!(HARDWARE_TOKEN_EXPIRY_DAYS, 30);
    }
}
