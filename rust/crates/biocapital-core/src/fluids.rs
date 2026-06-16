//! Fluid effect domain types — `doc/05-byproducts-fluids.md` §3 + §4.
//!
//! This module is the **authoritative** definition of the 4 biocapital fluids
//! and their effect payloads. The PG `fluid_effects` table (see
//! `rust/migrations/20260614000006_fluids.sql`) is the durable source of truth;
//! this file describes the in-memory shape and the wire-format helpers used by
//! the gRPC layer in `biocapital-grpc::player_state_service` (consumption
//! path) and `biocapital-grpc` routing into `EnvironmentService` (environmental
//! path; concrete `EnvironmentService` implementation lives in
//! `biocapital-environment`).
//!
//! Design decisions referenced:
//! - 4-value `BiocapitalFluid` enum (2026-06-14 task #8 spec; matches
//!   `doc/05-byproducts-fluids.md` §1 fluid list — Milk / Lactea is **not** a
//!   new fluid, it is a rename of `minecraft:milk` and is therefore out of
//!   scope here).
//! - 6-value `FluidEffectType` enum covering pleasure, hunger, body-part
//!   development, defeat-trigger, core-pod stress and a `DECORATIVE` no-op
//!   for `SuperLubricant` (00 §4 key design tradeoff).
//! - 3-value `FluidSource` enum (production / consumption / environment) so a
//!   single fluid can have multiple effect rows keyed by source — e.g. High
//!   Tide has both a CONSUMPTION pleasure effect and a PRODUCTION stress
//!   effect on the core pod.
//!
//! The Java side (`ModFluids.java`) registers the `Fluid` / `FluidType`
//! DeferredHolders for Create integration; this module deliberately does not
//! touch the Java types — the effect *truth* lives server-side and the Java
//! code is responsible for the world-rendering glue.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── BiocapitalFluid enum (4 values) ──────────────────────────────────────────

/// Authoritative list of the 4 mod-specific fluids per
/// `doc/05-byproducts-fluids.md` §1 (table) and `doc/05-byproducts-fluids.md`
/// §3 (per-fluid recipe notes).
///
/// The registration name (`as_str`) matches `ModFluids.java` and the Minecraft
/// registry path: `<namespace>:<path>` with the namespace fixed to
/// `create_biocapital`. Persistence shape:
/// - PG `fluid_effects.fluid` carries the path-only form (e.g. `high_tide`)
///   with a `CHECK (fluid IN (...))` constraint; the full namespace is added
///   in the Rust gRPC layer for wire compatibility with the Java registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BiocapitalFluid {
    /// 高潮流体 (High Tide). Produced by the core pod and collected from
    /// players in defeat state.
    HighTide,
    /// 超级润滑油 (Super Lubricant). Crafted by mixing High Tide + crude oil
    /// in a Mechanical Mixer. **Decorative** — does **not** alter Create
    /// machine RPM (00 §4 user-decision override of the original blueprint).
    SuperLubricant,
    /// 媚药水体 (Charm Potion). Crafted by mixing High Tide + lava in a
    /// Mechanical Mixer.
    CharmPotion,
    /// 精液 (Semen). Placeholder fluid (05 §3.4). Spawned from GENITAL part
    /// dev ≥ 30 + 30 s immersion in High Tide.
    Semen,
}

impl BiocapitalFluid {
    /// All 4 values, in stable declaration order.
    pub const ALL: [BiocapitalFluid; 4] = [
        BiocapitalFluid::HighTide,
        BiocapitalFluid::SuperLubricant,
        BiocapitalFluid::CharmPotion,
        BiocapitalFluid::Semen,
    ];

