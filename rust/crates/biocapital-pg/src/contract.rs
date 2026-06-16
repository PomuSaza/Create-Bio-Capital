//! PostgreSQL persistence for the contract module (09 §5).
//!
//! Companion to the domain types in
//! `biocapital_contract::domain`. This module is the only
//! piece of code that talks to `contracts` and
//! `contract_payouts`.
//!
//! Public surface:
//! - [`ContractRepository`] — trait the gRPC service depends
//!   on; tests can substitute an in-memory implementation.
//! - [`PgContractRepository`] — production implementation
//!   backed by `sqlx::PgPool`.
//! - [`ContractAuditWriter`] — append-only writer for
//!   `audit_bank` driven by contract payouts (the contract
//!   module re-uses the bank audit table for cross-table
//!   correlation; see 99 §5 + 09 §3.3). Wired through
//!   [`ContractServiceDeps`].
//! - [`RepoError`] — typed error surface returned to callers
//!   (also implements `Into<tonic::Status>` for clean `?`
//!   usage in the gRPC layer).
//!
//! Idempotency: every mutating method is keyed on a
//! caller-supplied UUID. `record_payout` has a unique index
//! on `request_id`; a replayed RPC fails the insert and the
//! gRPC layer turns that into a return-the-prior-row
//! response. The `contracts` lifecycle does **not** carry a
//! `request_id` (each contract has its own UUID and a
//! second proposal of the same id fails the PK).

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

use biocapital_contract::domain::{
    Contract, ContractPayout, ContractStatus, PayoutReason,
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

    #[error("invalid ContractStatus in column {column}: {value}")]
    InvalidStatus { column: &'static str, value: String },

    #[error("invalid PayoutReason in column {column}: {value}")]
    InvalidReason { column: &'static str, value: String },

    #[error("contract {contract_id} not found")]
    ContractNotFound { contract_id: Uuid },
}

/// Re-export alias so callers can disambiguate from
/// `bank::RepoError` / `player_state::RepoError` /
/// `core_pod::RepoError` / `dglab::RepoError`. The five
/// enums are kept distinct so the gRPC service can
/// `From`-convert each to the right tonic status without an
/// extra match arm.
pub type ContractRepoError = RepoError;

impl From<RepoError> for tonic::Status {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::Sqlx(sqlx::Error::RowNotFound) => {
                tonic::Status::not_found("contract row not found")
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
                    "invalid ContractStatus in {column}: {value}"
                ))
            }
            RepoError::InvalidReason { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid PayoutReason in {column}: {value}"
                ))
            }
            RepoError::ContractNotFound { contract_id } => {
                tonic::Status::not_found(format!(
                    "contract {contract_id} not found"
                ))
            }
        }
    }
}

// ── Repository trait ────────────────────────────────────────────────────────

#[async_trait]
pub trait ContractRepository: Send + Sync {
    /// Insert a freshly-built contract (status = `PROPOSED`).
    /// The lifecycle helpers guarantee the row has already been
    /// validated; this method is a pure INSERT and will fail
    /// with a PK violation on duplicate `contract_id`.
    async fn create(&self, contract: &Contract) -> Result<(), RepoError>;

    /// Fetch one contract by primary key. Returns
    /// `RepoError::ContractNotFound` when the row is missing.
    async fn get(&self, id: Uuid) -> Result<Contract, RepoError>;

    /// List contracts with optional filters. When all three
    /// filters are `None` this returns "every contract in the
    /// table" — only safe for admin paths. The gRPC layer
    /// forces at least one filter via the proto's
    /// `player_filter` (it maps to either proposer or
    /// acceptor).
    async fn list(
        &self,
        proposer: Option<Uuid>,
        acceptor: Option<Uuid>,
        status: Option<ContractStatus>,
        limit: i64,
    ) -> Result<Vec<Contract>, RepoError>;

    /// Update an existing contract row. The repository does
    /// **not** re-validate the lifecycle transition — the
    /// caller (lifecycle helpers) is the single source of
    /// truth for the state machine.
    async fn update(&self, contract: &Contract) -> Result<(), RepoError>;

    /// Append a payout row. Idempotent on `request_id` (unique
    /// index). On duplicate, returns the existing row.
    async fn record_payout(
        &self,
        payout: &ContractPayout,
    ) -> Result<ContractPayout, RepoError>;

    /// List all payouts for a contract, newest first.
    async fn list_payouts(
        &self,
        contract_id: Uuid,
    ) -> Result<Vec<ContractPayout>, RepoError>;

