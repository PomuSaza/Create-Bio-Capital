//! Hardware-token domain types — `HardwareToken`, `HardwareTokenStatus`,
//! and the 30-day / 3-slot constants from `doc/18-tg-whitelist.md` §1.2
//! + §5.1.
//!
//! Persistence lives in `biocapital-pg::hardware_token` (one row per
//! (owner, token) in `hardware_tokens`; FIFO replacement enforced by
//! a PG trigger). The gRPC layer exposes 5 RPCs in
//! `biocapital-grpc::bank_service` that drive the lifecycle.
//!
//! State machine (18 §6):
//!
//! ```text
//!        issued
//!           │
//!           ▼
//!        ┌────────┐  bind (玩家首次粘贴 token)  ┌────────┐
//!        │ ACTIVE │ ────────────────────────► │ BOUND  │
//!        └────────┘                            └────────┘
//!           │                                     │
//!           │ (未 bind 就过期)                     │ expires_at < now()
//!           ▼                                     ▼
//!        ┌────────┐                            ┌────────┐
//!        │EXPIRED │                            │EXPIRED │
//!        └────────┘                            └────────┘
//!                                                │
//!                                                │ 同 owner 第 4 个 token 触发 FIFO
//!                                                ▼
//!                                            ┌─────────┐
//!                                            │REPLACED │
//!                                            └─────────┘
//! ```
//!
//! Plus the orthogonal `REVOKED` state for the player / admin
//! "revoke" RPC. PG FIFO trigger ignores `REPLACED` / `REVOKED` /
//! `EXPIRED` when counting active slots.

use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

// ── Constants (doc/18 §1.2 + §5.1) ──────────────────────────────────────────

/// Per-player slot cap. The PG FIFO trigger counts `status NOT IN
/// ('REPLACED', 'REVOKED', 'EXPIRED')` and evicts the oldest
/// non-terminal row when the count reaches this number (18 §5.1 +
/// §5.2).
pub const HARDWARE_TOKEN_MAX_SLOTS: usize = 3;

/// Token lifetime in days. `issued_at + 30d = expires_at`. The
/// `expire_overdue` repository sweep marks anything past this as
/// `EXPIRED` (18 §3.3). Changing this constant is a wire-incompatible
/// change — the SQL column, the gRPC field, and the migration must
/// all move together.
pub const HARDWARE_TOKEN_EXPIRY_DAYS: i64 = 30;

// ── HardwareTokenStatus ─────────────────────────────────────────────────────

/// Lifecycle state. The wire form is the value stored in
/// `hardware_tokens.status` and surfaced in the audit
/// `op` column. Must stay in lock-step with the SQL CHECK
/// constraint and `audit_hardware_token.op` (99 §5.1.10-style
/// column list — to be added when the 99 §5.1.10 block lands).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HardwareTokenStatus {
    /// Issued but never bound.
    Active,
    /// Player pasted the token and the bind RPC succeeded.
    Bound,
    /// 30 days past `issued_at`. Set by the periodic sweep.
    Expired,
    /// Evicted by the FIFO trigger when a 4th token landed.
    Replaced,
    /// Explicitly revoked by the player / admin via `RevokeHardware`.
    Revoked,
}

impl HardwareTokenStatus {
    /// Wire/SQL form. Mirrors the SQL CHECK
    /// `(status IN ('ACTIVE','BOUND','EXPIRED','REPLACED','REVOKED'))`.
    pub fn as_str(self) -> &'static str {
        match self {
            HardwareTokenStatus::Active => "ACTIVE",
            HardwareTokenStatus::Bound => "BOUND",
            HardwareTokenStatus::Expired => "EXPIRED",
            HardwareTokenStatus::Replaced => "REPLACED",
            HardwareTokenStatus::Revoked => "REVOKED",
        }
    }

    /// Display impl required by `thiserror`'s `#[error("...{status}")]`
    /// interpolation in [`HardwareTokenError::NotBindable`]. We delegate
    /// to the wire form (uppercase) so error messages line up with the
    /// SQL CHECK vocabulary.
    pub fn as_display(&self) -> &'static str {
        self.as_str()
    }
}

