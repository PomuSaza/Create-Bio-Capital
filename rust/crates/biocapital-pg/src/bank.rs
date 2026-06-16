//! PostgreSQL persistence for the Bank module (08 §5).
//!
//! Companion to the domain types in `biocapital_bank::domain`. This
//! module is the only piece of code that talks to `bank_accounts`,
//! `cat_grass_batches`, `bank_transactions`, and `audit_bank`.
//!
//! Public surface:
//! - [`BankRepository`] — trait the gRPC service depends on; tests
//!   can substitute an in-memory implementation.
//! - [`PgBankRepository`] — production implementation backed by
//!   `sqlx::PgPool`.
//! - [`BankAuditWriter`] — append-only writer for `audit_bank`,
//!   wired into the gRPC service via [`BankServiceDeps`].
//! - [`RepoError`] — typed error surface returned to callers
//!   (also implements `Into<tonic::Status>` for clean `?` usage in
//!   the gRPC layer).
//!
//! Idempotency: every `atomic_*` method is keyed on `request_id`. If
//! the same id has already been recorded, the call returns the prior
//! transaction unchanged instead of performing the mutation a second
//! time (08 §5.3 + 99 §2.2).

use async_trait::async_trait;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

use biocapital_bank::domain::{
    BankAccount, BankOp, BankTransaction, CatGrassBatch, CatGrassSource, MAX_BALANCE,
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

    #[error("invalid BankOp in column {column}: {value}")]
    InvalidBankOp { column: &'static str, value: String },

    #[error("invalid CatGrassSource in column {column}: {value}")]
    InvalidSource { column: &'static str, value: String },

    #[error("account {account_uuid} not found")]
    AccountNotFound { account_uuid: Uuid },

    #[error("batch {batch_id} not found")]
    BatchNotFound { batch_id: Uuid },
}

/// Re-export alias so callers can disambiguate from
/// `player_state::RepoError`. The two enums are kept distinct so
/// the gRPC service can `From`-convert each to the right tonic
/// status without an extra match arm.
pub type BankRepoError = RepoError;

impl From<RepoError> for tonic::Status {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::Sqlx(sqlx::Error::RowNotFound) => {
                tonic::Status::not_found("bank row not found")
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
            RepoError::InvalidBankOp { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid BankOp in {column}: {value}"
                ))
            }
            RepoError::InvalidSource { column, value } => {
                tonic::Status::invalid_argument(format!(
                    "invalid CatGrassSource in {column}: {value}"
                ))
            }
            RepoError::AccountNotFound { account_uuid } => {
                tonic::Status::not_found(format!(
                    "bank account {account_uuid} not found"
                ))
            }
            RepoError::BatchNotFound { batch_id } => {
                tonic::Status::not_found(format!("cat-grass batch {batch_id} not found"))
            }
        }
    }
}

// ── Repository trait ────────────────────────────────────────────────────────

#[async_trait]
pub trait BankRepository: Send + Sync {
    async fn get_account(&self, uuid: Uuid) -> Result<BankAccount, RepoError>;

    async fn get_account_by_owner(
        &self,
        owner: Uuid,
    ) -> Result<BankAccount, RepoError>;

    async fn upsert_account(&self, account: &BankAccount) -> Result<(), RepoError>;

    /// Atomic deposit: bumps `bank_accounts.balance` and writes a
    /// `bank_transactions` row in a single transaction. Idempotent on
    /// `request_id` — see module docs.
    async fn atomic_deposit(
        &self,
        uuid: Uuid,
        amount: i64,
        batch_id: Uuid,
        request_id: Uuid,
        tick_millis: i64,
    ) -> Result<BankTransaction, RepoError>;

    async fn atomic_withdraw(
        &self,
        uuid: Uuid,
        amount: i64,
        request_id: Uuid,
        tick_millis: i64,
    ) -> Result<BankTransaction, RepoError>;

