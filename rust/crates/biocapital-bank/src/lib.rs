//! `biocapital-bank` — bank ledger logic per `doc/08-bank.md` (PRIORITY).
//!
//! The crate is split into two layers:
//!
//! - `domain` — pure types and validators (no I/O). Mirrors
//!   `biocapital_core::player_state` and is the dependency target
//!   for the gRPC / PG / JNI layers.
//! - `pg` (in `biocapital-pg::bank`) — sqlx-backed repositories.
//! - `grpc` (in `biocapital-grpc::bank_service`) — tonic gRPC
//!   server. Consumes the domain types via this crate.
//!
//! The Java side (`mo.dystopia.biocapital.bank.BankManager`) is the
//! grey-decommission baseline; new writes go through the Rust path
//! once `NativeRustBindings.callBank` returns a non-empty response.

pub mod domain;

pub use domain::{
    validate_deposit, validate_transfer, validate_withdraw, BankAccount, BankError,
    BankOp, BankTransaction, CatGrassBatch, CatGrassSource, HardwareToken,
    HardwareTokenError, HardwareTokenStatus, HISTORY_SIZE, MAX_BALANCE,
    HARDWARE_TOKEN_EXPIRY_DAYS, HARDWARE_TOKEN_MAX_SLOTS, Whitelist, WhitelistError,
    WhitelistToml,
};

/// Placeholder kept for callers that may still depend on a no-op
/// symbol. New code should use the typed re-exports above.
pub fn placeholder() {}
