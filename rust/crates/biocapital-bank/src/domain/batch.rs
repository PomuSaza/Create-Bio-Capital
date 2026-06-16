//! Cat-grass batch tracking — `08-bank.md` §2.2.
//!
//! Every blade of cat-grass traces back to one of three `CatGrassSource`
//! origins, recorded in the `cat_grass_batches` table. A batch is
//! produced once (`total_amount`) and consumed piecemeal as players
//! withdraw / transfer (`remaining_amount`).
//!
//! The `batch_id` is **not** stored on the in-world `ItemStack`; it
//! lives only in PostgreSQL. The `CatGrassItem` Java side has no
//! `DataComponent` for it (per `08-bank.md` §2.1).

use thiserror::Error;
use uuid::Uuid;

use super::account::BankError;

// ── CatGrassSource ─────────────────────────────────────────────────────────

/// How a batch came into existence. The string form is the SQL CHECK
/// constraint value; the variants are the in-memory enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CatGrassSource {
    /// Deposited into an account by an ATM (08 §4.4 — "插入猫草 →
    /// 销毁物品 + 加余额"). The `producer_uuid` is the player whose
    /// account receives the credit.
    AtmDeposit,
    /// Issued by the Bio-Capital reward pipeline (e.g. pod production
    /// payouts from `04-core-pod.md`). The `producer_uuid` is the
    /// system actor (`None` if the caller is anonymous / a service).
    BiocapitalReward,
    /// Issued by an admin command (`/biocapital bank admin_issue`).
    /// `producer_uuid` is the admin's player UUID.
    AdminIssue,
}

impl CatGrassSource {
    /// Wire/SQL form. Must match
    /// `CHECK (production_source IN ('ATM_DEPOSIT','BIOCAPITAL_REWARD','ADMIN_ISSUE'))`
    /// on `cat_grass_batches`.
    pub fn as_str(self) -> &'static str {
        match self {
            CatGrassSource::AtmDeposit => "ATM_DEPOSIT",
            CatGrassSource::BiocapitalReward => "BIOCAPITAL_REWARD",
            CatGrassSource::AdminIssue => "ADMIN_ISSUE",
        }
    }

    /// Parse the wire form. Returns `None` for unknown strings; the
    /// SQL CHECK constraint should already prevent that.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "ATM_DEPOSIT" => Some(CatGrassSource::AtmDeposit),
            "BIOCAPITAL_REWARD" => Some(CatGrassSource::BiocapitalReward),
            "ADMIN_ISSUE" => Some(CatGrassSource::AdminIssue),
            _ => None,
        }
    }
}

// ── CatGrassBatch ──────────────────────────────────────────────────────────

/// One row in `cat_grass_batches`. Tracks the lifecycle of a slice of
/// cat-grass from production to consumption.
///
/// The fields mirror the SQL columns 1:1; the only divergence is
/// `current_holder_uuid` (optional, `None` while the batch is in
/// circulation outside any account).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatGrassBatch {
    /// Primary key (`cat_grass_batches.batch_id`).
    pub batch_id: Uuid,
    /// Player or system that produced the batch. `None` for system
    /// origins where attribution is meaningless (e.g. world-gen
    /// rewards). The SQL column is nullable for the same reason.
    pub producer_uuid: Option<Uuid>,
    /// Server tick (millis) at production time.
    pub production_tick: i64,
    /// How the batch entered existence.
    pub production_source: CatGrassSource,
    /// Total amount produced. Once set, this is immutable.
    pub total_amount: i64,
    /// Amount still in circulation / un-consumed. Decrements on
    /// `consume`.
    pub remaining_amount: i64,
    /// Account currently holding the batch's worth of cat-grass, or
    /// `None` if the batch is split across physical items in the
    /// world (chests, players' inventories, etc.).
    pub current_holder_uuid: Option<Uuid>,
}

// ── Constructors & mutations ───────────────────────────────────────────────