    /// Atomic two-account transfer. Returns the two
    /// `BankTransaction` rows (TRANSFER_OUT, TRANSFER_IN) in
    /// canonical order. Idempotent on `request_id`.
    async fn atomic_transfer(
        &self,
        from: Uuid,
        to: Uuid,
        amount: i64,
        counterparty_name: Option<String>,
        request_id: Uuid,
        tick_millis: i64,
    ) -> Result<(BankTransaction, BankTransaction), RepoError>;

    async fn get_history(
        &self,
        uuid: Uuid,
        limit: i64,
    ) -> Result<Vec<BankTransaction>, RepoError>;

    async fn lock_device(
        &self,
        uuid: Uuid,
        device_id: String,
    ) -> Result<BankAccount, RepoError>;

    /// Accept an invite code; the implementation is free to validate
    /// the code however the deployment wants. For now we accept any
    /// non-empty code (the gRPC layer is the gatekeeper).
    async fn unlock_device(
        &self,
        uuid: Uuid,
        invite_code: Uuid,
    ) -> Result<BankAccount, RepoError>;

    async fn create_batch(
        &self,
        batch: &CatGrassBatch,
    ) -> Result<(), RepoError>;

    async fn consume_batch(
        &self,
        batch_id: Uuid,
        amount: i64,
    ) -> Result<CatGrassBatch, RepoError>;

    async fn list_batches_by_holder(
        &self,
        holder: Uuid,
    ) -> Result<Vec<CatGrassBatch>, RepoError>;
}

// ── Audit writer ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BankAuditEntry {
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
pub trait BankAuditWriter: Send + Sync {
    async fn write(&self, entry: BankAuditEntry) -> Result<(), RepoError>;
}

// ── Postgres implementation ────────────────────────────────────────────────

#[derive(Clone)]
pub struct PgBankRepository {
    pool: PgPool,
}

