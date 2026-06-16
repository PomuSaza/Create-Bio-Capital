//! `biocapital-contract` — slave-contract domain logic
//! (`doc/09-contracts.md`).
//!
//! Mirrors `biocapital-bank` in structure:
//!
//! - [`domain`]      — pure types + lifecycle transitions +
//!                     payout computation. No I/O. The
//!                     dependency target for the gRPC / PG /
//!                     JNI layers.
//! - `pg` (`biocapital-pg::contract`)      — sqlx-backed
//!   repositories. Wired by task #53.
//! - `grpc` (`biocapital-grpc::contract_service`) — tonic
//!   gRPC server. Wired by task #54.
//!
//! The Java side (`mo.dystopia.biocapital.bank.ContractManager`)
//! is the grey-decommission baseline; new writes go through
//! the Rust path once `NativeRustBindings.callContract` returns
//! a non-empty response (16 §3.4 dispatch protocol).
//!
//! Cross-crate wiring:
//! - `biocapital-bank` (path dep) — `compute_payout` returns a
//!   payout row the gRPC layer hands to `BankService.Transfer`.
//! - `biocapital-pg`              — owns the durable schema
//!   (`contracts` + `contract_payouts`).
//! - `biocapital-grpc`            — owns the 7-RPC surface.

pub mod domain;

pub use domain::{
    accept_contract, compute_payout, propose_contract, reject_contract, redeem_contract,
    terminate_contract, Contract, ContractError, ContractPayout, ContractStatus, PayoutReason,
};

/// Placeholder kept for callers that may still depend on a
/// no-op symbol. New code should use the typed re-exports
/// above.
pub fn placeholder() {}