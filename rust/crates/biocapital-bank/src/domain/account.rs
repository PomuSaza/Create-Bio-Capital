//! Bank account domain types — `BankAccount`, `BankTransaction`,
//! `BankOp`.
//!
//! See `doc/08-bank.md` §1 / §5 for the design contract and
//! `doc/14-rust-services.md` §3.2 for the proto schema. The constants
//! here pin the wire values that `BankManager.MAX_BALANCE` and
//! `BankManager.HISTORY_SIZE` in the Java side share.

use thiserror::Error;
use uuid::Uuid;

// ── Constants (mirrors Java `BankManager` 1:1) ──────────────────────────────

/// Per-player balance cap, in cat-grass units (1 unit == 1 blade).
///
/// Mirrors `mo.dystopia.biocapital.bank.BankManager.MAX_BALANCE` in the
/// Java side. Changing this value touches the SQL `CHECK` constraint on
/// `bank_accounts.balance`, the gRPC `BalanceResponse.max_balance`
/// field, and the `08-bank.md` §1.2 invariant — see
/// `99-integration-matrix.md` §10.2.
pub const MAX_BALANCE: i64 = 100_000_000;

/// History ring buffer size used by the bank screen's "History" panel.
///
/// Mirrors `mo.dystopia.biocapital.bank.BankManager.HISTORY_SIZE`.
pub const HISTORY_SIZE: usize = 16;

// ── BankOp ─────────────────────────────────────────────────────────────────

/// The four kinds of bank ledger entries. The string form is the value
/// stored in `bank_transactions.op` and surfaced via the
/// `BankTransaction.op` proto field — must stay in lock-step with
/// `CHECK (op IN ('DEPOSIT','WITHDRAW','TRANSFER_OUT','TRANSFER_IN'))`
/// on the SQL table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BankOp {
    Deposit,
    Withdraw,
    TransferOut,
    TransferIn,
}

impl BankOp {
    /// Wire/SQL form. Used for `bank_transactions.op` and the audit
    /// `op` column.
    pub fn as_str(self) -> &'static str {
        match self {
            BankOp::Deposit => "DEPOSIT",
            BankOp::Withdraw => "WITHDRAW",
            BankOp::TransferOut => "TRANSFER_OUT",
            BankOp::TransferIn => "TRANSFER_IN",
        }
    }

    /// Parse the wire form back into the enum. Returns `None` for
    /// unknown strings (defensive — the SQL CHECK constraint should
    /// already prevent this).
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "DEPOSIT" => Some(BankOp::Deposit),
            "WITHDRAW" => Some(BankOp::Withdraw),
            "TRANSFER_OUT" => Some(BankOp::TransferOut),
            "TRANSFER_IN" => Some(BankOp::TransferIn),
            _ => None,
        }
    }

    /// Audit-log op suffix used by the gRPC layer (e.g. `bank.deposit`,
    /// `bank.transfer`). Concatenated with the `bank.` prefix by the
    /// service so the `AuditService` can dispatch on a single namespace.
    pub fn audit_op(self) -> &'static str {
        match self {
            BankOp::Deposit => "bank.deposit",
            BankOp::Withdraw => "bank.withdraw",
            BankOp::TransferOut => "bank.transfer_out",
            BankOp::TransferIn => "bank.transfer_in",
        }
    }
}

// ── BankAccount ────────────────────────────────────────────────────────────

/// One bank account, owned by a single player. The `account_uuid` is
/// the durable primary key (`bank_accounts.account_uuid`); `owner_uuid`
/// is a unique index used for "look up by player" (e.g. when a card is
/// first signed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankAccount {
    /// Primary key (`bank_accounts.account_uuid`).
    pub account_uuid: Uuid,
    /// The player this account belongs to (`bank_accounts.owner_uuid`).
    pub owner_uuid: Uuid,
    /// Current balance in cat-grass units. Always within
    /// `[0, max_balance]`. CHECK constraint on the SQL column enforces
    /// the lower bound; the `MAX_BALANCE` cap is enforced by the
    /// repository on the write path.
    pub balance: i64,
    /// Per-account cap. Defaults to [`MAX_BALANCE`]; the SQL default
    /// is 100,000,000 to match. Held per-row so future config
    /// changes can lower a single account's cap without a global
    /// migration.
    pub max_balance: i64,
    /// Device ID the account is locked to, if any. `None` means
    /// "unlocked — any device can perform writes" (see
    /// `08-bank.md` §3.5).
    pub device_lock: Option<String>,
    /// Server tick (millis) of the first row insert.
    pub created_tick: i64,
    /// Server tick (millis) of the most recent successful mutation.
    pub updated_tick: i64,
}

impl BankAccount {
    /// Can the account lose `amount` units right now? The check is
    /// pure logic — it does **not** consult the device lock; the
    /// caller is expected to have validated the device context first
    /// (the gRPC service routes through `validate_transfer` /
    /// `validate_withdraw` for that).
    pub fn can_withdraw(&self, amount: i64) -> bool {
        if amount <= 0 {
            return false;
        }
        self.balance >= amount
    }

    /// Can the account receive `amount` units right now? Enforces the
    /// per-account `max_balance` cap.
    pub fn can_deposit(&self, amount: i64) -> bool {
        if amount <= 0 {
            return false;
        }
        self.balance.saturating_add(amount) <= self.max_balance
    }

