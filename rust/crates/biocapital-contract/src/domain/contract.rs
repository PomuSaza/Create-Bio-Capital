//! Contract domain types — `Contract` / `ContractPayout` /
//! `ContractStatus` (09 §2.1 + 14 §3.2).
//!
//! This module is the `biocapital-contract` analogue of
//! `biocapital_bank::domain::account`: pure data + status enum,
//! independent of PostgreSQL, gRPC, or JNI. Lifecycle transitions
//! live in [`super::lifecycle`] and produce / consume these
//! structs.

use thiserror::Error;
use uuid::Uuid;

// ── ContractStatus ──────────────────────────────────────────────────────────

/// The five lifecycle states a contract can be in.
///
/// Wire form (the value stored in `contracts.status` and surfaced
/// via the proto `ContractResponse.status` field) is the SCREAMING
/// SNAKE form below. The SQL `CHECK (status IN (...))` constraint
/// on `contracts.status` enforces the same set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContractStatus {
    /// Proposer has created the row; acceptor has not yet
    /// confirmed. Initial state from `ProposeContract`.
    Proposed,
    /// Both parties have confirmed. Daily payouts and
    /// redemption cost enforcement are live.
    Active,
    /// Either party explicitly ended the contract before its
    /// natural expiry / redemption. Reason carried in
    /// [`Contract::reason`].
    Terminated,
    /// The slave side paid off `redemption_cost` to the master
    /// and the contract was dissolved (09 §3.3).
    Redeemed,
    /// The acceptor explicitly refused before activation
    /// (`RejectContract`), or the proposal expired before the
    /// acceptor confirmed (`expires_tick` lapsed).
    Rejected,
}

impl ContractStatus {
    /// Wire / SQL form. Must stay in lock-step with the SQL
    /// `CHECK` constraint declared in
    /// `rust/migrations/20260614000005_contracts.sql`.
    pub fn as_str(self) -> &'static str {
        match self {
            ContractStatus::Proposed => "PROPOSED",
            ContractStatus::Active => "ACTIVE",
            ContractStatus::Terminated => "TERMINATED",
            ContractStatus::Redeemed => "REDEEMED",
            ContractStatus::Rejected => "REJECTED",
        }
    }

    /// Parse the wire form back into the enum. Returns `None`
    /// for unknown strings (the SQL CHECK constraint should
    /// prevent this; defensive on the read path).
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "PROPOSED" => Some(ContractStatus::Proposed),
            "ACTIVE" => Some(ContractStatus::Active),
            "TERMINATED" => Some(ContractStatus::Terminated),
            "REDEEMED" => Some(ContractStatus::Redeemed),
            "REJECTED" => Some(ContractStatus::Rejected),
            _ => None,
        }
    }

    /// Audit-log op suffix used by the gRPC layer (e.g.
    /// `contract.propose`, `contract.redeem`). Concatenated
    /// with the `contract.` prefix by the service so the
    /// `AuditService` can dispatch on a single namespace.
    pub fn audit_op(self) -> &'static str {
        match self {
            ContractStatus::Proposed => "contract.propose",
            ContractStatus::Active => "contract.activate",
            ContractStatus::Terminated => "contract.terminate",
            ContractStatus::Redeemed => "contract.redeem",
            ContractStatus::Rejected => "contract.reject",
        }
    }

    /// True if the contract is in a "still going" state — used
    /// by the gRPC layer to reject mutations on closed rows
    /// (e.g. terminate a `REJECTED` contract).
    pub fn is_open(self) -> bool {
        matches!(self, ContractStatus::Proposed | ContractStatus::Active)
    }
}

// ── PayoutReason ────────────────────────────────────────────────────────────

/// Why a `ContractPayout` was emitted. The wire form is what
/// `contract_payouts.reason` stores; the SQL `CHECK` constraint
/// (`reason IN (...)`) is open-ended today (VARCHAR(64)) but the
/// Rust side sticks to the closed set below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PayoutReason {
    /// Daily tribute from slave to master (09 §3.2). Emitted
    /// by the scheduler, not by a direct user RPC.
    DailyTribute,
    /// `RedeemContract` triggered a one-shot payout equal to
    /// `redemption_cost` (09 §3.3).
    Redemption,
    /// Custom reason supplied by KubeJS / admin command. The
    /// string form is `"CUSTOM"` with the actual rationale
    /// carried in `BankTransaction.notes`.
    Custom,
}