impl PgBankRepository {
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

/// Helper: idempotency lookup. Returns the previously-stored tx
/// row when `request_id` has already been recorded, otherwise None.
async fn fetch_by_request_id(
    pool: &PgPool,
    request_id: Uuid,
) -> Result<Option<BankTransaction>, RepoError> {
    let row_opt = sqlx::query(
        r#"
        SELECT tx_id, account_uuid, op, amount, balance_after,
               counterparty_uuid, counterparty_name, tick_millis, request_id
          FROM bank_transactions
         WHERE request_id = $1
         LIMIT 1
        "#,
    )
    .bind(request_id)
    .fetch_optional(pool)
    .await?;
    match row_opt {
        None => Ok(None),
        Some(row) => Ok(Some(row_to_tx(&row)?)),
    }
}

fn row_to_account(row: &sqlx::postgres::PgRow) -> Result<BankAccount, RepoError> {
    let account_uuid: Uuid = row.try_get("account_uuid").map_err(|e| {
        RepoError::InvalidUuid {
            column: "account_uuid",
            value: e.to_string(),
        }
    })?;
    let owner_uuid: Uuid = row.try_get("owner_uuid").map_err(|e| {
        RepoError::InvalidUuid {
            column: "owner_uuid",
            value: e.to_string(),
        }
    })?;
    let balance: i64 = row.try_get("balance").unwrap_or(0);
    let max_balance: i64 = row.try_get("max_balance").unwrap_or(MAX_BALANCE);
    let device_lock: Option<String> = row.try_get("device_lock").ok();
    let created_tick: i64 = row.try_get("created_tick").unwrap_or(0);
    let updated_tick: i64 = row.try_get("updated_tick").unwrap_or(0);

    // Defensive clamps. The CHECK constraints should make these
    // unconditional, but we don't want a poisoned row to crash the
    // gRPC server on read.
    let balance = balance.clamp(0, max_balance);
    let max_balance = max_balance.max(1);

    Ok(BankAccount {
        account_uuid,
        owner_uuid,
        balance,
        max_balance,
        device_lock,
        created_tick,
        updated_tick,
    })
}

fn row_to_tx(row: &sqlx::postgres::PgRow) -> Result<BankTransaction, RepoError> {
    let tx_id: Uuid = row.try_get("tx_id").map_err(|e| RepoError::InvalidUuid {
        column: "tx_id",
        value: e.to_string(),
    })?;
    let account_uuid: Uuid = row.try_get("account_uuid").map_err(|e| {
        RepoError::InvalidUuid {
            column: "account_uuid",
            value: e.to_string(),
        }
    })?;
    let op_wire: String = row.try_get("op").map_err(|e| RepoError::InvalidBankOp {
        column: "op",
        value: e.to_string(),
    })?;
    let op = BankOp::from_wire(&op_wire).ok_or_else(|| RepoError::InvalidBankOp {
        column: "op",
        value: op_wire.clone(),
    })?;
    let amount: i64 = row.try_get("amount").unwrap_or(0);
    let balance_after: i64 = row.try_get("balance_after").unwrap_or(0);
    let counterparty_uuid: Option<Uuid> = row.try_get("counterparty_uuid").ok();
    let counterparty_name: Option<String> = row.try_get("counterparty_name").ok();
    let tick_millis: i64 = row.try_get("tick_millis").unwrap_or(0);
    let request_id: Uuid = row
        .try_get("request_id")
        .map_err(|e| RepoError::InvalidUuid {
            column: "request_id",
            value: e.to_string(),
        })?;

    Ok(BankTransaction {
        tx_id,
        account_uuid,
        op,
        amount: amount.max(0),
        balance_after: balance_after.max(0),
        counterparty_uuid,
        counterparty_name,
        tick_millis,
        request_id,
    })
}

fn row_to_batch(row: &sqlx::postgres::PgRow) -> Result<CatGrassBatch, RepoError> {
    let batch_id: Uuid = row.try_get("batch_id").map_err(|e| RepoError::InvalidUuid {
        column: "batch_id",
        value: e.to_string(),
    })?;
    let producer_uuid: Option<Uuid> = row.try_get("producer_uuid").ok();
    let production_tick: i64 = row.try_get("production_tick").unwrap_or(0);
    let source_wire: String = row.try_get("production_source").map_err(|e| {
        RepoError::InvalidSource {
            column: "production_source",
            value: e.to_string(),
        }
    })?;
    let production_source = CatGrassSource::from_wire(&source_wire).ok_or_else(|| {
        RepoError::InvalidSource {
            column: "production_source",
            value: source_wire.clone(),
        }
    })?;
    let total_amount: i64 = row.try_get("total_amount").unwrap_or(0);
    let remaining_amount: i64 = row.try_get("remaining_amount").unwrap_or(0);
    let current_holder_uuid: Option<Uuid> = row.try_get("current_holder_uuid").ok();

    Ok(CatGrassBatch {
        batch_id,
        producer_uuid,
        production_tick,
        production_source,
        total_amount: total_amount.max(0),
        remaining_amount: remaining_amount.max(0),
        current_holder_uuid,
    })
}

#[async_trait]
impl BankRepository for PgBankRepository {
    async fn get_account(&self, uuid: Uuid) -> Result<BankAccount, RepoError> {
        let row_opt = sqlx::query(
            r#"
            SELECT account_uuid, owner_uuid, balance, max_balance,
                   device_lock, created_tick, updated_tick
              FROM bank_accounts
             WHERE account_uuid = $1
            "#,
        )
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        match row_opt {
            None => Err(RepoError::AccountNotFound { account_uuid: uuid }),
            Some(row) => row_to_account(&row),
        }
    }

    async fn get_account_by_owner(
        &self,
        owner: Uuid,
    ) -> Result<BankAccount, RepoError> {
        let row_opt = sqlx::query(
            r#"
            SELECT account_uuid, owner_uuid, balance, max_balance,
                   device_lock, created_tick, updated_tick
              FROM bank_accounts
             WHERE owner_uuid = $1
            "#,
        )
        .bind(owner)
        .fetch_optional(&self.pool)
        .await?;
        match row_opt {
            None => Err(RepoError::AccountNotFound { account_uuid: owner }),
            Some(row) => row_to_account(&row),
        }
    }