    /// Full Minecraft registration name (`create_biocapital:<path>`).
    /// Matches the Java `DeferredRegister.create(ForgeRegistries.FLUIDS, MODID)`
    /// calls in `ModFluids.java`.
    pub fn as_str(&self) -> &'static str {
        match self {
            BiocapitalFluid::HighTide => "create_biocapital:high_tide",
            BiocapitalFluid::SuperLubricant => "create_biocapital:super_lubricant",
            BiocapitalFluid::CharmPotion => "create_biocapital:charm_potion",
            BiocapitalFluid::Semen => "create_biocapital:semen",
        }
    }

    /// Short path form (`<path>` without namespace). Used as the value for
    /// `fluid_effects.fluid` CHECK constraint.
    pub fn as_path(&self) -> &'static str {
        match self {
            BiocapitalFluid::HighTide => "high_tide",
            BiocapitalFluid::SuperLubricant => "super_lubricant",
            BiocapitalFluid::CharmPotion => "charm_potion",
            BiocapitalFluid::Semen => "semen",
        }
    }

    /// `00 §4` key design override: Super Lubricant is purely decorative.
    /// It never alters Create's RPM cap and never modifies player state when
    /// an entity is immersed in it. The PG `fluid_effects` seed row for this
    /// fluid carries `effect_type = 'DECORATIVE'`.
    pub fn is_decorative(&self) -> bool {
        matches!(self, BiocapitalFluid::SuperLubricant)
    }
}

impl fmt::Display for BiocapitalFluid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_path())
    }
}

impl FromStr for BiocapitalFluid {
    type Err = FluidParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Accept both forms (path-only and full registration name) for
        // tolerance against caller-site drift.
        let trimmed = s
            .strip_prefix("create_biocapital:")
            .unwrap_or(s);
        match trimmed {
            "high_tide" => Ok(BiocapitalFluid::HighTide),
            "super_lubricant" => Ok(BiocapitalFluid::SuperLubricant),
            "charm_potion" => Ok(BiocapitalFluid::CharmPotion),
            "semen" => Ok(BiocapitalFluid::Semen),
            other => Err(FluidParseError(other.to_string())),
        }
    }
}

/// Error returned by `BiocapitalFluid::from_str` when the string does not
/// match any of the 4 canonical paths / registration names.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error(
    "unknown fluid name: {0} (expected one of high_tide/super_lubricant/charm_potion/semen, optionally prefixed create_biocapital:)"
)]
pub struct FluidParseError(pub String);

// ── FluidEffectType enum (6 values) ─────────────────────────────────────────

/// What an effect *does* when applied to a player.
///
/// The enum is split by *what* the effect modifies (a player stat, a body
/// part, a binary state machine trigger, a core-pod side effect) rather than
/// by the source. `DECORATIVE` is the no-op variant reserved for fluids that
/// must remain in the table (for the seed INSERTs and any future
/// player-authored overrides) but never actually mutate state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FluidEffectType {
    /// Add `magnitude` to `PlayerStateSnapshot.pleasure` (05 §4.1 / §4.2).
    /// Clamping is handled by `PlayerStateSnapshot::add_pleasure`.
    PleasureBoost,
    /// Add `magnitude` to `PlayerStateSnapshot.hunger` (05 §4.1).
    /// Clamping handled by `add_hunger`.
    HungerBoost,
    /// Add `magnitude` to the `GENITAL` body-part development (05 §4.4).
    /// Per 05 §3.4 the only body part touched by fluids is GENITAL; future
    /// extensions would carry a separate `part` column.
    PartDevBoost,
    /// Trigger the defeat-state entry path. The gRPC layer emits a
    /// `DefeatStateEnterEvent` after this effect fires (07 §6 + 99 §3.1).
    /// `magnitude` is reserved (0 for current flows).
    DefeatTrigger,
    /// Add `magnitude` to the producing core pod's stress output
    /// (`pod.tick` path; 05 §3.1 production source). Surfaces in the
    /// `audit_core_pod.stress_units` column.
    StressBoost,
    /// Pure-decoration no-op. `SuperLubricant` carries exactly one row of
    /// this type with `magnitude = 0` and any source. The gRPC layer
    /// short-circuits on `DECORATIVE` so no PlayerState mutation occurs.
    Decorative,
}

impl FluidEffectType {
    /// All 6 values, in stable declaration order.
    pub const ALL: [FluidEffectType; 6] = [
        FluidEffectType::PleasureBoost,
        FluidEffectType::HungerBoost,
        FluidEffectType::PartDevBoost,
        FluidEffectType::DefeatTrigger,
        FluidEffectType::StressBoost,
        FluidEffectType::Decorative,
    ];

    /// Wire name used by the PG `fluid_effects.effect_type` CHECK constraint
    /// and the audit rows in `audit_player_state`.
    pub fn as_str(&self) -> &'static str {
        match self {
            FluidEffectType::PleasureBoost => "PLEASURE_BOOST",
            FluidEffectType::HungerBoost => "HUNGER_BOOST",
            FluidEffectType::PartDevBoost => "PART_DEV_BOOST",
            FluidEffectType::DefeatTrigger => "DEFEAT_TRIGGER",
            FluidEffectType::StressBoost => "STRESS_BOOST",
            FluidEffectType::Decorative => "DECORATIVE",
        }
    }
}