    /// Is the account locked to `device_id`? `true` if a lock is set
    /// **and** the provided device does not match. `false` when
    /// `device_lock` is `None` (unlocked).
    pub fn is_locked_to(&self, device_id: Option<&str>) -> bool {
        match (&self.device_lock, device_id) {
            (Some(locked), Some(provided)) => locked != provided,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }
}

// ── BankTransaction ────────────────────────────────────────────────────────

/// One row of the `bank_transactions` table. Mirrors the proto
/// `BankTransaction` message shape 1:1 (see
/// `doc/14-rust-services.md` §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankTransaction {
    /// Primary key (`bank_transactions.tx_id`).
    pub tx_id: Uuid,
    /// Account this entry belongs to (`bank_transactions.account_uuid`).
    pub account_uuid: Uuid,
    /// Kind of operation.
    pub op: BankOp,
    /// Amount moved, always strictly positive. The direction is
    /// encoded in `op` (DEPOSIT = +balance, TRANSFER_OUT = -balance, …).
    pub amount: i64,
    /// Snapshot of the account balance **after** this transaction
    /// committed. Stored on the row so the "History" panel can render
    /// the bar without a second read.
    pub balance_after: i64,
    /// Counter-party account, when applicable (transfer). `None` for
    /// plain deposit / withdraw.
    pub counterparty_uuid: Option<Uuid>,
    /// Counter-party display name (e.g. the player's Minecraft name).
    /// Mirrors the Java `BankManager.transfer` `counterpartyName`
    /// argument; optional, truncated to 64 chars by the SQL CHECK
    /// constraint.
    pub counterparty_name: Option<String>,
    /// Server tick (millis) when the transaction committed.
    pub tick_millis: i64,
    /// Caller-supplied idempotency key. The repository uses this to
    /// short-circuit duplicate RPCs (see `08-bank.md` §5.3).
    pub request_id: Uuid,
}

// ── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BankError {
    /// Caller asked to withdraw / transfer more than the account
    /// currently holds.
    #[error("insufficient funds: have {have}, want {want}")]
    InsufficientFunds { have: i64, want: i64 },

    /// Caller asked to transfer / withdraw from a zero-balance
    /// account (08 §3.4 面板 2: 余额 = 0 不可转出).
    #[error("balance is zero; transfers are not allowed from an empty account")]
    ZeroBalance,

    /// Deposit would push the balance above `max_balance`. The
    /// repository layer clamps rather than rejecting on deposit
    /// (mirrors the Java side), but explicit transfers from the
    /// gRPC service use this to short-circuit.
    #[error("would exceed max balance ({max}): have {have}, add {add}")]
    OverMax { max: i64, have: i64, add: i64 },

    /// The account is device-locked to a different device than the
    /// one presenting the request (08 §3.5).
    #[error("device lock mismatch: locked to {locked:?}, presented {presented:?}")]
    DeviceLocked {
        locked: Option<String>,
        presented: Option<String>,
    },

    /// Non-positive amount slipped through past the gRPC layer's
    /// sanity checks. The repository layer treats this as a hard
    /// rejection so the ledger never sees a zero / negative delta.
    #[error("amount must be strictly positive, got {0}")]
    NonPositiveAmount(i64),
}

// ── Sanity tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn account(balance: i64, max: i64, lock: Option<&str>) -> BankAccount {
        let owner = Uuid::new_v4();
        BankAccount {
            account_uuid: Uuid::new_v4(),
            owner_uuid: owner,
            balance,
            max_balance: max,
            device_lock: lock.map(str::to_owned),
            created_tick: 0,
            updated_tick: 0,
        }
    }

    #[test]
    fn can_withdraw_rejects_non_positive_and_overdraw() {
        let a = account(100, MAX_BALANCE, None);
        assert!(!a.can_withdraw(0));
        assert!(!a.can_withdraw(-1));
        assert!(a.can_withdraw(100));
        assert!(!a.can_withdraw(101));
    }

    #[test]
    fn can_deposit_respects_max_balance() {
        let a = account(MAX_BALANCE - 5, MAX_BALANCE, None);
        assert!(a.can_deposit(5));
        assert!(!a.can_deposit(6));
        assert!(!a.can_deposit(0));
    }

    #[test]
    fn is_locked_to_respects_none_and_mismatch() {
        let unlocked = account(0, MAX_BALANCE, None);
        assert!(!unlocked.is_locked_to(Some("dev-a")));
        assert!(!unlocked.is_locked_to(None));

        let locked = account(0, MAX_BALANCE, Some("dev-a"));
        assert!(!locked.is_locked_to(Some("dev-a")));
        assert!(locked.is_locked_to(Some("dev-b")));
        assert!(locked.is_locked_to(None));
    }

    #[test]
    fn bank_op_round_trips_wire_form() {
        for op in [
            BankOp::Deposit,
            BankOp::Withdraw,
            BankOp::TransferOut,
            BankOp::TransferIn,
        ] {
            assert_eq!(BankOp::from_wire(op.as_str()), Some(op));
        }
        assert_eq!(BankOp::from_wire("NOPE"), None);
    }
}
