//! Domain types for the slave-contract system (`doc/09-contracts.md`).
//!
//! Source of truth for the Rust side: `doc/09-contracts.md` +
//! `rust/proto/biocapital.proto` (`biocapital.v1.ContractService`).
//!
//! The contract crate owns the lifecycle state machine
//! (`propose_contract` / `accept_contract` / `reject_contract` /
//! `terminate_contract` / `redeem_contract`) plus the pure
//! `compute_payout` helper. Persistence and gRPC layers depend on
//! this module; this module depends only on `biocapital-bank` for
//! transfer validation and on `uuid` / `thiserror` / `serde_json`.
//!
//! Field naming note: the canonical domain names follow the user
//! task spec (`proposer_uuid` / `acceptor_uuid`); the proto wire
//! shape uses `master_uuid` / `slave_uuid`. The gRPC layer maps
//! between the two at the request / response boundary.

pub mod contract;
pub mod lifecycle;

pub use contract::{Contract, ContractError, ContractPayout, ContractStatus, PayoutReason};
pub use lifecycle::{
    accept_contract, compute_payout, propose_contract, reject_contract, redeem_contract,
    terminate_contract,
};