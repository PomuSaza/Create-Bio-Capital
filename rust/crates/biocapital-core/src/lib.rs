// `biocapital-core` — authoritative data structures and business rules for the
// Rust side of the Create: Bio-Capital server. Implementations live in
// per-module files; this top-level `lib.rs` simply re-exports the public API.
//
// See:
//   - `doc/02-player-state.md` for `player_state`
//   - `doc/03-body-development.md` for the 12-value BodyPart enum
//   - `doc/05-byproducts-fluids.md` for `fluids` (4 fluid enum + effect types)
//   - `doc/14-rust-services.md` for the overall Rust architecture

pub mod cache;
pub mod fluids;
pub mod player_state;

pub use cache::{LoadError, PlayerStateCache, PlayerStateLoader, DEFAULT_TTL};
pub use fluids::{
    BiocapitalFluid, FluidEffect, FluidEffectType, FluidEffectTypeParseError,
    FluidParseError, FluidSource, FluidSourceParseError,
};
pub use player_state::{
    BodyPart, BodyPartParseError, ClampResult, Outcome, PartChange, PartDevError,
    PlayerStateSnapshot, DEFAULT_HIDDEN_HP, DEFAULT_MAX_HUNGER, DEFAULT_STAT_MAX,
    HIDDEN_HP_FLOOR, SPAWN_HIDDEN_HP, SPAWN_HUNGER_RUST, SPAWN_PLEASURE, STAT_MIN,
};

/// Placeholder kept for callers that may still depend on a no-op symbol.
/// New code should use the typed re-exports above.
pub fn placeholder() {}