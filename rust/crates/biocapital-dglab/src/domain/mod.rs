//! Domain types for the DG_LAB integration (10 §1-§4).
//!
//! Source of truth for the Rust side: `doc/10-hardware-dglab.md` +
//! `rust/proto/biocapital.proto` (`biocapital.v1.DglabService`).
//!
//! The three submodules are kept thin and self-contained so the
//! gRPC service, the WebSocket client, and the PG repository can
//! share the same shapes without a circular dep on `biocapital-pg`.

pub mod connection;
pub mod strength;
pub mod token;

pub use connection::{DglabConnection, DglabError, DglabState};
pub use strength::{StrengthSource, StrengthState, Waveform};
pub use token::DglabToken;