impl PayoutReason {
    pub fn as_str(self) -> &'static str {
        match self {
            PayoutReason::DailyTribute => "DAILY_TRIBUTE",
            PayoutReason::Redemption => "REDEMPTION",
            PayoutReason::Custom => "CUSTOM",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "DAILY_TRIBUTE" => Some(PayoutReason::DailyTribute),
            "REDEMPTION" => Some(PayoutReason::Redemption),
            "CUSTOM" => Some(PayoutReason::Custom),
            _ => None,
        }
    }
}

// ── Contract ────────────────────────────────────────────────────────────────

/// One row of the `contracts` table. Mirrors the proto
/// `ContractResponse` message shape (see
/// `doc/14-rust-services.md` §3.2).
///
/// Naming: the canonical Rust field names follow the user task
/// spec (`proposer_uuid` / `acceptor_uuid`). The proto wire
/// shape is `master_uuid` / `slave_uuid`. The gRPC layer maps
/// at the request / response boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct Contract {
    /// Primary key (`contracts.contract_id`).
    pub contract_id: Uuid,
    /// The party proposing the contract. Equivalent to the
    /// proto `master_uuid` (`ContractResponse.master`).
    pub proposer_uuid: Uuid,
    /// The party accepting (or rejecting) the contract.
    /// Equivalent to the proto `slave_uuid`
    /// (`ContractResponse.slave`).
    pub acceptor_uuid: Uuid,
    /// Current lifecycle state (09 §2.1).
    pub status: ContractStatus,
    /// Terms kind discriminator. One of:
    /// `"DAILY_TRIBUTE"` / `"BODY_PLEDGE"` / `"CUSTOM"`.
    /// Mirrored into `contracts.terms_type` (kept as a free
    /// VARCHAR(32) on the SQL side so KubeJS scripts can
    /// inject new types without a migration — see 09 §2.2).
    pub terms_type: String,
    /// Terms payload serialised to JSON text. Persisted as
    /// `contracts.terms_json` (JSONB column). The schema is
    /// open-ended per 09 §2.2: `{"type": ..., "parameters": {…}}`.
    pub terms_json: String,
    /// Master-side revenue share, `0..=100`. Mirrored into
    /// `contracts.revenue_share_pct`; CHECK constraint
    /// enforces the same range.
    pub revenue_share_pct: f32,
    /// Cat-grass units the slave must pay to redeem (09 §3.3).
    /// Mirrored into `contracts.redemption_cost`. Defaults
    /// to 0 (contract cannot be redeemed via the standard
    /// payout path — caller may still `Terminate`).
    pub redemption_cost: i64,
    /// Tick at which the proposal auto-`REJECT`s if the
    /// acceptor hasn't confirmed. `None` = no expiry. Mirrored
    /// into `contracts.expires_tick`.
    pub expires_tick: Option<i64>,
    /// Server tick (ms) of the initial `INSERT`.
    pub created_tick: i64,
    /// Server tick (ms) of the most recent mutation.
    pub updated_tick: i64,
    /// Server tick (ms) at which `AcceptContract` flipped the
    /// status to `ACTIVE`. `None` while still `PROPOSED`.
    pub activated_tick: Option<i64>,
    /// Server tick (ms) at which the contract entered
    /// `TERMINATED`. Set by `terminate_contract` or by the
    /// expiry sweeper.
    pub terminated_tick: Option<i64>,
    /// Server tick (ms) at which the contract was
    /// `REDEEMED`. Set by `redeem_contract`.
    pub redeemed_tick: Option<i64>,
    /// Free-form reason captured on `REJECTED` /
    /// `TERMINATED`. Persisted as `contracts.reason`
    /// (VARCHAR(256)). The Java UI shows it on the contract
    /// detail page; the KubeJS binding surfaces it on
    /// `events.onContractTerminated`.
    pub reason: Option<String>,
}

