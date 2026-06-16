//! `biocapital-dglab` — DG_LAB hardware integration (10 §1-§4).
//!
//! 2026-06-14 task #96 architectural correction: the Rust
//! server is the **WebSocket SERVER**; the DG_LAB phone app
//! connects **to us** (verified against the local
//! DGLabCraft-1.21.1-1.0.6.jar — see
//! `~/.claude/projects/.../memory/dglab-protocol-extracted.md`).
//!
//! Module map:
//! - [`domain`]              — `DglabToken` / `StrengthState` /
//!                            `DglabConnection` / `DglabState`
//!                            (single-connection-per-token
//!                            registry)
//! - [`waveform`]            — 15 [`WaveformType`] variants
//!                            (doc/10 §2.6)
//! - [`scheduler`]           — `EffectSource` 4-value priority
//!                            + lease-based single-slot scheduler
//! - [`ws_server`]           — `tokio-tungstenite` **server** +
//!                            message routing

pub mod domain;
pub mod scheduler;
pub mod waveform;
pub mod ws_server;

pub use domain::{
    DglabConnection, DglabError, DglabState, DglabToken, StrengthSource, StrengthState,
    Waveform,
};
pub use scheduler::{EffectRequest, EffectScheduler, EffectSource};
pub use waveform::WaveformType;
pub use ws_server::{ChannelState, DglabWsConfig, DglabWsServer};
