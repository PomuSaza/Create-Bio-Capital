//! Environment-effect domain types — `doc/07-environment.md` §2 + §3 + §4 + §5.
//!
//! This module is the **authoritative** in-memory shape for the 4
//! de-fatalised environment kinds the mod tracks:
//!
//! - `Lava`         — fluid immersion; +1.0 pleasure / sec (07 §2.2)
//! - `SwampMud`     — block contact; −2.0 hunger / sec, ×0.9 movement
//!   (07 §3.2; the doc gives "−0.5 hunger / 5 s" which reduces to
//!   the same magnitude)
//! - `Sand`         — block contact; −0.5 hunger / hour (desertification,
//!   07 §4.1)
//! - `MagmaBlock`   — block contact; +0.5 pleasure / sec (07 §5.1)
//! - `Fluid(String)` — escape hatch for the FLUID_<X> tokens routed
//!   by [`biocapital_environment::fluid_from_environment_token`]
//!   (task #8 fluid path, 05 §4 + 07 §8).
//!
//! The gRPC `EnvironmentService` dispatches the 4 canonical environments
//! to the rule table in this file; `Fluid(String)` is the FLUID_<X>
//! route that delegates to the existing
//! `apply_fluid_effect_environmental` path in `lib.rs` (task #8).
//!
//! Persistence: the durable mirror of `DEFAULT_ENVIRONMENT_RULES` lives
//! in the new `environment_default_rules` table defined in
//! `rust/migrations/20260614000008_environment.sql`. The table is the
//! per-environment "seed" used by the gRPC layer; the constants in this
//! file are the in-process fallback so unit tests and offline deploys
//! don't need a live database to resolve a rule.
//!
//! Field decisions:
//! - `EnvironmentType::Fluid(String)` uses a `String` (not a
//!   `BiocapitalFluid`) to keep this crate independent of
//!   `biocapital-core::fluids`; the gRPC layer converts tokens back
//!   into the `BiocapitalFluid` enum via the existing
//!   `fluid_from_environment_token` helper.
//! - `EnvironmentModifier` carries a primary delta plus a tag (some
//!   rules need to flip a *flag* like `NoFatalDamage` or `TriggerDefeat`
//!   without a numeric delta). The `VisualOnly` variant covers SAND's
//!   "desertification" effect that has no pleasure / hunger impact.
//! - `IntensityFormula::FixedDuration { duration_ticks }` is the per
//!   task #8 + 07 §8 公式: a fixed `base` is applied once per
//!   `duration_ticks` window. 20 ticks (1 s) is the canonical "per
//!   second" rate; 72000 ticks is 1 hour at 20 tps (07 §4.1 SAND
//!   沙漠化).

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// ── EnvironmentType enum (4 + 1 values) ─────────────────────────────────────

/// The 4 canonical de-fatalised environments tracked by
/// `doc/07-environment.md`, plus a `Fluid(String)` escape hatch for
/// the FLUID_<X> tokens routed by
/// [`biocapital_environment::fluid_from_environment_token`]
/// (task #8 fluid path).
///
/// The wire form (used in `EnvironmentEffectRequest.environment` and
/// `environment_default_rules.environment`) is the SCREAMING_SNAKE
/// string returned by [`EnvironmentType::as_str`]. The 4 canonical
/// values match the PG `environment_default_rules.environment` CHECK
/// constraint exactly; `FLUID_<X>` tokens are matched by a different
/// routing path (prefix detection in the gRPC layer).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EnvironmentType {
    /// 岩浆 (07 §2). `FluidImmersion` source.
    Lava,
    /// 沼泽泥地 (07 §3). `BlockContact` source.
    SwampMud,
    /// 沙 (07 §4). `BlockContact` source. The doc has two sand variants
    /// (every 10 s +0.5 pleasure AND every hour −0.5 hunger); the rule
    /// table carries the hunger side because that is the
    /// "desertification" effect the mod is most concerned with. The
    /// pleasure side is folded into the player-state's existing tick
    /// loop (it is the only non-environment-attached component) and
    /// does not need an `EnvironmentEffectRule` row.
    Sand,
    /// 岩浆块 (07 §5). `BlockContact` source.
    MagmaBlock,
    /// Escape hatch: an arbitrary `FLUID_<X>` token. The gRPC layer
    /// uses the prefix-detect rule from task #8 to route these calls
    /// to `apply_fluid_effect_environmental`. `EnvironmentEffectRule`
    /// rows are never seeded for `Fluid(_)` — the fluid path has its
    /// own table (`fluid_effects`).
    Fluid(String),
}