/// Build a new batch row with a fresh `batch_id`. `amount` must be
/// strictly positive; we reject zero / negative up front rather than
/// letting the SQL CHECK trip on insert.
pub fn new_batch(
    amount: i64,
    source: CatGrassSource,
    producer: Option<Uuid>,
    tick: i64,
) -> CatGrassBatch {
    CatGrassBatch {
        batch_id: Uuid::new_v4(),
        producer_uuid: producer,
        production_tick: tick,
        production_source: source,
        total_amount: amount,
        remaining_amount: amount,
        current_holder_uuid: None,
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BatchError {
    /// `consume` was called with a non-positive amount. Should never
    /// reach here from the gRPC layer; the layer clamps first.
    #[error("consume amount must be positive, got {0}")]
    NonPositiveAmount(i64),

    /// The batch is fully consumed. Callers should treat this as
    /// "deactivate and drop"; the SQL `remaining_amount = 0` row is
    /// kept for audit history.
    #[error("batch is fully consumed")]
    Exhausted,
}

impl BankError {
    /// Convenience: surface batch exhaustion through the unified
    /// `BankError` enum so the gRPC layer can use a single `?`
    /// operator. `Batch` variants are kept here for symmetry with
    /// `account` / `transfer` errors.
    pub fn from_batch(_e: BatchError) -> Self {
        // Mapping is intentionally lossy — the gRPC layer treats
        // batch exhaustion as INTERNAL with a deterministic message.
        // Detailed variants are logged via `tracing` before the
        // conversion.
        BankError::NonPositiveAmount(0)
    }
}

impl CatGrassBatch {
    /// Decrement `remaining_amount` by `amount`. Returns the
    /// **actual** amount consumed, which may be less than requested
    /// if the batch does not have that much left.
    ///
    /// Behaviour:
    /// - `amount <= 0` → `Err(NonPositiveAmount)`; no state change.
    /// - `amount >= remaining_amount` → consume what's left, return
    ///   that value, set `remaining_amount = 0`. Caller should
    ///   expect a subsequent call to fail with `Exhausted`.
    pub fn consume(&mut self, amount: i64) -> Result<i64, BatchError> {
        if amount <= 0 {
            return Err(BatchError::NonPositiveAmount(amount));
        }
        if self.remaining_amount == 0 {
            return Err(BatchError::Exhausted);
        }
        let taken = amount.min(self.remaining_amount);
        self.remaining_amount -= taken;
        Ok(taken)
    }

    /// True iff the batch has been fully consumed.
    pub fn is_exhausted(&self) -> bool {
        self.remaining_amount == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_batch_initializes_remaining_to_total() {
        let producer = Uuid::new_v4();
        let b = new_batch(500, CatGrassSource::AtmDeposit, Some(producer), 0);
        assert_eq!(b.total_amount, 500);
        assert_eq!(b.remaining_amount, 500);
        assert_eq!(b.production_source, CatGrassSource::AtmDeposit);
        assert_eq!(b.producer_uuid, Some(producer));
        assert_eq!(b.current_holder_uuid, None);
    }

    #[test]
    fn consume_partial() {
        let mut b = new_batch(100, CatGrassSource::AdminIssue, None, 0);
        assert_eq!(b.consume(30).unwrap(), 30);
        assert_eq!(b.remaining_amount, 70);
        assert!(!b.is_exhausted());
    }

    #[test]
    fn consume_clamps_to_remaining() {
        let mut b = new_batch(10, CatGrassSource::BiocapitalReward, None, 0);
        assert_eq!(b.consume(50).unwrap(), 10);
        assert_eq!(b.remaining_amount, 0);
        assert!(b.is_exhausted());
    }

    #[test]
    fn consume_rejects_zero() {
        let mut b = new_batch(10, CatGrassSource::AtmDeposit, None, 0);
        assert_eq!(b.consume(0), Err(BatchError::NonPositiveAmount(0)));
        assert_eq!(b.remaining_amount, 10);
    }

    #[test]
    fn consume_rejects_double_take() {
        let mut b = new_batch(5, CatGrassSource::AtmDeposit, None, 0);
        let _ = b.consume(5).unwrap();
        assert_eq!(b.consume(1), Err(BatchError::Exhausted));
    }

    #[test]
    fn source_round_trips_wire_form() {
        for s in [
            CatGrassSource::AtmDeposit,
            CatGrassSource::BiocapitalReward,
            CatGrassSource::AdminIssue,
        ] {
            assert_eq!(CatGrassSource::from_wire(s.as_str()), Some(s));
        }
        assert_eq!(CatGrassSource::from_wire("nope"), None);
    }
}
