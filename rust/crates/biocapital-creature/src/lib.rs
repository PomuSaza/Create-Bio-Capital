//! `biocapital-creature` — bio-customization + hostile-mob
//! replacement domain types (per `doc/13-bio-customization.md` +
//! `doc/06-hostile-mobs.md`).
//!
//! Public surface:
//! - `domain::creature` — `CreatureConfig` / `ModelSource` /
//!   `DropEntry` / `StatOverrides` and friends (the `creatures.json`
//!   schema). The full `creature_configs` table + hot-reload land
//!   in task #11; the types are in place now so the gRPC layer
//!   can start wiring `GetCreature` etc.
//! - `domain::mob_replacement` — the `mob_replacements` row type
//!   used by `HostileMobService.GetDropChance`. The repository +
//!   migration land in task #9.
//! - `hot_reload` — `CreatureHotReloader` + `Clock` seam +
//!   `ReloadReport` / `LoadOutcome` types. Drives the 5 s
//!   `config/biocapital/creatures/` watcher (13 §5).
//!
//! Task #9 wires `HostileMobService` (2 RPC) on top of these
//! types. The `CreatureService` (3 RPC) lands in task #11.

pub mod domain;
pub mod hot_reload;
pub mod pg;

pub use domain::{
    AudioClips, CreatureConfig, CreatureError, DropEntry, DropPartDevGate, ModelSource,
    ModelSourceParseError, ModelVariants, MobReplacement, MobReplacementError, StatOverrides,
    DEFAULT_DESIRE_FRAGMENT_CHANCE, MAX_DROP_STACK,
};
pub use hot_reload::{
    Clock, CreatureHotReloader, LoadOutcome, ReloadError, ReloadReport, SystemClock,
};

/// Placeholder kept for callers that may still depend on a
/// no-op symbol. New code should use the typed re-exports above.
pub fn placeholder() {}