    /// Sweep helper for the tokio expiry task. Returns every
    /// contract that is still `PROPOSED` and whose
    /// `expires_tick <= current_tick`. The caller flips them
    /// to `REJECTED` via [`Self::update`].
    async fn list_expired(
        &self,
        current_tick: i64,
    ) -> Result<Vec<Contract>, RepoError>;
}

// ── Audit writer (delegates to audit_bank for cross-table correlation) ─────

/// Append-only audit entry for a contract event. The
/// contract module re-uses `audit_bank` for payout rows
/// because the underlying ledger movement goes through
/// `BankService.Transfer`; the other 3 contract events
/// (`ContractCreatedEvent` / `ContractActivatedEvent` /
/// `ContractTerminatedEvent`) are emitted through the gRPC
/// response's `event_meta` and logged by the Sable JNI bridge
/// via the existing NeoForge event channel (99 §3.1).
#[derive(Debug, Clone)]
pub struct ContractAuditEntry {
    pub log_id: Uuid,
    pub actor_uuid: Uuid,
    pub actor_type: &'static str,
    pub target_account_uuid: Uuid,
    pub op: &'static str,
    pub before_balance: i64,
    pub after_balance: i64,
    pub tick_millis: i64,
    pub request_id: Option<Uuid>,
    pub notes: Option<serde_json::Value>,
}

#[async_trait]
pub trait ContractAuditWriter: Send + Sync {
    async fn write(&self, entry: ContractAuditEntry) -> Result<(), RepoError>;
}

// ── Postgres implementation ────────────────────────────────────────────────

#[derive(Clone)]
pub struct PgContractRepository {
    pool: PgPool,
}

