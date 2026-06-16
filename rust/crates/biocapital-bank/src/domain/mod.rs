//! Bank domain types — pure logic, no I/O.
//!
//! This module is the `biocapital-bank` analogue of
//! `biocapital_core::player_state`: domain types and validators
//! independent of PostgreSQL, gRPC, or JNI. Persistence and RPC
//! layers depend on this module; this module depends on nothing but
//! `uuid` and `thiserror`.

pub mod account;
pub mod batch;
pub mod hardware_token;
pub mod transfer;
pub mod whitelist;

pub use account::{
    BankAccount, BankError, BankOp, BankTransaction, HISTORY_SIZE, MAX_BALANCE,
};
pub use batch::{new_batch, BatchError, CatGrassBatch, CatGrassSource};
pub use hardware_token::{
    HardwareToken, HardwareTokenError, HardwareTokenStatus, HARDWARE_TOKEN_EXPIRY_DAYS,
    HARDWARE_TOKEN_MAX_SLOTS,
};
pub use transfer::{validate_deposit, validate_transfer, validate_withdraw};
pub use whitelist::{Whitelist, WhitelistError, WhitelistToml};