impl Contract {
    /// Is the contract still open (i.e. can it be activated,
    /// rejected, terminated, or redeemed)? Wraps
    /// [`ContractStatus::is_open`] for call-site readability.
    pub fn is_open(&self) -> bool {
        self.status.is_open()
    }

    /// Has the proposal expired by `now_tick`? Returns `false`
    /// if `expires_tick` is `None` (no expiry) or `now_tick` is
    /// still before the deadline.
    pub fn is_expired(&self, now_tick: i64) -> bool {
        match self.expires_tick {
            Some(deadline) => now_tick >= deadline,
            None => false,
        }
    }

    /// Validate the mutable numeric / textual fields. Called by
    /// the lifecycle helpers before mutating a row. The
    /// repository layer re-asserts the constraints on write
    /// (the SQL CHECKs are the last line of defence).
    pub fn validate(&self) -> Result<(), ContractError> {
        if !(0.0..=100.0).contains(&self.revenue_share_pct) {
            return Err(ContractError::RevenueShareOutOfRange {
                got: self.revenue_share_pct,
            });
        }
        if self.redemption_cost < 0 {
            return Err(ContractError::NegativeRedemptionCost {
                got: self.redemption_cost,
            });
        }
        if self.proposer_uuid == self.acceptor_uuid {
            return Err(ContractError::SelfContract {
                who: self.proposer_uuid,
            });
        }
        if self.terms_type.is_empty() {
            return Err(ContractError::EmptyTermsType);
        }
        Ok(())
    }
}

// ── ContractPayout ──────────────────────────────────────────────────────────

/// One row of the `contract_payouts` table (09 §5.1).
///
/// Mirrors the proto shape as closely as the wire protocol
/// allows: the proto doesn't carry a dedicated payout message
/// (the events piggy-back on `ContractResponse.event_meta`),
/// but the durable row mirrors what the gRPC layer fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractPayout {
    /// Primary key (`contract_payouts.payout_id`).
    pub payout_id: Uuid,
    /// Owning contract (`contract_payouts.contract_id`,
    /// FK to `contracts.contract_id`).
    pub contract_id: Uuid,
    /// `bank_accounts.account_uuid` of the debit side. For
    /// tribute / redemption this is the slave (acceptor);
    /// for negative-space flows it can be the proposer.
    pub from_account: Uuid,
    /// `bank_accounts.account_uuid` of the credit side.
    /// Tribute / redemption credit the master (proposer).
    pub to_account: Uuid,
    /// Amount moved, always strictly positive. The direction
    /// is encoded by the from/to pair. CHECK constraint
    /// `amount > 0` enforced at the SQL layer.
    pub amount: i64,
    /// Why this payout was emitted (`contract_payouts.reason`).
    pub reason: PayoutReason,
    /// Server tick (ms) when the bank transfer committed.
    pub tick_millis: i64,
    /// Idempotency key. Mirrors `bank_transactions.request_id`
    /// for the underlying transfer; also the unique-index
    /// `idx_contract_payouts_request_id` dedupes against
    /// replayed RPCs (09 §5.3 + 99 §2.2).
    pub request_id: Uuid,
}

// ── ContractError ───────────────────────────────────────────────────────────

#[derive(Debug, Error, PartialEq)]
pub enum ContractError {
    #[error("proposer and acceptor must differ, got {who}")]
    SelfContract { who: Uuid },

    #[error("revenue_share_pct must be in [0, 100], got {got}")]
    RevenueShareOutOfRange { got: f32 },

    #[error("redemption_cost must be non-negative, got {got}")]
    NegativeRedemptionCost { got: i64 },

    #[error("terms_type is empty")]
    EmptyTermsType,

    #[error("contract {contract_id} not found")]
    NotFound { contract_id: Uuid },

    /// Lifecycle state machine rejection. Most common cause:
    /// trying to terminate a `REJECTED` / `REDEEMED` row.
    #[error("contract {contract_id} is in state {current:?}, cannot {action}")]
    InvalidTransition {
        contract_id: Uuid,
        current: ContractStatus,
        action: &'static str,
    },

