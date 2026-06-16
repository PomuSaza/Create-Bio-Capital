//! `biocapital-pod` — core-pod production logic per `doc/04-core-pod.md`.
//!
//! The crate is split into two layers, mirroring the pattern set by
//! `biocapital-bank` / `biocapital-core`:
//!
//! - `domain` — pure types and validators (no I/O). Includes
//!   [`domain::CorePod`], [`domain::FluidStack`],
//!   [`domain::ProductionFormula`], [`domain::tick_pod`],
//!   [`domain::enter_pod`], and [`domain::exit_pod`].
//! - [`compute`] — direct-call helpers consumed by the Sable JNI
//!   bridge. [`compute::compute_stress`] is the synchronous
//!   `host_uuid → stress_units` calculation that
//!   `CorePodBlockEntity.tick()` invokes every server tick (see
//!   `doc/16-sable-bridge.md` §3.4 and `doc/04-core-pod.md` §2.3).
//!
//! Persistence (`biocapital-pg::core_pod`) and the gRPC service
//! (`biocapital-grpc::core_pod_service`) consume the domain module
//! from this crate.

pub mod compute;
pub mod domain;

pub use compute::{compute_stress, STRESS_BASE, STRESS_DECAY};
pub use domain::{
    enter_pod, exit_pod, tick_pod, CorePod, FluidStack, PodEnterResult, PodError,
    PodExitResult, PodStatus, PodTickOutcome, ProductionFormula, INPUT_FLUID_MIN_ENTER_MB,
    MAX_HUNGER_THRESHOLD, RPM_DEFAULT,
};

/// Placeholder kept for callers that may still depend on a no-op
/// symbol. New code should use the typed re-exports above.
pub fn placeholder() {}