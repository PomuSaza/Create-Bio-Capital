//! Core-pod domain types — pure logic, no I/O.
//!
//! See `doc/04-core-pod.md` for the design contract and
//! `doc/14-rust-services.md` §3.2 for the proto schema.
//!
//! Mirrors the pattern set by `biocapital-bank::domain`:
//! - `pod`     — the durable entity (`CorePod`, `FluidStack`, `ProductionFormula`).
//! - `production` — pure-function tick / production cycle.
//! - `enter_exit` — hosting state transitions and entry / exit validation.
//!
//! The `compute` module (sibling of `domain`) holds the Sable JNI
//! direct-call helper (`compute_stress`) that bypasses gRPC.

pub mod enter_exit;
pub mod pod;
pub mod production;

pub use enter_exit::{enter_pod, exit_pod, PodEnterResult, PodError, PodExitResult};
pub use pod::{
    CorePod, FluidStack, PodStatus, ProductionFormula, INPUT_FLUID_MIN_ENTER_MB,
    MAX_HUNGER_THRESHOLD,
};
pub use production::{tick_pod, PodTickOutcome, RPM_DEFAULT};