    async fn upsert_account(&self, account: &BankAccount) -> Result<(), RepoError> {
        sqlx::query(
            r#"
            INSERT INTO bank_accounts
                   (account_uuid, owner_uuid, balance, max_balance,
                    device_lock, created_tick, updated_tick)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (account_uuid) DO UPDATE
            SET balance      = EXCLUDED.balance,
                max_balance  = EXCLUDED.max_balance,
                device_lock  = EXCLUDED.device_lock,
                updated_tick = EXCLUDED.updated_tick
            "#,
        )
        .bind(account.account_uuid)
        .bind(account.owner_uuid)
        .bind(account.balance)
        .bind(account.max_balance)
        .bind(account.device_lock.as_deref())
        .bind(account.created_tick)
        .bind(account.updated_tick)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn atomic_deposit(
        &self,
        uuid: Uuid,
        amount: i64,
        batch_id: Uuid,
        request_id: Uuid,
        tick_millis: i64,
    ) -> Result<BankTransaction, RepoError> {
        // Idempotency short-circuit.
        if let Some(existing) = fetch_by_request_id(&self.pool, request_id).await? {
            return Ok(existing);
        }

        let mut tx = self.pool.begin().await?;

        // Lock the row to prevent concurrent deposits racing.
        let row_opt = sqlx::query(
            r#"
            SELECT balance, max_balance
              FROM bank_accounts
             WHERE account_uuid = $1
             FOR UPDATE
            "#,
        )
        .bind(uuid)
        .fetch_optional(&mut *tx)
        .await?;

        let row = row_opt.ok_or(RepoError::AccountNotFound { account_uuid: uuid })?;
        let balance: i64 = row.try_get("balance").unwrap_or(0);
        let max_balance: i64 = row.try_get("max_balance").unwrap_or(MAX_BALANCE);

        // Clamp at the cap; mirror the Java BankManager.deposit
        // behaviour (08 §3.4 — 面板 1 is read-only, but the deposit
        // RPC must not push balance above max).
        let headroom = max_balance - balance;
        let accepted = amount.min(headroom).max(0);
        let new_balance = balance + accepted;

        sqlx::query(
            r#"
            UPDATE bank_accounts
               SET balance = $2, updated_tick = $3
             WHERE account_uuid = $1
            "#,
        )
        .bind(uuid)
        .bind(new_balance)
        .bind(tick_millis)
        .execute(&mut *tx)
        .await?;

        // Record the transaction row. The unique index on
        // `request_id` is the final dedupe guard; if a concurrent
        // request slipped in between our lookup and this insert, the
        // conflict aborts this transaction and the caller retries.
        let tx_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO bank_transactions
                   (tx_id, account_uuid, op, amount, balance_after,
                    counterparty_uuid, counterparty_name, tick_millis, request_id)
            VALUES ($1, $2, 'DEPOSIT', $3, $4, $5, NULL, $6, $7)
            "#,
        )
        .bind(tx_id)
        .bind(uuid)
        .bind(accepted)
        .bind(new_balance)
        .bind(batch_id) // counterparty_uuid carries the batch_id on deposit
        .bind(tick_millis)
        .bind(request_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(BankTransaction {
            tx_id,
            account_uuid: uuid,
            op: BankOp::Deposit,
            amount: accepted,
            balance_after: new_balance,
            counterparty_uuid: Some(batch_id),
            counterparty_name: None,
            tick_millis,
            request_id,
        })
    }

    async fn atomic_withdraw(
        &self,
        uuid: Uuid,
        amount: i64,
        request_id: Uuid,
        tick_millis: i64,
    ) -> Result<BankTransaction, RepoError> {
        if let Some(existing) = fetch_by_request_id(&self.pool, request_id).await? {
            return Ok(existing);
        }

        let mut tx = self.pool.begin().await?;

        let row_opt = sqlx::query(
            r#"
            SELECT balance
              FROM bank_accounts
             WHERE account_uuid = $1
             FOR UPDATE
            "#,
        )
        .bind(uuid)
        .fetch_optional(&mut *tx)
        .await?;

        let row = row_opt.ok_or(RepoError::AccountNotFound { account_uuid: uuid })?;
        let balance: i64 = row.try_get("balance").unwrap_or(0);

        // Mirror Java: take = min(balance, amount). Insufficient
        // funds are surfaced as 0 taken (not an error); the gRPC
        // layer's validator short-circuits before reaching here.
        let taken = balance.min(amount.max(0));
        let new_balance = balance - taken;

        sqlx::query(
            r#"
            UPDATE bank_accounts
               SET balance = $2, updated_tick = $3
             WHERE account_uuid = $1
            "#,
        )
        .bind(uuid)
        .bind(new_balance)
        .bind(tick_millis)
        .execute(&mut *tx)
        .await?;

        let tx_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO bank_transactions
                   (tx_id, account_uuid, op, amount, balance_after,
                    counterparty_uuid, counterparty_name, tick_millis, request_id)
            VALUES ($1, $2, 'WITHDRAW', $3, $4, NULL, NULL, $5, $6)
            "#,
        )
        .bind(tx_id)
        .bind(uuid)
        .bind(taken)
        .bind(new_balance)
        .bind(tick_millis)
        .bind(request_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(BankTransaction {
            tx_id,
            account_uuid: uuid,
            op: BankOp::Withdraw,
            amount: taken,
            balance_after: new_balance,
            counterparty_uuid: None,
            counterparty_name: None,
            tick_millis,
            request_id,
        })
    }

    async fn atomic_transfer(
        &self,
        from: Uuid,
        to: Uuid,
        amount: i64,
        counterparty_name: Option<String>,
        request_id: Uuid,
        tick_millis: i64,
    ) -> Result<(BankTransaction, BankTransaction), RepoError> {
        if from == to {
            return Err(RepoError::Sqlx(sqlx::Error::Protocol(
                "transfer from == to".into(),
            )));
        }
        if let Some(existing) = fetch_by_request_id(&self.pool, request_id).await? {
            // We can't return both halves from a single id lookup;
            // the contract here is that the same request_id implies
            // a full prior transfer, so we look up the counter-side
            // row by (counterparty_uuid, request_id).
            let from_row = existing.clone();
            let counter_row_opt = sqlx::query(
                r#"
                SELECT tx_id, account_uuid, op, amount, balance_after,
                       counterparty_uuid, counterparty_name, tick_millis, request_id
                  FROM bank_transactions
                 WHERE request_id = $1 AND account_uuid = $2
                "#,
            )
            .bind(request_id)
            .bind(to)
            .fetch_optional(&self.pool)
            .await?;
            let counter_row = counter_row_opt.ok_or(RepoError::Sqlx(
                sqlx::Error::Protocol("transfer counter-tx missing".into()),
            ))?;
            return Ok((from_row, row_to_tx(&counter_row)?));
        }

        let mut tx = self.pool.begin().await?;

        // Lock both rows in a deterministic order to avoid deadlocks.
        let (first, second) = if from < to { (from, to) } else { (to, from) };

        let first_row_opt = sqlx::query(
            r#"
            SELECT balance
              FROM bank_accounts
             WHERE account_uuid = $1
             FOR UPDATE
            "#,
        )
        .bind(first)
        .fetch_optional(&mut *tx)
        .await?;
        let second_row_opt = sqlx::query(
            r#"
            SELECT balance
              FROM bank_accounts
             WHERE account_uuid = $1
             FOR UPDATE
            "#,
        )
        .bind(second)
        .fetch_optional(&mut *tx)
        .await?;

        let (from_row, to_row) = if from < to {
            (
                first_row_opt.ok_or(RepoError::AccountNotFound { account_uuid: from })?,
                second_row_opt.ok_or(RepoError::AccountNotFound { account_uuid: to })?,
            )
        } else {
            (
                second_row_opt.ok_or(RepoError::AccountNotFound { account_uuid: from })?,
                first_row_opt.ok_or(RepoError::AccountNotFound { account_uuid: to })?,
            )
        };

        let from_balance: i64 = from_row.try_get("balance").unwrap_or(0);
        let to_balance: i64 = to_row.try_get("balance").unwrap_or(0);

        let taken = from_balance.min(amount.max(0));
        // 08 §3.4 面板 2: cap destination at max_balance.
        let max_to_balance: i64 = sqlx::query_scalar(
            "SELECT max_balance FROM bank_accounts WHERE account_uuid = $1",
        )
        .bind(to)
        .fetch_one(&mut *tx)
        .await
        .unwrap_or(MAX_BALANCE);
        let headroom = max_to_balance - to_balance;
        let credited = taken.min(headroom).max(0);
        let new_from = from_balance - taken;
        let new_to = to_balance + credited;

        sqlx::query(
            r#"
            UPDATE bank_accounts
               SET balance = $2, updated_tick = $3
             WHERE account_uuid = $1
            "#,
        )
        .bind(from)
        .bind(new_from)
        .bind(tick_millis)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            UPDATE bank_accounts
               SET balance = $2, updated_tick = $3
             WHERE account_uuid = $1
            "#,
        )
        .bind(to)
        .bind(new_to)
        .bind(tick_millis)
        .execute(&mut *tx)
        .await?;

        let tx_out_id = Uuid::new_v4();
        let tx_in_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO bank_transactions
                   (tx_id, account_uuid, op, amount, balance_after,
                    counterparty_uuid, counterparty_name, tick_millis, request_id)
            VALUES ($1, $2, 'TRANSFER_OUT', $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(tx_out_id)
        .bind(from)
        .bind(taken)
        .bind(new_from)
        .bind(to)
        .bind(counterparty_name.as_deref())
        .bind(tick_millis)
        .bind(request_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO bank_transactions
                   (tx_id, account_uuid, op, amount, balance_after,
                    counterparty_uuid, counterparty_name, tick_millis, request_id)
            VALUES ($1, $2, 'TRANSFER_IN', $3, $4, $5, NULL, $6, $7)
            "#,
        )
        .bind(tx_in_id)
        .bind(to)
        .bind(credited)
        .bind(new_to)
        .bind(from)
        .bind(tick_millis)
        .bind(request_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok((
            BankTransaction {
                tx_id: tx_out_id,
                account_uuid: from,
                op: BankOp::TransferOut,
                amount: taken,
                balance_after: new_from,
                counterparty_uuid: Some(to),
                counterparty_name: counterparty_name.clone(),
                tick_millis,
                request_id,
            },
            BankTransaction {
                tx_id: tx_in_id,
                account_uuid: to,
                op: BankOp::TransferIn,
                amount: credited,
                balance_after: new_to,
                counterparty_uuid: Some(from),
                counterparty_name: None,
                tick_millis,
                request_id,
            },
        ))
    }

    async fn get_history(
        &self,
        uuid: Uuid,
        limit: i64,
    ) -> Result<Vec<BankTransaction>, RepoError> {
        let limit = limit.clamp(1, 1024);
        let rows = sqlx::query(
            r#"
            SELECT tx_id, account_uuid, op, amount, balance_after,
                   counterparty_uuid, counterparty_name, tick_millis, request_id
              FROM bank_transactions
             WHERE account_uuid = $1
             ORDER BY tick_millis DESC
             LIMIT $2
            "#,
        )
        .bind(uuid)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_tx).collect()
    }

    async fn lock_device(
        &self,
        uuid: Uuid,
        device_id: String,
    ) -> Result<BankAccount, RepoError> {
        let row_opt = sqlx::query(
            r#"
            UPDATE bank_accounts
               SET device_lock = $2,
                   updated_tick = $3
             WHERE account_uuid = $1
            RETURNING account_uuid, owner_uuid, balance, max_balance,
                      device_lock, created_tick, updated_tick
            "#,
        )
        .bind(uuid)
        .bind(&device_id)
        .bind(chrono::Utc::now().timestamp_millis())
        .fetch_optional(&self.pool)
        .await?;
        let row = row_opt.ok_or(RepoError::AccountNotFound { account_uuid: uuid })?;
        row_to_account(&row)
    }

    async fn unlock_device(
        &self,
        uuid: Uuid,
        _invite_code: Uuid,
    ) -> Result<BankAccount, RepoError> {
        // The invite code validation is a future concern (12 §2.7
        // surfaces the 10-minute expiry). For now we accept any
        // non-empty code and clear the lock; the gRPC layer is the
        // gatekeeper.
        let row_opt = sqlx::query(
            r#"
            UPDATE bank_accounts
               SET device_lock = NULL,
                   updated_tick = $2
             WHERE account_uuid = $1
            RETURNING account_uuid, owner_uuid, balance, max_balance,
                      device_lock, created_tick, updated_tick
            "#,
        )
        .bind(uuid)
        .bind(chrono::Utc::now().timestamp_millis())
        .fetch_optional(&self.pool)
        .await?;
        let row = row_opt.ok_or(RepoError::AccountNotFound { account_uuid: uuid })?;
        row_to_account(&row)
    }

    async fn create_batch(
        &self,
        batch: &CatGrassBatch,
    ) -> Result<(), RepoError> {
        sqlx::query(
            r#"
            INSERT INTO cat_grass_batches
                   (batch_id, producer_uuid, production_tick, production_source,
                    total_amount, remaining_amount, current_holder_uuid)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(batch.batch_id)
        .bind(batch.producer_uuid)
        .bind(batch.production_tick)
        .bind(batch.production_source.as_str())
        .bind(batch.total_amount)
        .bind(batch.remaining_amount)
        .bind(batch.current_holder_uuid)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn consume_batch(
        &self,
        batch_id: Uuid,
        amount: i64,
    ) -> Result<CatGrassBatch, RepoError> {
        let mut tx = self.pool.begin().await?;
        let row_opt = sqlx::query(
            r#"
            SELECT batch_id, producer_uuid, production_tick, production_source,
                   total_amount, remaining_amount, current_holder_uuid
              FROM cat_grass_batches
             WHERE batch_id = $1
             FOR UPDATE
            "#,
        )
        .bind(batch_id)
        .fetch_optional(&mut *tx)
        .await?;
        let row = row_opt.ok_or(RepoError::BatchNotFound { batch_id })?;
        let mut batch = row_to_batch(&row)?;

        // Clamp at remaining (mirrors CatGrassBatch::consume
        // semantics). SQL CHECK will reject if we ever go below
        // zero, but the clamp is the in-memory guarantee.
        let taken = amount.max(0).min(batch.remaining_amount);
        batch.remaining_amount -= taken;

        sqlx::query(
            r#"
            UPDATE cat_grass_batches
               SET remaining_amount = $2
             WHERE batch_id = $1
            "#,
        )
        .bind(batch_id)
        .bind(batch.remaining_amount)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(batch)
    }

    async fn list_batches_by_holder(
        &self,
        holder: Uuid,
    ) -> Result<Vec<CatGrassBatch>, RepoError> {
        let rows = sqlx::query(
            r#"
            SELECT batch_id, producer_uuid, production_tick, production_source,
                   total_amount, remaining_amount, current_holder_uuid
              FROM cat_grass_batches
             WHERE current_holder_uuid = $1
             ORDER BY production_tick DESC
            "#,
        )
        .bind(holder)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_batch).collect()
    }
}

// ── Audit writer ────────────────────────────────────────────────────────────

pub struct PgBankAuditWriter {
    pool: PgPool,
}

impl PgBankAuditWriter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl BankAuditWriter for PgBankAuditWriter {
    async fn write(&self, entry: BankAuditEntry) -> Result<(), RepoError> {
        let notes = entry.notes.unwrap_or_else(|| json!({}));
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
        .bind(notes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// ── Composite service deps ─────────────────────────────────────────────────

#[derive(Clone)]
pub struct BankServiceDeps {
    pub repo: std::sync::Arc<dyn BankRepository>,
    pub audit: std::sync::Arc<dyn BankAuditWriter>,
}

impl BankServiceDeps {
    pub fn new(
        repo: std::sync::Arc<dyn BankRepository>,
        audit: std::sync::Arc<dyn BankAuditWriter>,
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
        fn _assert_object_safe(_: std::sync::Arc<dyn BankRepository>) {}
        fn _assert_audit_object_safe(_: std::sync::Arc<dyn BankAuditWriter>) {}
    }
}
