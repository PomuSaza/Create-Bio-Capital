//! `biocapital-creature` domain types.
//!
//! Per `doc/13-bio-customization.md` §6.1, the Rust side is the
//! **authoritative** parser + cache of `creatures.json` and of
//! the `mob_replacements` table. The Java side reads via
//! `CreatureService.GetCreature` / `HostileMobService.GetDropChance`
//! and never writes directly to either table.
//!
//! Task #9 scope (2026-06-14): the `MobReplacement` shape plus
//! the bare `CreatureConfig` skeleton. The full `creature_configs`
//! table and the `CreatureService.ReloadCreatures` hot-reload
//! path land in task #11.

pub mod creature;
pub mod mob_replacement;

pub use creature::{
    AudioClips, CreatureConfig, CreatureError, DropEntry, DropPartDevGate, ModelSource,
    ModelSourceParseError, ModelVariants, StatOverrides, DEFAULT_DESIRE_FRAGMENT_CHANCE,
    MAX_DROP_STACK,
};
pub use mob_replacement::{MobReplacement, MobReplacementError};