impl EnvironmentType {
    /// All 4 canonical environments in stable declaration order.
    /// Excludes `Fluid(_)` because the fluid path is open-ended.
    pub const CANONICAL: [EnvironmentType; 4] = [
        EnvironmentType::Lava,
        EnvironmentType::SwampMud,
        EnvironmentType::Sand,
        EnvironmentType::MagmaBlock,
    ];

    /// Wire name used by the PG `environment_default_rules.environment`
    /// CHECK constraint and the audit `audit_environment.environment`
    /// column. `Fluid(token)` returns the literal token (e.g.
    /// `"FLUID_HIGH_TIDE"`).
    pub fn as_str(&self) -> String {
        match self {
            EnvironmentType::Lava => "LAVA".to_string(),
            EnvironmentType::SwampMud => "SWAMP_MUD".to_string(),
            EnvironmentType::Sand => "SAND".to_string(),
            EnvironmentType::MagmaBlock => "MAGMA_BLOCK".to_string(),
            EnvironmentType::Fluid(token) => token.clone(),
        }
    }

    /// True for the 4 canonical environments. `Fluid(_)` returns
    /// false — the fluid path is dispatched separately.
    pub fn is_canonical(&self) -> bool {
        !matches!(self, EnvironmentType::Fluid(_))
    }
}

impl std::fmt::Display for EnvironmentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_str())
    }
}

impl std::str::FromStr for EnvironmentType {
    type Err = EnvironmentParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "LAVA" => Ok(EnvironmentType::Lava),
            "SWAMP_MUD" => Ok(EnvironmentType::SwampMud),
            "SAND" => Ok(EnvironmentType::Sand),
            "MAGMA_BLOCK" => Ok(EnvironmentType::MagmaBlock),
            other if other.starts_with("FLUID_") => Ok(EnvironmentType::Fluid(other.to_string())),
            other => Err(EnvironmentParseError(other.to_string())),
        }
    }
}

/// Error returned by `EnvironmentType::from_str` when the string is
/// not a canonical name and does not start with the `FLUID_` prefix.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error(
    "unknown environment: {0} (expected LAVA / SWAMP_MUD / SAND / MAGMA_BLOCK, or a FLUID_<X> token)"
)]
pub struct EnvironmentParseError(pub String);

// ── EnvironmentModifier enum (6 values) ─────────────────────────────────────

/// What an environment *does* to the player standing in / on it.
///
/// The enum is split by *what* the rule mutates. `PleasureDelta` /
/// `HungerDelta` are numeric; `MovementModifier` is a multiplicative
/// speed factor; `NoFatalDamage` / `TriggerDefeat` flip a *flag*; and
/// `VisualOnly` covers SAND's "no gameplay impact, just look" mode.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EnvironmentModifier {
    /// Add `magnitude` to `PlayerStateSnapshot.pleasure` once per
    /// `IntensityFormula.duration_ticks` window. The `pleasure_delta`
    /// is signed: positive adds, negative removes. 07 §2.2 / §3.2 /
    /// §4.1 / §5.1.
    PleasureDelta(f32),
    /// Add `magnitude` to `PlayerStateSnapshot.hunger`. Signed.
    /// 07 §3.2 / §4.1.
    HungerDelta(f32),
    /// Multiply the player's movement speed by `magnitude` (typically
    /// `0.5` for swamp mud, 07 §3.2). Applied at the Java
    /// `MobEffectInstance` layer; the gRPC layer reports it via
    /// `EnvironmentModifiers.movement_modifier` so the Java side
    /// can install the right `MOVEMENT_SLOWDOWN` effect.
    MovementModifier(f32),
    /// When this modifier is the rule's `primary_modifier`, the
    /// environment *replaces* fatal damage with the
    /// `NoFatalDamage` flag (07 §2.1 / §3.2). The Rust gRPC layer
    /// flips the `DamageResponse.killed = false` envelope and writes
    /// `audit_environment.no_fatal_damage = TRUE` so the audit trail
    /// captures the swap.
    NoFatalDamage,
    /// When this modifier fires, the gRPC layer sets
    /// `EnvironmentEffectResponse.triggered_defeat = TRUE` and the
    /// caller is expected to pivot to the defeat-state entry path
    /// (07 §6 + 06 §2.3). The pleasure / hunger mutation
    /// continues to apply as well.
    TriggerDefeat,
    /// Pure-decoration / no gameplay effect. Used by the SAND rule
    /// when the player is in the *low* pleasure / high hunger
    /// corner of the env heatmap where there is no mutation to
    /// apply.
    VisualOnly,
}