impl PgContractRepository {
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

fn row_to_contract(row: &sqlx::postgres::PgRow) -> Result<Contract, RepoError> {
    let contract_id: Uuid = row.try_get("contract_id").map_err(|e| {
        RepoError::InvalidUuid {
            column: "contract_id",
            value: e.to_string(),
        }
    })?;
    let master_uuid: Uuid = row.try_get("master_uuid").map_err(|e| {
        RepoError::InvalidUuid {
            column: "master_uuid",
            value: e.to_string(),
        }
    })?;
    let slave_uuid: Uuid = row.try_get("slave_uuid").map_err(|e| {
        RepoError::InvalidUuid {
            column: "slave_uuid",
            value: e.to_string(),
        }
    })?;
    let status_wire: String = row.try_get("status").map_err(|e| {
        RepoError::InvalidStatus {
            column: "status",
            value: e.to_string(),
        }
    })?;
    let status = ContractStatus::from_wire(&status_wire).ok_or_else(|| {
        RepoError::InvalidStatus {
            column: "status",
            value: status_wire.clone(),
        }
    })?;
    let terms_type: String = row.try_get("terms_type").unwrap_or_default();
    // terms_json is stored as JSONB; sqlx returns it as
    // `serde_json::Value` when the json feature is on.
    let terms_json: serde_json::Value =
        row.try_get("terms_json").unwrap_or(serde_json::json!({}));
    let terms_json = terms_json.to_string();
    let revenue_share_pct: f32 = row.try_get("revenue_share_pct").unwrap_or(0.0);
    let redemption_cost: i64 = row.try_get("redemption_cost").unwrap_or(0);
    let expires_tick: Option<i64> = row.try_get("expires_tick").ok();
    let created_tick: i64 = row.try_get("created_tick").unwrap_or(0);
    let updated_tick: i64 = row.try_get("updated_tick").unwrap_or(0);
    let activated_tick: Option<i64> = row.try_get("activated_tick").ok();
    let terminated_tick: Option<i64> = row.try_get("terminated_tick").ok();
    let redeemed_tick: Option<i64> = row.try_get("redeemed_tick").ok();
    let reason: Option<String> = row.try_get("reason").ok();

    Ok(Contract {
        contract_id,
        // Wire column `master_uuid` ↔ domain `proposer_uuid`.
        proposer_uuid: master_uuid,
        acceptor_uuid: slave_uuid,
        status,
        terms_type,
        terms_json,
        revenue_share_pct,
        redemption_cost,
        expires_tick,
        created_tick,
        updated_tick,
        activated_tick,
        terminated_tick,
        redeemed_tick,
        reason,
    })
}

fn row_to_payout(row: &sqlx::postgres::PgRow) -> Result<ContractPayout, RepoError> {
    let payout_id: Uuid = row.try_get("payout_id").map_err(|e| {
        RepoError::InvalidUuid {
            column: "payout_id",
            value: e.to_string(),
        }
    })?;
    let contract_id: Uuid = row.try_get("contract_id").map_err(|e| {
        RepoError::InvalidUuid {
            column: "contract_id",
            value: e.to_string(),
        }
    })?;
    let from_account: Uuid = row.try_get("from_account").map_err(|e| {
        RepoError::InvalidUuid {
            column: "from_account",
            value: e.to_string(),
        }
    })?;
    let to_account: Uuid = row.try_get("to_account").map_err(|e| {
        RepoError::InvalidUuid {
            column: "to_account",
            value: e.to_string(),
        }
    })?;
    let amount: i64 = row.try_get("amount").unwrap_or(0).max(0);
    let reason_wire: String = row.try_get("reason").map_err(|e| {
        RepoError::InvalidReason {
            column: "reason",
            value: e.to_string(),
        }
    })?;
    let reason = PayoutReason::from_wire(&reason_wire).ok_or_else(|| {
        RepoError::InvalidReason {
            column: "reason",
            value: reason_wire.clone(),
        }
    })?;
    let tick_millis: i64 = row.try_get("tick_millis").unwrap_or(0);
    let request_id: Uuid = row.try_get("request_id").map_err(|e| {
        RepoError::InvalidUuid {
            column: "request_id",
            value: e.to_string(),
        }
    })?;

    Ok(ContractPayout {
        payout_id,
        contract_id,
        from_account,
        to_account,
        amount,
        reason,
        tick_millis,
        request_id,
    })
}

#[async_trait]
impl ContractRepository for PgContractRepository {
    async fn create(&self, contract: &Contract) -> Result<(), RepoError> {
        // terms_json is a JSONB column; bind as serde_json::Value
        // so sqlx serialises consistently with the read path.
        let terms_json: serde_json::Value = serde_json::from_str(&contract.terms_json)
            .unwrap_or_else(|_| serde_json::json!({}));
        sqlx::query(
            r#"
            INSERT INTO contracts
                   (contract_id, master_uuid, slave_uuid, status,
                    terms_type, terms_json,
                    revenue_share_pct, redemption_cost, expires_tick,
                    created_tick, updated_tick,
                    activated_tick, terminated_tick, redeemed_tick, reason)
            VALUES ($1, $2, $3, $4,
                    $5, $6,
                    $7, $8, $9,
                    $10, $11,
                    $12, $13, $14, $15)
            "#,
        )
        .bind(contract.contract_id)
        .bind(contract.proposer_uuid)
        .bind(contract.acceptor_uuid)
        .bind(contract.status.as_str())
        .bind(&contract.terms_type)
        .bind(&terms_json)
        .bind(contract.revenue_share_pct)
        .bind(contract.redemption_cost)
        .bind(contract.expires_tick)
        .bind(contract.created_tick)
        .bind(contract.updated_tick)
        .bind(contract.activated_tick)
        .bind(contract.terminated_tick)
        .bind(contract.redeemed_tick)
        .bind(contract.reason.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: Uuid) -> Result<Contract, RepoError> {
        let row_opt = sqlx::query(
            r#"
            SELECT contract_id, master_uuid, slave_uuid, status,
                   terms_type, terms_json,
                   revenue_share_pct, redemption_cost, expires_tick,
                   created_tick, updated_tick,
                   activated_tick, terminated_tick, redeemed_tick, reason
              FROM contracts
             WHERE contract_id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let row = row_opt.ok_or(RepoError::ContractNotFound { contract_id: id })?;
        row_to_contract(&row)
    }

    async fn list(
        &self,
        proposer: Option<Uuid>,
        acceptor: Option<Uuid>,
        status: Option<ContractStatus>,
        limit: i64,
    ) -> Result<Vec<Contract>, RepoError> {
        let limit = limit.clamp(1, 1024);
        // Build a dynamic WHERE. The number of bound parameters
        // depends on which filters are set; we hand-roll the SQL
        // rather than fighting sqlx::query_builder here.
        let mut sql = String::from(
            "SELECT contract_id, master_uuid, slave_uuid, status, \
                    terms_type, terms_json, revenue_share_pct, redemption_cost, \
                    expires_tick, created_tick, updated_tick, \
                    activated_tick, terminated_tick, redeemed_tick, reason \
               FROM contracts WHERE TRUE",
        );
        // When both proposer and acceptor are set to the same UUID
        // (the gRPC `player_filter` semantics: "any contract where
        // player X is on either side"), emit a single `master = $1
        // OR slave = $1` clause. Otherwise fall back to AND.
        if let (Some(p), Some(a)) = (proposer, acceptor) {
            if p == a {
                sql.push_str(" AND (master_uuid = $1 OR slave_uuid = $1)");
            } else {
                sql.push_str(" AND master_uuid = $1 AND slave_uuid = $2");
            }
        } else if proposer.is_some() {
            sql.push_str(" AND master_uuid = $1");
        } else if acceptor.is_some() {
            sql.push_str(" AND slave_uuid = $1");
        }
        if status.is_some() {
            let n = proposer.is_some() as i32 + acceptor.is_some() as i32 + 1;
            sql.push_str(&format!(" AND status = ${n}"));
        }
        sql.push_str(" ORDER BY updated_tick DESC");
        let n = proposer.is_some() as i32
            + acceptor.is_some() as i32
            + status.is_some() as i32
            + 1;
        sql.push_str(&format!(" LIMIT ${n}"));

        let mut q = sqlx::query(&sql);
        if let Some(p) = proposer {
            q = q.bind(p);
        }
        if let Some(a) = acceptor {
            q = q.bind(a);
        }
        if let Some(s) = status {
            q = q.bind(s.as_str());
        }
        q = q.bind(limit);

        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_contract).collect()
    }

    async fn update(&self, contract: &Contract) -> Result<(), RepoError> {
        let terms_json: serde_json::Value = serde_json::from_str(&contract.terms_json)
            .unwrap_or_else(|_| serde_json::json!({}));
        let res = sqlx::query(
            r#"
            UPDATE contracts
               SET master_uuid        = $2,
                   slave_uuid         = $3,
                   status             = $4,
                   terms_type         = $5,
                   terms_json         = $6,
                   revenue_share_pct  = $7,
                   redemption_cost    = $8,
                   expires_tick       = $9,
                   updated_tick       = $10,
                   activated_tick     = $11,
                   terminated_tick    = $12,
                   redeemed_tick      = $13,
                   reason             = $14
             WHERE contract_id = $1
            "#,
        )
        .bind(contract.contract_id)
        .bind(contract.proposer_uuid)
        .bind(contract.acceptor_uuid)
        .bind(contract.status.as_str())
        .bind(&contract.terms_type)
        .bind(&terms_json)
        .bind(contract.revenue_share_pct)
        .bind(contract.redemption_cost)
        .bind(contract.expires_tick)
        .bind(contract.updated_tick)
        .bind(contract.activated_tick)
        .bind(contract.terminated_tick)
        .bind(contract.redeemed_tick)
        .bind(contract.reason.as_deref())
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(RepoError::ContractNotFound {
                contract_id: contract.contract_id,
            });
        }
        Ok(())
    }