impl fmt::Display for FluidEffectType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FluidEffectType {
    type Err = FluidEffectTypeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "PLEASURE_BOOST" => Ok(FluidEffectType::PleasureBoost),
            "HUNGER_BOOST" => Ok(FluidEffectType::HungerBoost),
            "PART_DEV_BOOST" => Ok(FluidEffectType::PartDevBoost),
            "DEFEAT_TRIGGER" => Ok(FluidEffectType::DefeatTrigger),
            "STRESS_BOOST" => Ok(FluidEffectType::StressBoost),
            "DECORATIVE" => Ok(FluidEffectType::Decorative),
            other => Err(FluidEffectTypeParseError(other.to_string())),
        }
    }
}

/// Error returned by `FluidEffectType::from_str`.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error(
    "unknown fluid effect type: {0} (expected one of PLEASURE_BOOST/HUNGER_BOOST/PART_DEV_BOOST/DEFEAT_TRIGGER/STRESS_BOOST/DECORATIVE)"
)]
pub struct FluidEffectTypeParseError(pub String);

// ── FluidSource enum (3 values) ─────────────────────────────────────────────

/// Where the effect originates. The gRPC layer uses this to route an effect
/// row to the right service:
/// - `Production` — core pod output path; lands in
///   `audit_core_pod.stress_units` (only `STRESS_BOOST` rows reach here).
/// - `Consumption` — player drank / bucket-applied the fluid; routed to
///   `PlayerStateService::add_fluid_effect`.
/// - `Environment` — entity is immersed in the fluid; routed to
///   `EnvironmentService::apply_fluid_effect_environmental`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FluidSource {
    Production,
    Consumption,
    Environment,
}

impl FluidSource {
    pub const ALL: [FluidSource; 3] = [
        FluidSource::Production,
        FluidSource::Consumption,
        FluidSource::Environment,
    ];

    /// Wire name used by the PG `fluid_effects.source` CHECK constraint.
    pub fn as_str(&self) -> &'static str {
        match self {
            FluidSource::Production => "PRODUCTION",
            FluidSource::Consumption => "CONSUMPTION",
            FluidSource::Environment => "ENVIRONMENT",
        }
    }
}

impl fmt::Display for FluidSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FluidSource {
    type Err = FluidSourceParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "PRODUCTION" => Ok(FluidSource::Production),
            "CONSUMPTION" => Ok(FluidSource::Consumption),
            "ENVIRONMENT" => Ok(FluidSource::Environment),
            other => Err(FluidSourceParseError(other.to_string())),
        }
    }
}

/// Error returned by `FluidSource::from_str`.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error(
    "unknown fluid source: {0} (expected one of PRODUCTION/CONSUMPTION/ENVIRONMENT)"
)]
pub struct FluidSourceParseError(pub String);

// ── FluidEffect (in-memory payload) ─────────────────────────────────────────

/// A single fluid effect row, as it appears in the PG `fluid_effects` table
/// and as it flows through the gRPC layer.
///
/// The row carries an `effect_id` UUID so the gRPC layer can produce
/// idempotent audit entries keyed on the row's identity rather than the
/// tuple `(fluid, effect_type, source)` — the same tuple may appear twice
/// for two different snapshots if a player triggers the same effect twice
/// within the same tick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FluidEffect {
    /// Stable per-row identifier (matches `fluid_effects.effect_id`).
    pub effect_id: uuid::Uuid,

    /// Which fluid this effect belongs to.
    pub fluid: BiocapitalFluid,

    /// What the effect does.
    pub effect_type: FluidEffectType,

    /// Strength of the effect. Always `0.0` for `DECORATIVE` /
    /// `DEFEAT_TRIGGER`; otherwise the magnitude is a raw float whose
    /// interpretation depends on `effect_type` (see the per-type enum
    /// comments).
    pub magnitude: f32,

    /// How long the effect lasts. `0` = instant (applies once and never
    /// re-fires). A positive value means the entity must remain immersed /
    /// consumable for that many server ticks before the effect ticks down.
    pub duration_ticks: i64,

    /// Where the effect originates. The gRPC layer dispatches on this.
    pub source: FluidSource,

    /// Server tick at which the row was inserted into PG. Used for audit
    /// ordering and for the seed INSERTs (`0`).
    pub created_tick: i64,
}