impl EnvironmentModifier {
    /// Wire name used by the PG
    /// `environment_default_rules.primary_modifier` CHECK constraint
    /// and the audit `audit_environment.notes` JSON payload.
    pub fn as_str(&self) -> &'static str {
        match self {
            EnvironmentModifier::PleasureDelta(_) => "PLEASURE_DELTA",
            EnvironmentModifier::HungerDelta(_) => "HUNGER_DELTA",
            EnvironmentModifier::MovementModifier(_) => "MOVEMENT_MODIFIER",
            EnvironmentModifier::NoFatalDamage => "NO_FATAL_DAMAGE",
            EnvironmentModifier::TriggerDefeat => "TRIGGER_DEFEAT",
            EnvironmentModifier::VisualOnly => "VISUAL_ONLY",
        }
    }

    /// Numeric magnitude, when applicable. `0.0` for the flag /
    /// no-op variants.
    pub fn magnitude(&self) -> f32 {
        match self {
            EnvironmentModifier::PleasureDelta(m) => *m,
            EnvironmentModifier::HungerDelta(m) => *m,
            EnvironmentModifier::MovementModifier(m) => *m,
            _ => 0.0,
        }
    }
}

// ── IntensityFormula enum (3 values) ────────────────────────────────────────

/// How often / for how long the rule's `primary_modifier` fires.
///
/// The formula is applied by the gRPC `EnvironmentService` on every
/// `ApplyEnvironmentEffect` call. The `duration_ticks` field is
/// 07 §8's "how long the effect lasts" knob; `0` means "instant /
/// apply once" (used by the VFX-only rules).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IntensityFormula {
    /// Apply the modifier once, `magnitude` per second,
    /// indefinitely. 07 §2.2 (Lava). `magnitude` is the per-tick
    /// delta; the gRPC layer multiplies by the proto
    /// `EnvironmentEffectRequest.intensity` (07 §8 formula
    /// `applied_magnitude = base_magnitude × intensity`).
    Fixed(f32),
    /// Linear-decay formula. `base` is the per-tick delta at the
    /// source; the effective delta falls off by
    /// `decay_per_block` per block of distance, capped at
    /// `max_blocks`. Beyond `max_blocks` the rule does not fire.
    /// 07 §2.2 (Lava, "上方 1 块") is the canonical example:
    /// the rule applies at full strength when the player is in the
    /// lava and decays to 0 one block above.
    LinearDistance {
        base: f32,
        decay_per_block: f32,
        max_blocks: i32,
    },
    /// Apply the modifier `base` once per `duration_ticks`-tick
    /// window. `20` ticks = 1 s at 20 tps; `72000` = 1 hour. This
    /// is the formula used by 07 §3.2 (SwampMud −2.0 hunger / s)
    /// and 07 §4.1 (Sand −0.5 hunger / hour).
    FixedDuration { base: f32, duration_ticks: i64 },
}

impl IntensityFormula {
    /// Wire name used by the PG
    /// `environment_default_rules.intensity_formula` CHECK
    /// constraint.
    pub fn as_str(&self) -> &'static str {
        match self {
            IntensityFormula::Fixed(_) => "FIXED",
            IntensityFormula::LinearDistance { .. } => "LINEAR_DISTANCE",
            IntensityFormula::FixedDuration { .. } => "FIXED_DURATION",
        }
    }

    /// The numeric `base` magnitude carried by the formula. For
    /// `LinearDistance` this is the source-strength; for `Fixed` /
    /// `FixedDuration` it is the per-tick / per-window delta.
    pub fn base(&self) -> f32 {
        match self {
            IntensityFormula::Fixed(m) => *m,
            IntensityFormula::LinearDistance { base, .. } => *base,
            IntensityFormula::FixedDuration { base, .. } => *base,
        }
    }

    /// The duration / window in ticks. `0` for the "always on"
    /// `Fixed` variant; the
    /// `LinearDistance` variant returns `0` because there is no
    /// tick window, just a distance cap.
    pub fn duration_ticks(&self) -> i64 {
        match self {
            IntensityFormula::Fixed(_) => 0,
            IntensityFormula::LinearDistance { .. } => 0,
            IntensityFormula::FixedDuration { duration_ticks, .. } => *duration_ticks,
        }
    }
}