    async fn record_payout(
        &self,
        payout: &ContractPayout,
    ) -> Result<ContractPayout, RepoError> {
        // Idempotency short-circuit: same request_id → return
        // the prior row. The UNIQUE INDEX is the last-line
        // guard; this lookup is the fast path.
        if let Some(existing) = fetch_payout_by_request_id(&self.pool, payout.request_id).await? {
            return Ok(existing);
        }

        sqlx::query(
            r#"
            INSERT INTO contract_payouts
                   (payout_id, contract_id, from_account, to_account,
                    amount, reason, tick_millis, request_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(payout.payout_id)
        .bind(payout.contract_id)
        .bind(payout.from_account)
        .bind(payout.to_account)
        .bind(payout.amount)
        .bind(payout.reason.as_str())
        .bind(payout.tick_millis)
        .bind(payout.request_id)
        .execute(&self.pool)
        .await?;
        Ok(payout.clone())
    }

    async fn list_payouts(
        &self,
        contract_id: Uuid,
    ) -> Result<Vec<ContractPayout>, RepoError> {
        let rows = sqlx::query(
            r#"
            SELECT payout_id, contract_id, from_account, to_account,
                   amount, reason, tick_millis, request_id
              FROM contract_payouts
             WHERE contract_id = $1
             ORDER BY tick_millis DESC
            "#,
        )
        .bind(contract_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_payout).collect()
    }

    async fn list_expired(
        &self,
        current_tick: i64,
    ) -> Result<Vec<Contract>, RepoError> {
        let rows = sqlx::query(
            r#"
            SELECT contract_id, master_uuid, slave_uuid, status,
                   terms_type, terms_json,
                   revenue_share_pct, redemption_cost, expires_tick,
                   created_tick, updated_tick,
                   activated_tick, terminated_tick, redeemed_tick, reason
              FROM contracts
             WHERE status = 'PROPOSED'
               AND expires_tick IS NOT NULL
               AND expires_tick <= $1
            "#,
        )
        .bind(current_tick)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_contract).collect()
    }
}

async fn fetch_payout_by_request_id(
    pool: &PgPool,
    request_id: Uuid,
) -> Result<Option<ContractPayout>, RepoError> {
    let row_opt = sqlx::query(
        r#"
        SELECT payout_id, contract_id, from_account, to_account,
               amount, reason, tick_millis, request_id
          FROM contract_payouts
         WHERE request_id = $1
         LIMIT 1
        "#,
    )
    .bind(request_id)
    .fetch_optional(pool)
    .await?;
    match row_opt {
        None => Ok(None),
        Some(row) => Ok(Some(row_to_payout(&row)?)),
    }
}

// ── Audit writer (delegates to audit_bank) ─────────────────────────────────

pub struct PgContractAuditWriter {
    pool: PgPool,
}

impl PgContractAuditWriter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ContractAuditWriter for PgContractAuditWriter {
    async fn write(&self, entry: ContractAuditEntry) -> Result<(), RepoError> {
        let notes = entry.notes.unwrap_or_else(|| serde_json::json!({}));
        // Cross-table audit: contract payouts land in
        // `audit_bank` so a single query covers both ledger
        // sides. The op string is `contract.<event>` so the
        // existing 99 §2.2 audit dispatch can split by
        // namespace.
        sqlx::query(
            r#"
            INSERT INTO audit_bank
                   (log_id, actor_uuid, actor_type, target_account_uuid,
                    op, before_balance, after_balance,
                    tick_millis, request_id, notes)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(entry.log_id)
        .bind(entry.actor_uuid)
        .bind(entry.actor_type)
        .bind(entry.target_account_uuid)
        .bind(entry.op)
        .bind(entry.before_balance)
        .bind(entry.after_balance)
        .bind(entry.tick_millis)
        .bind(entry.request_id)
        .bind(&notes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// ── Composite service deps ─────────────────────────────────────────────────

#[derive(Clone)]
pub struct ContractServiceDeps {
    pub repo: std::sync::Arc<dyn ContractRepository>,
    /// Optional — when `None` the gRPC layer skips audit
    /// writes. The production wiring sets this; tests may
    /// leave it unset.
    pub audit: Option<std::sync::Arc<dyn ContractAuditWriter>>,
    /// Cross-crate handle to `BankService.Transfer`. The
    /// gRPC layer uses this for `RedeemContract`; the
    /// dependency is set up in `biocapital-grpc`'s
    /// `lib.rs` (task #54 follow-up). When `None` the gRPC
    /// layer emits a clear error rather than silently
    /// skipping the bank transfer.
    pub bank_transfer:
        Option<std::sync::Arc<dyn BankTransferPort + Send + Sync>>,
}

impl ContractServiceDeps {
    pub fn new(repo: std::sync::Arc<dyn ContractRepository>) -> Self {
        Self {
            repo,
            audit: None,
            bank_transfer: None,
        }
    }

    pub fn with_audit(mut self, audit: std::sync::Arc<dyn ContractAuditWriter>) -> Self {
        self.audit = Some(audit);
        self
    }

    pub fn with_bank_transfer(
        mut self,
        bank: std::sync::Arc<dyn BankTransferPort + Send + Sync>,
    ) -> Self {
        self.bank_transfer = Some(bank);
        self
    }
}

/// Cross-crate port: a thin wrapper around the bank
/// transfer call. The `biocapital-grpc::contract_service`
/// supplies a real implementation that wraps the gRPC
/// `BankService::Transfer` client; tests can substitute a
/// mock.
///
/// Returned tuple mirrors `BankRepository::atomic_transfer`
/// semantics — `(tx_out_balance_after, tx_in_balance_after)`
/// for logging / audit purposes. Failures map to
/// `tonic::Status` already; the contract layer just
/// propagates.
#[async_trait]
pub trait BankTransferPort: Send + Sync {
    async fn transfer(
        &self,
        from: Uuid,
        to: Uuid,
        amount: i64,
        memo: Option<String>,
        request_id: Uuid,
    ) -> Result<(i64, i64), tonic::Status>;
}

// ── Sanity test (no live DB) ────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trait_is_object_safe() {
        fn _assert_object_safe(_: std::sync::Arc<dyn ContractRepository>) {}
        fn _assert_audit_object_safe(
            _: std::sync::Arc<dyn ContractAuditWriter>,
        ) {
        }
        fn _assert_bank_object_safe(
            _: std::sync::Arc<dyn BankTransferPort + Send + Sync>,
        ) {
        }
    }
}