impl FluidEffect {
    /// Build a fresh `FluidEffect` with a random `effect_id`. Used by the
    /// PG repository when inserting new rows.
    pub fn new(
        fluid: BiocapitalFluid,
        effect_type: FluidEffectType,
        magnitude: f32,
        duration_ticks: i64,
        source: FluidSource,
        created_tick: i64,
    ) -> Self {
        Self {
            effect_id: uuid::Uuid::new_v4(),
            fluid,
            effect_type,
            magnitude,
            duration_ticks,
            source,
            created_tick,
        }
    }

    /// True if this effect mutates no player / pod state. The gRPC layer
    /// short-circuits on this.
    pub fn is_noop(&self) -> bool {
        matches!(self.effect_type, FluidEffectType::Decorative)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fluid_all_count_is_four() {
        assert_eq!(BiocapitalFluid::ALL.len(), 4);
    }

    #[test]
    fn effect_type_all_count_is_six() {
        assert_eq!(FluidEffectType::ALL.len(), 6);
    }

    #[test]
    fn source_all_count_is_three() {
        assert_eq!(FluidSource::ALL.len(), 3);
    }

    #[test]
    fn fluid_decorative_flag_matches_super_lubricant() {
        assert!(BiocapitalFluid::SuperLubricant.is_decorative());
        assert!(!BiocapitalFluid::HighTide.is_decorative());
        assert!(!BiocapitalFluid::CharmPotion.is_decorative());
        assert!(!BiocapitalFluid::Semen.is_decorative());
    }

    #[test]
    fn fluid_roundtrip_both_forms() {
        for f in BiocapitalFluid::ALL {
            let path = f.as_path();
            let from_path: BiocapitalFluid = path.parse().unwrap();
            let from_full: BiocapitalFluid = f.as_str().parse().unwrap();
            assert_eq!(from_path, f);
            assert_eq!(from_full, f);
        }
    }

    #[test]
    fn fluid_rejects_unknown() {
        assert!(BiocapitalFluid::from_str("lava").is_err());
        assert!(BiocapitalFluid::from_str("MILK").is_err()); // milk is minecraft:milk, not a biocapital fluid
        assert!(BiocapitalFluid::from_str("").is_err());
    }

    #[test]
    fn fluid_registration_names_match_java() {
        // Pin the 4 wire names so a Rust-side rename cannot drift away
        // from the Java `ModFluids.java` DeferredRegister calls.
        assert_eq!(BiocapitalFluid::HighTide.as_str(), "create_biocapital:high_tide");
        assert_eq!(
            BiocapitalFluid::SuperLubricant.as_str(),
            "create_biocapital:super_lubricant"
        );
        assert_eq!(
            BiocapitalFluid::CharmPotion.as_str(),
            "create_biocapital:charm_potion"
        );
        assert_eq!(BiocapitalFluid::Semen.as_str(), "create_biocapital:semen");
    }

    #[test]
    fn effect_type_roundtrip() {
        for t in FluidEffectType::ALL {
            let s = t.as_str();
            let back: FluidEffectType = s.parse().unwrap();
            assert_eq!(back, t);
        }
    }

    #[test]
    fn effect_type_serialises_to_screaming_snake() {
        let s = serde_json::to_string(&FluidEffectType::PartDevBoost).unwrap();
        assert_eq!(s, "\"PART_DEV_BOOST\"");
    }

    #[test]
    fn source_roundtrip() {
        for s in FluidSource::ALL {
            let back: FluidSource = s.as_str().parse().unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn fluid_effect_is_noop_matches_decorative() {
        let dec = FluidEffect::new(
            BiocapitalFluid::SuperLubricant,
            FluidEffectType::Decorative,
            0.0,
            0,
            FluidSource::Production,
            0,
        );
        assert!(dec.is_noop());

        let pleasure = FluidEffect::new(
            BiocapitalFluid::HighTide,
            FluidEffectType::PleasureBoost,
            5.0,
            200,
            FluidSource::Consumption,
            0,
        );
        assert!(!pleasure.is_noop());
    }
}