/// `Display` for [`HardwareTokenStatus`] — hand-written so we don't
/// pull in `strum` / `derive_more`. Routes through [`Self::as_str`] so
/// the human-readable form is identical to the SQL wire form.
impl std::fmt::Display for HardwareTokenStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl HardwareTokenStatus {
    /// Parse the wire form back into the enum. Returns `None` for
    /// unknown strings (defensive — the SQL CHECK constraint should
    /// already prevent this).
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "ACTIVE" => Some(HardwareTokenStatus::Active),
            "BOUND" => Some(HardwareTokenStatus::Bound),
            "EXPIRED" => Some(HardwareTokenStatus::Expired),
            "REPLACED" => Some(HardwareTokenStatus::Replaced),
            "REVOKED" => Some(HardwareTokenStatus::Revoked),
            _ => None,
        }
    }

    /// True iff this state is terminal for the FIFO-slot count.
    /// The PG trigger and the Rust `list_for_owner` filter both
    /// use this predicate to ignore terminal rows when enforcing
    /// the 3-slot cap.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            HardwareTokenStatus::Expired
                | HardwareTokenStatus::Replaced
                | HardwareTokenStatus::Revoked
        )
    }

    /// True iff the token is "in play" — issued and not yet
    /// replaced/expired/revoked. Used by `find_valid_token` and by
    /// the gRPC `Authenticate` hot path.
    pub fn is_alive(self) -> bool {
        !self.is_terminal()
    }
}

// ── HardwareToken ───────────────────────────────────────────────────────────

/// One row in the `hardware_tokens` table. Mirrors the SQL column
/// list 1:1 (see `20260614000009_hardware_token.sql`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareToken {
    /// Primary key (`hardware_tokens.token_id`).
    pub token_id: Uuid,
    /// Player who owns the slot (`hardware_tokens.owner_uuid`).
    pub owner_uuid: Uuid,
    /// The SHA-256 + base32 hardware-id hash bound to this slot.
    /// `None` = issued but never bound (status = `Active`).
    /// `Some(hash)` = bound (status = `Bound`).
    pub hardware_id_hash: Option<String>,
    /// Lifecycle state.
    pub status: HardwareTokenStatus,
    /// When the slot was first minted.
    pub issued_at: DateTime<Utc>,
    /// `issued_at + 30d`. The sweep task marks this row `Expired`
    /// once `now() > expires_at`.
    pub expires_at: DateTime<Utc>,
    /// When the player first pasted the token. `None` for never-bound
    /// rows.
    pub bound_at: Option<DateTime<Utc>>,
    /// When the FIFO trigger evicted this row. `None` for live rows.
    pub replaced_at: Option<DateTime<Utc>>,
    /// When the player / admin explicitly revoked this row. `None`
    /// for live rows.
    pub revoked_at: Option<DateTime<Utc>>,
}

impl HardwareToken {
    /// Has the token's expiry window passed `now`? Pure logic —
    /// callers decide whether to act on it (the gRPC layer
    /// short-circuits on `true`).
    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        now > self.expires_at
    }

    /// True iff the token is currently usable for `Authenticate`.
    /// Combines `is_alive()` and `!is_expired_at(now)`. The gRPC
    /// `Authenticate` hot path calls this once per (owner,
    /// hardware_id_hash) match.
    pub fn is_valid_at(&self, now: DateTime<Utc>) -> bool {
        self.status.is_alive() && !self.is_expired_at(now)
    }

    /// Compute the canonical `expires_at` for a given `issued_at`.
    /// Used by the repository on `create_token`.
    pub fn compute_expires_at(issued_at: DateTime<Utc>) -> DateTime<Utc> {
        issued_at + chrono::Duration::days(HARDWARE_TOKEN_EXPIRY_DAYS)
    }
}

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HardwareTokenError {
    /// BindHardware RPC was called for a token_id that does not
    /// exist (or does not belong to the supplied owner_uuid).
    #[error("token {token_id} not found for owner {owner_uuid}")]
    TokenNotFound { token_id: Uuid, owner_uuid: Uuid },

    /// BindHardware was called for a token that is no longer in
    /// `Active` state (already bound / expired / replaced /
    /// revoked). The status is surfaced for the gRPC layer's
    /// "reason" mapping.
    #[error("token {token_id} is in terminal/non-bindable state {status}")]
    NotBindable {
        token_id: Uuid,
        status: HardwareTokenStatus,
    },

    /// BindHardware was called with a `hardware_id_hash` that does
    /// not match the expected 52-char base32 form (18 §4.2). The
    /// gRPC layer surfaces this as `INVALID_ARGUMENT`.
    #[error("hardware_id_hash must be 52 base32 chars, got length {0}")]
    InvalidHardwareIdHash(usize),

    /// `expires_at` in the past at issuance. The repository
    /// rejects this to keep `is_expired_at` monotonic.
    #[error("expires_at {expires_at} is not after issued_at {issued_at}")]
    ExpiresAtNotAfterIssued {
        issued_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    },

    /// Parse failed for the wire form. Mirrors the SQL CHECK
    /// "got unknown status string".
    #[error("invalid HardwareTokenStatus: {0}")]
    InvalidStatus(String),
}