// ── EnvironmentSource enum (3 values) ───────────────────────────────────────

/// Where the rule originates. The gRPC `EnvironmentService` uses this
/// to decide whether to call `ApplyEnvironmentEffect` (block contact)
/// or hand the request off to the `apply_fluid_effect_environmental`
/// path (fluid immersion / air exposure).
///
/// `AirExposure` is reserved for the SAND "desertification" effect
/// that triggers even when the player is not standing on a sand
/// block (07 §4.1 is ambiguous on the source; the task #10 spec
/// keeps the option open for future rules).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EnvironmentSource {
    /// Player is standing on / inside a vanilla `Block` (e.g. sand,
    /// magma_block, custom swamp_mud). 07 §3 / §4 / §5.
    BlockContact,
    /// Player is immersed in a fluid. The gRPC layer dispatches these
    /// to `apply_fluid_effect_environmental` (07 §8 + 05 §4 fluid
    /// path). 07 §2.
    FluidImmersion,
    /// Player is exposed to the environment without direct contact
    /// (e.g. standing in the same biome as a sandstorm). Reserved
    /// for future SAND desertification rules.
    AirExposure,
}

impl EnvironmentSource {
    /// All 3 values, in stable declaration order.
    pub const ALL: [EnvironmentSource; 3] = [
        EnvironmentSource::BlockContact,
        EnvironmentSource::FluidImmersion,
        EnvironmentSource::AirExposure,
    ];

    /// Wire name used by the PG
    /// `environment_default_rules.source` CHECK constraint.
    pub fn as_str(&self) -> &'static str {
        match self {
            EnvironmentSource::BlockContact => "BLOCK_CONTACT",
            EnvironmentSource::FluidImmersion => "FLUID_IMMERSION",
            EnvironmentSource::AirExposure => "AIR_EXPOSURE",
        }
    }
}

impl std::fmt::Display for EnvironmentSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EnvironmentSource {
    type Err = EnvironmentSourceParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BLOCK_CONTACT" => Ok(EnvironmentSource::BlockContact),
            "FLUID_IMMERSION" => Ok(EnvironmentSource::FluidImmersion),
            "AIR_EXPOSURE" => Ok(EnvironmentSource::AirExposure),
            other => Err(EnvironmentSourceParseError(other.to_string())),
        }
    }
}

/// Error returned by `EnvironmentSource::from_str`.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error(
    "unknown environment source: {0} (expected BLOCK_CONTACT / FLUID_IMMERSION / AIR_EXPOSURE)"
)]
pub struct EnvironmentSourceParseError(pub String);

// ── EnvironmentEffectRule (in-memory payload) ───────────────────────────────