    /// The caller of `accept_contract` / `reject_contract` /
    /// `terminate_contract` is not the contract party the
    /// lifecycle helper expects.
    #[error("caller {caller} is not the {expected} of contract {contract_id}")]
    NotParty {
        contract_id: Uuid,
        caller: Uuid,
        expected: &'static str,
    },

    #[error("contract {contract_id} expired at tick {expires_tick}")]
    Expired {
        contract_id: Uuid,
        expires_tick: i64,
    },
}

// ── Sanity tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn base_contract() -> Contract {
        Contract {
            contract_id: Uuid::new_v4(),
            proposer_uuid: Uuid::new_v4(),
            acceptor_uuid: Uuid::new_v4(),
            status: ContractStatus::Proposed,
            terms_type: "DAILY_TRIBUTE".to_owned(),
            terms_json: r#"{"type":"DAILY_TRIBUTE","parameters":{}}"#.to_owned(),
            revenue_share_pct: 10.0,
            redemption_cost: 100,
            expires_tick: None,
            created_tick: 0,
            updated_tick: 0,
            activated_tick: None,
            terminated_tick: None,
            redeemed_tick: None,
            reason: None,
        }
    }

    #[test]
    fn status_round_trips_wire_form() {
        for s in [
            ContractStatus::Proposed,
            ContractStatus::Active,
            ContractStatus::Terminated,
            ContractStatus::Redeemed,
            ContractStatus::Rejected,
        ] {
            assert_eq!(ContractStatus::from_wire(s.as_str()), Some(s));
        }
        assert_eq!(ContractStatus::from_wire("NOPE"), None);
    }

    #[test]
    fn payout_reason_round_trips_wire_form() {
        for r in [
            PayoutReason::DailyTribute,
            PayoutReason::Redemption,
            PayoutReason::Custom,
        ] {
            assert_eq!(PayoutReason::from_wire(r.as_str()), Some(r));
        }
        assert_eq!(PayoutReason::from_wire("NOPE"), None);
    }

    #[test]
    fn is_open_matches_open_states() {
        let mut c = base_contract();
        for s in [
            ContractStatus::Proposed,
            ContractStatus::Active,
            ContractStatus::Terminated,
            ContractStatus::Redeemed,
            ContractStatus::Rejected,
        ] {
            c.status = s;
            assert_eq!(c.is_open(), s.is_open());
        }
        assert!(ContractStatus::Proposed.is_open());
        assert!(ContractStatus::Active.is_open());
        assert!(!ContractStatus::Terminated.is_open());
        assert!(!ContractStatus::Redeemed.is_open());
        assert!(!ContractStatus::Rejected.is_open());
    }

    #[test]
    fn is_expired_only_when_deadline_passed() {
        let mut c = base_contract();
        c.expires_tick = Some(100);
        assert!(!c.is_expired(99));
        assert!(c.is_expired(100));
        assert!(c.is_expired(101));
        c.expires_tick = None;
        assert!(!c.is_expired(i64::MAX));
    }

    #[test]
    fn validate_rejects_bad_inputs() {
        let mut c = base_contract();
        c.revenue_share_pct = 100.1;
        assert!(matches!(
            c.validate(),
            Err(ContractError::RevenueShareOutOfRange { .. })
        ));
        c.revenue_share_pct = -0.1;
        assert!(matches!(
            c.validate(),
            Err(ContractError::RevenueShareOutOfRange { .. })
        ));
        c.revenue_share_pct = 0.0;
        c.redemption_cost = -1;
        assert!(matches!(
            c.validate(),
            Err(ContractError::NegativeRedemptionCost { .. })
        ));
        c.redemption_cost = 0;
        let same = c.proposer_uuid;
        c.acceptor_uuid = same;
        assert!(matches!(c.validate(), Err(ContractError::SelfContract { .. })));
        c.acceptor_uuid = Uuid::new_v4();
        c.terms_type.clear();
        assert!(matches!(c.validate(), Err(ContractError::EmptyTermsType)));
    }
}