// ── Sanity tests (no DB) ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn token(status: HardwareTokenStatus) -> HardwareToken {
        let now = Utc::now();
        HardwareToken {
            token_id: Uuid::new_v4(),
            owner_uuid: Uuid::new_v4(),
            hardware_id_hash: None,
            status,
            issued_at: now,
            expires_at: HardwareToken::compute_expires_at(now),
            bound_at: None,
            replaced_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn status_round_trips_wire_form() {
        for s in [
            HardwareTokenStatus::Active,
            HardwareTokenStatus::Bound,
            HardwareTokenStatus::Expired,
            HardwareTokenStatus::Replaced,
            HardwareTokenStatus::Revoked,
        ] {
            assert_eq!(HardwareTokenStatus::from_wire(s.as_str()), Some(s));
        }
        assert_eq!(HardwareTokenStatus::from_wire("NOPE"), None);
    }

    #[test]
    fn terminal_predicate_matches_doc() {
        assert!(!HardwareTokenStatus::Active.is_terminal());
        assert!(!HardwareTokenStatus::Bound.is_terminal());
        assert!(HardwareTokenStatus::Expired.is_terminal());
        assert!(HardwareTokenStatus::Replaced.is_terminal());
        assert!(HardwareTokenStatus::Revoked.is_terminal());
    }

    #[test]
    fn alive_predicate_inverts_terminal() {
        for s in [
            HardwareTokenStatus::Active,
            HardwareTokenStatus::Bound,
            HardwareTokenStatus::Expired,
            HardwareTokenStatus::Replaced,
            HardwareTokenStatus::Revoked,
        ] {
            assert_eq!(s.is_alive(), !s.is_terminal());
        }
    }

    #[test]
    fn compute_expires_at_adds_thirty_days() {
        let issued = Utc::now();
        let exp = HardwareToken::compute_expires_at(issued);
        let delta = exp.signed_duration_since(issued);
        assert_eq!(delta.num_days(), HARDWARE_TOKEN_EXPIRY_DAYS);
    }

    #[test]
    fn validity_window_falls_off_after_expires_at() {
        let now = Utc::now();
        let mut t = token(HardwareTokenStatus::Bound);
        t.expires_at = now + chrono::Duration::seconds(1);
        assert!(t.is_valid_at(now));
        assert!(!t.is_valid_at(now + chrono::Duration::seconds(2)));
    }

    #[test]
    fn terminal_status_is_never_valid() {
        for s in [
            HardwareTokenStatus::Expired,
            HardwareTokenStatus::Replaced,
            HardwareTokenStatus::Revoked,
        ] {
            assert!(!token(s).is_valid_at(Utc::now()));
        }
    }

    #[test]
    fn max_slots_is_three() {
        // Pinned constant — 18 §5.1.
        assert_eq!(HARDWARE_TOKEN_MAX_SLOTS, 3);
        assert_eq!(HARDWARE_TOKEN_EXPIRY_DAYS, 30);
    }
}