/// A single environment-effect rule, as it appears in the PG
/// `environment_default_rules` table and as it flows through the
/// gRPC `EnvironmentService`.
///
/// The struct is intentionally simple: the rule carries a primary
/// modifier, an intensity formula, and a source flag. Compound rules
/// (e.g. "swamp mud applies pleasure + hunger + movement") are
/// expressed as multiple rows in `environment_default_rules` rather
/// than as a richer Rust struct — task #10's design choice (the
/// "4 environments × 1 row per env" model).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentEffectRule {
    /// Stable per-row identifier (matches
    /// `environment_default_rules.rule_id`).
    pub rule_id: Uuid,

    /// Which environment this rule governs. 4 canonical values
    /// (`Lava` / `SwampMud` / `Sand` / `MagmaBlock`) plus the
    /// `Fluid(_)` escape hatch.
    pub environment: EnvironmentType,

    /// What the rule does. Most rules carry a single
    /// `PleasureDelta` / `HungerDelta` / `MovementModifier`;
    /// `NoFatalDamage` / `TriggerDefeat` are reserved for the
    /// rule that *replaces* a vanilla damage tick (lava / magma).
    pub primary_modifier: EnvironmentModifier,

    /// How often the modifier fires. 07 §2.2 / §3.2 / §4.1 / §5.1
    /// values are baked into [`DEFAULT_ENVIRONMENT_RULES`].
    pub intensity_formula: IntensityFormula,

    /// Where the rule originates. The gRPC layer uses this to
    /// route the request.
    pub source: EnvironmentSource,

    /// Whether the rule is active. Inactive rules are
    /// skipped by the gRPC read path; the partial UNIQUE
    /// index on `environment WHERE enabled = TRUE` keeps the
    /// hot read tight.
    pub enabled: bool,

    /// Server tick at which the row was first inserted.
    /// `0` for the seeded rows.
    pub created_tick: i64,

    /// Tie-breaker when multiple enabled rows match the same
    /// environment. Higher `priority` wins. The gRPC layer
    /// sorts by `priority DESC, rule_id DESC` (deterministic
    /// secondary key) and takes the first row. The seeded
    /// rows all carry `priority = 0`; admin overrides
    /// (task #11) will use positive priorities to mask the
    /// default. Mirrors `mob_replacements.priority` (99
    /// §5.1.8) and the `audit_*` convention.
    pub priority: i32,
}

impl EnvironmentEffectRule {
    /// Construct a fresh rule with a random `rule_id`. Used by
    /// the PG repository when inserting new rows and by tests.
    pub fn new(
        environment: EnvironmentType,
        primary_modifier: EnvironmentModifier,
        intensity_formula: IntensityFormula,
        source: EnvironmentSource,
    ) -> Self {
        Self {
            rule_id: Uuid::new_v4(),
            environment,
            primary_modifier,
            intensity_formula,
            source,
            enabled: true,
            created_tick: 0,
            priority: 0,
        }
    }

    /// Construct a rule with an explicit `rule_id`. Used by
    /// the PG repository when deserialising an existing row.
    pub fn with_id(
        rule_id: Uuid,
        environment: EnvironmentType,
        primary_modifier: EnvironmentModifier,
        intensity_formula: IntensityFormula,
        source: EnvironmentSource,
        enabled: bool,
        created_tick: i64,
        priority: i32,
    ) -> Self {
        Self {
            rule_id,
            environment,
            primary_modifier,
            intensity_formula,
            source,
            enabled,
            created_tick,
            priority,
        }
    }
}

// ── DEFAULT_ENVIRONMENT_RULES (in-process seed) ─────────────────────────────

/// Mirrors the seed INSERTs in
/// `rust/migrations/20260614000008_environment.sql`.
///
/// The gRPC `EnvironmentService` consults this constant when the PG
/// `environment_default_rules` table is empty (cold start) or when
/// the rule for the requested environment is missing. In production
/// the table is always populated by the migration, so this constant
/// is the *fallback* only.
pub const DEFAULT_ENVIRONMENT_RULES: &[EnvironmentEffectRule] = &[
    // LAVA: +1.0 pleasure per second while immersed. The
    //       `NoFatalDamage` side of the rule is the *gating*
    //       behaviour (07 §2.1) — handled in the gRPC layer's
    //       damage response envelope rather than as a
    //       `primary_modifier` here.
    EnvironmentEffectRule {
        rule_id: Uuid::nil(),
        environment: EnvironmentType::Lava,
        primary_modifier: EnvironmentModifier::PleasureDelta(1.0),
        intensity_formula: IntensityFormula::FixedDuration {
            base: 1.0,
            duration_ticks: 20,
        },
        source: EnvironmentSource::FluidImmersion,
        enabled: true,
        created_tick: 0,
        priority: 0,
    },
    // SWAMP_MUD: −2.0 hunger per second while standing on the block.
    //            The 50 % movement slow is applied by the Java side
    //            via the existing `EnvironmentEffects` listener; the
    //            gRPC layer reports `movement_modifier = 0.5` in
    //            `EnvironmentModifiers` so the Java side can install
    //            the matching `MOVEMENT_SLOWDOWN` instance.
    EnvironmentEffectRule {
        rule_id: Uuid::nil(),
        environment: EnvironmentType::SwampMud,
        primary_modifier: EnvironmentModifier::HungerDelta(-2.0),
        intensity_formula: IntensityFormula::FixedDuration {
            base: -2.0,
            duration_ticks: 20,
        },
        source: EnvironmentSource::BlockContact,
        enabled: true,
        created_tick: 0,
        priority: 0,
    },
    // SAND: −0.5 hunger per hour (desertification, 07 §4.1).
    //       72000 ticks = 1 hour at 20 tps.
    EnvironmentEffectRule {
        rule_id: Uuid::nil(),
        environment: EnvironmentType::Sand,
        primary_modifier: EnvironmentModifier::HungerDelta(-0.5),
        intensity_formula: IntensityFormula::FixedDuration {
            base: -0.5,
            duration_ticks: 72_000,
        },
        source: EnvironmentSource::BlockContact,
        enabled: true,
        created_tick: 0,
        priority: 0,
    },
    // MAGMA_BLOCK: +0.5 pleasure per second while standing on the
    //              block. Lighter than LAVA per 07 §5.1.
    EnvironmentEffectRule {
        rule_id: Uuid::nil(),
        environment: EnvironmentType::MagmaBlock,
        primary_modifier: EnvironmentModifier::PleasureDelta(0.5),
        intensity_formula: IntensityFormula::FixedDuration {
            base: 0.5,
            duration_ticks: 20,
        },
        source: EnvironmentSource::BlockContact,
        enabled: true,
        created_tick: 0,
        priority: 0,
    },
];

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_count_is_four() {
        assert_eq!(EnvironmentType::CANONICAL.len(), 4);
    }

    #[test]
    fn environment_type_roundtrip_canonical() {
        for env in EnvironmentType::CANONICAL {
            let s = env.as_str();
            let back: EnvironmentType = s.parse().unwrap();
            assert_eq!(back, env);
        }
    }

    #[test]
    fn environment_type_roundtrip_fluid_escape_hatch() {
        let f = EnvironmentType::Fluid("FLUID_HIGH_TIDE".to_string());
        let s = f.as_str();
        assert_eq!(s, "FLUID_HIGH_TIDE");
        let back: EnvironmentType = s.parse().unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn environment_type_rejects_unknown() {
        assert!("MILK".parse::<EnvironmentType>().is_err());
        assert!("lava".parse::<EnvironmentType>().is_err()); // case-sensitive
        assert!("".parse::<EnvironmentType>().is_err());
    }

    #[test]
    fn canonical_is_canonical() {
        for env in EnvironmentType::CANONICAL {
            assert!(env.is_canonical());
        }
        assert!(!EnvironmentType::Fluid("FLUID_X".into()).is_canonical());
    }

    #[test]
    fn modifier_wire_names_match_99_matrix() {
        assert_eq!(EnvironmentModifier::PleasureDelta(0.0).as_str(), "PLEASURE_DELTA");
        assert_eq!(EnvironmentModifier::HungerDelta(0.0).as_str(), "HUNGER_DELTA");
        assert_eq!(EnvironmentModifier::MovementModifier(0.0).as_str(), "MOVEMENT_MODIFIER");
        assert_eq!(EnvironmentModifier::NoFatalDamage.as_str(), "NO_FATAL_DAMAGE");
        assert_eq!(EnvironmentModifier::TriggerDefeat.as_str(), "TRIGGER_DEFEAT");
        assert_eq!(EnvironmentModifier::VisualOnly.as_str(), "VISUAL_ONLY");
    }

    #[test]
    fn modifier_magnitude_zero_for_flags() {
        assert_eq!(EnvironmentModifier::NoFatalDamage.magnitude(), 0.0);
        assert_eq!(EnvironmentModifier::TriggerDefeat.magnitude(), 0.0);
        assert_eq!(EnvironmentModifier::VisualOnly.magnitude(), 0.0);
        assert_eq!(EnvironmentModifier::PleasureDelta(7.5).magnitude(), 7.5);
        assert_eq!(EnvironmentModifier::HungerDelta(-2.0).magnitude(), -2.0);
    }

    #[test]
    fn formula_wire_names_match_99_matrix() {
        assert_eq!(IntensityFormula::Fixed(0.0).as_str(), "FIXED");
        assert_eq!(
            IntensityFormula::LinearDistance {
                base: 0.0,
                decay_per_block: 0.0,
                max_blocks: 0
            }
            .as_str(),
            "LINEAR_DISTANCE"
        );
        assert_eq!(
            IntensityFormula::FixedDuration {
                base: 0.0,
                duration_ticks: 0
            }
            .as_str(),
            "FIXED_DURATION"
        );
    }

    #[test]
    fn formula_base_and_duration_match() {
        let f = IntensityFormula::FixedDuration {
            base: 1.0,
            duration_ticks: 20,
        };
        assert_eq!(f.base(), 1.0);
        assert_eq!(f.duration_ticks(), 20);
        let g = IntensityFormula::LinearDistance {
            base: 10.0,
            decay_per_block: 0.5,
            max_blocks: 4,
        };
        assert_eq!(g.base(), 10.0);
        assert_eq!(g.duration_ticks(), 0); // no tick window
    }

    #[test]
    fn source_roundtrip() {
        for s in EnvironmentSource::ALL {
            let back: EnvironmentSource = s.as_str().parse().unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn source_rejects_unknown() {
        assert!("MAGIC".parse::<EnvironmentSource>().is_err());
        assert!("".parse::<EnvironmentSource>().is_err());
    }

    #[test]
    fn default_rules_have_one_row_per_canonical_env() {
        assert_eq!(DEFAULT_ENVIRONMENT_RULES.len(), 4);
        let envs: Vec<&EnvironmentType> = DEFAULT_ENVIRONMENT_RULES
            .iter()
            .map(|r| &r.environment)
            .collect();
        assert!(envs.contains(&&EnvironmentType::Lava));
        assert!(envs.contains(&&EnvironmentType::SwampMud));
        assert!(envs.contains(&&EnvironmentType::Sand));
        assert!(envs.contains(&&EnvironmentType::MagmaBlock));
    }

    #[test]
    fn default_lava_rule_pleasure_one_per_second() {
        let lava = DEFAULT_ENVIRONMENT_RULES
            .iter()
            .find(|r| r.environment == EnvironmentType::Lava)
            .expect("lava rule");
        assert_eq!(
            lava.primary_modifier,
            EnvironmentModifier::PleasureDelta(1.0)
        );
        assert_eq!(
            lava.intensity_formula,
            IntensityFormula::FixedDuration {
                base: 1.0,
                duration_ticks: 20
            }
        );
        assert_eq!(lava.source, EnvironmentSource::FluidImmersion);
    }

    #[test]
    fn default_swamp_mud_rule_hunger_minus_two_per_second() {
        let s = DEFAULT_ENVIRONMENT_RULES
            .iter()
            .find(|r| r.environment == EnvironmentType::SwampMud)
            .expect("swamp rule");
        assert_eq!(s.primary_modifier, EnvironmentModifier::HungerDelta(-2.0));
        assert_eq!(
            s.intensity_formula,
            IntensityFormula::FixedDuration {
                base: -2.0,
                duration_ticks: 20
            }
        );
        assert_eq!(s.source, EnvironmentSource::BlockContact);
    }

    #[test]
    fn default_sand_rule_hunger_minus_half_per_hour() {
        let s = DEFAULT_ENVIRONMENT_RULES
            .iter()
            .find(|r| r.environment == EnvironmentType::Sand)
            .expect("sand rule");
        assert_eq!(s.primary_modifier, EnvironmentModifier::HungerDelta(-0.5));
        // 72000 ticks = 1 hour at 20 tps
        assert_eq!(s.intensity_formula.duration_ticks(), 72_000);
        assert_eq!(s.source, EnvironmentSource::BlockContact);
    }

    #[test]
    fn default_magma_block_rule_pleasure_half_per_second() {
        let m = DEFAULT_ENVIRONMENT_RULES
            .iter()
            .find(|r| r.environment == EnvironmentType::MagmaBlock)
            .expect("magma rule");
        assert_eq!(
            m.primary_modifier,
            EnvironmentModifier::PleasureDelta(0.5)
        );
        assert_eq!(
            m.intensity_formula,
            IntensityFormula::FixedDuration {
                base: 0.5,
                duration_ticks: 20
            }
        );
        assert_eq!(m.source, EnvironmentSource::BlockContact);
    }
}
