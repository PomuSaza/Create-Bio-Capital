//! PlayerState core domain types — `doc/02-player-state.md` §3 + `doc/03-body-development.md` §1.1.
//!
//! This module is the **authoritative** definition of player state shapes for the
//! Rust side. The Java `PlayerStateAttachment` (see
//! `src/main/java/mo/dystopia/biocapital/state/PlayerStateAttachment.java`) is now
//! treated as a **cache mirror** only; PostgreSQL is the durable source of truth.
//!
//! Design decisions referenced:
//! - 12-value `BodyPart` enum (2026-06-14 user decision; supersedes 6-value legacy
//!   enum recorded in `BodyPart.java`).
//! - `hidden_hp` floor: 1.0 — damage that would push below the floor is clamped
//!   and `low_hp_hits` is incremented (02 §3.3).
//! - `pleasure` / `hunger` are clamped to `[0.0, 100.0]`. `hunger` may also be
//!   driven by `max_hunger` (03 §2 — belly development raises the cap).
//! - `defeat_count` is the persistent, monotonic counter of defeat-state entries
//!   (02 §3.4).
//! - `active_contracts` is reserved for task #9 (09 §2.1); the field is stored
//!   here but the wiring lands later.
//! - `low_hp_hits` is monotonic across deaths (02 §1.2).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// ── Constants ───────────────────────────────────────────────────────────────

/// Default maximum for pleasure / hunger display (02 §1.1).
pub const DEFAULT_STAT_MAX: f32 = 100.0;

/// Default hidden HP pool size (02 §1.1).
pub const DEFAULT_HIDDEN_HP: f32 = 20.0;

/// Default starting pleasure (02 §1.1, mirrors Java `SPAWN_PLEASURE`).
pub const SPAWN_PLEASURE: f32 = 0.0;

/// Default starting hunger (02 §1.1, mirrors Java `SPAWN_HUNGER`).
///
/// **Note**: the Java side uses `20.0f` for `SPAWN_HUNGER` and `20.0f` for
/// `DEFAULT_HIDDEN_HP`. The user's spec for this task, however, asks for
/// `hunger = 50.0` as the Rust-side default. We follow the user spec for the
/// Rust defaults; the Java side keeps its 20.0f until a coordinated migration
/// flips the value. See CHANGELOG "[02-player-state] - 2026-06-14".
pub const SPAWN_HUNGER_RUST: f32 = 50.0;

/// Default starting hidden HP.
pub const SPAWN_HIDDEN_HP: f32 = DEFAULT_HIDDEN_HP;

/// Default maximum hunger cap before belly development raises it.
pub const DEFAULT_MAX_HUNGER: i32 = 100;

/// Minimum stat value (pleasure / hunger); never negative (02 §1.1).
pub const STAT_MIN: f32 = 0.0;

/// Floor for `hidden_hp`; damage below this is clamped (02 §3.3).
pub const HIDDEN_HP_FLOOR: f32 = 1.0;

// ── BodyPart enum (12 values) ────────────────────────────────────────────────

/// Discrete body regions tracked by the body-development system.
///
/// **Authoritative list** per `doc/03-body-development.md` §1.1 (2026-06-14 user
/// decision). The 6-value legacy enum (`FOOT / CHEST / ARM / MOUTH / ABDOMEN /
/// GENITALS`) recorded in `BodyPart.java` is **deprecated** and **must not** be
/// used by new code.
///
/// Persistence shape:
/// - proto `PlayerState.parts` map key uses the SCREAMING_SNAKE string from
///   `BodyPart::as_str` (matches `biocapital.proto` comment).
/// - PG `body_part_development.part_name` has a CHECK constraint enforcing the
///   same 12 string set (see `rust/migrations/20260614000001_player_state.sql`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BodyPart {
    Head,
    Neck,
    Chest,
    Belly,
    Genital,
    Butt,
    Back,
    LeftArm,
    RightArm,
    LeftLeg,
    RightLeg,
    Feet,
}

impl BodyPart {
    /// All 12 values, in stable declaration order. Useful for
    /// initialising maps and for exhaustive table checks.
    pub const ALL: [BodyPart; 12] = [
        BodyPart::Head,
        BodyPart::Neck,
        BodyPart::Chest,
        BodyPart::Belly,
        BodyPart::Genital,
        BodyPart::Butt,
        BodyPart::Back,
        BodyPart::LeftArm,
        BodyPart::RightArm,
        BodyPart::LeftLeg,
        BodyPart::RightLeg,
        BodyPart::Feet,
    ];

    /// Wire name for proto map keys and PG `part_name` CHECK constraint.
    pub fn as_str(&self) -> &'static str {
        match self {
            BodyPart::Head => "HEAD",
            BodyPart::Neck => "NECK",
            BodyPart::Chest => "CHEST",
            BodyPart::Belly => "BELLY",
            BodyPart::Genital => "GENITAL",
            BodyPart::Butt => "BUTT",
            BodyPart::Back => "BACK",
            BodyPart::LeftArm => "LEFT_ARM",
            BodyPart::RightArm => "RIGHT_ARM",
            BodyPart::LeftLeg => "LEFT_LEG",
            BodyPart::RightLeg => "RIGHT_LEG",
            BodyPart::Feet => "FEET",
        }
    }
}

impl fmt::Display for BodyPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BodyPart {
    type Err = BodyPartParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "HEAD" => Ok(BodyPart::Head),
            "NECK" => Ok(BodyPart::Neck),
            "CHEST" => Ok(BodyPart::Chest),
            "BELLY" => Ok(BodyPart::Belly),
            "GENITAL" => Ok(BodyPart::Genital),
            "BUTT" => Ok(BodyPart::Butt),
            "BACK" => Ok(BodyPart::Back),
            "LEFT_ARM" => Ok(BodyPart::LeftArm),
            "RIGHT_ARM" => Ok(BodyPart::RightArm),
            "LEFT_LEG" => Ok(BodyPart::LeftLeg),
            "RIGHT_LEG" => Ok(BodyPart::RightLeg),
            "FEET" => Ok(BodyPart::Feet),
            other => Err(BodyPartParseError(other.to_string())),
        }
    }
}

/// Error returned by `BodyPart::from_str` when the string does not match any
/// of the 12 canonical values.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("unknown body part name: {0} (expected one of HEAD/NECK/CHEST/BELLY/GENITAL/BUTT/BACK/LEFT_ARM/RIGHT_ARM/LEFT_LEG/RIGHT_LEG/FEET)")]
pub struct BodyPartParseError(pub String);

// ── Result / outcome types ──────────────────────────────────────────────────

/// Outcome of a clamp-to-range operation. `clamped` is true when the result
/// differs from the raw value (`old + delta`) because one of the bounds was
/// reached. `before` / `after` are pre- / post-clamp values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClampResult {
    pub before: f32,
    pub after: f32,
    pub delta: f32,
    pub clamped: bool,
}

impl ClampResult {
    pub fn new(before: f32, after: f32, delta: f32, clamped: bool) -> Self {
        Self {
            before,
            after,
            delta,
            clamped,
        }
    }
}

/// Outcome of a single body-part delta. Includes the per-part clamp info and
/// the new absolute value of the part.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartChange {
    pub before: f32,
    pub after: f32,
    pub delta: f32,
    pub clamped: bool,
}

impl PartChange {
    pub fn new(before: f32, after: f32, delta: f32, clamped: bool) -> Self {
        Self {
            before,
            after,
            delta,
            clamped,
        }
    }
}

/// Outcome of a hidden-damage application. Mirrors the Java
/// `PlayerStateAttachment.applyHiddenDamage` semantics:
///
/// - `absorbed` is the amount of damage actually subtracted from the pool
///   (may be less than the input if the floor was hit).
/// - `floor_hit` is true when the damage pushed the pool below `HIDDEN_HP_FLOOR`
///   and the pool was clamped to the floor (and `low_hp_hits` was incremented).
/// - `defeat_triggered` is true when the floor was hit. Per 02 §3.3, hitting
///   the floor also applies a punitive debuff; the application of that debuff
///   is **not** represented here (it lives in the environment / hostile layer
///   that initiated the damage call).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    pub before: f32,
    pub after: f32,
    pub absorbed: f32,
    pub floor_hit: bool,
    pub defeat_triggered: bool,
}

impl Outcome {
    pub fn noop(before: f32) -> Self {
        Self {
            before,
            after: before,
            absorbed: 0.0,
            floor_hit: false,
            defeat_triggered: false,
        }
    }
}

// ── PlayerStateSnapshot ──────────────────────────────────────────────────────

/// In-memory view of a single player's full state, mirroring the `player_state`
/// row plus its 12 `body_part_development` rows.
///
/// This is the type that flows between the repository, the gRPC layer, and the
/// Sable JNI bridge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerStateSnapshot {
    pub uuid: Uuid,

    /// Pleasure bar, clamped to `[STAT_MIN, DEFAULT_STAT_MAX]`.
    pub pleasure: f32,

    /// Hunger bar, clamped to `[STAT_MIN, max_hunger]`.
    pub hunger: f32,

    /// Hidden HP. Floor `HIDDEN_HP_FLOOR`; damage clamps here, increments
    /// `low_hp_hits` and triggers defeat on overflow.
    pub hidden_hp: f32,

    /// Per-body-part development. Stored as a `BTreeMap` for deterministic
    /// iteration order (handy for tests, debug dumps, and JSON snapshots).
    pub parts: BTreeMap<BodyPart, f32>,

    /// Cumulative number of times `hidden_hp` has been clamped to the floor
    /// (i.e. defeat state entries). Monotonic, never resets (02 §1.2).
    pub defeat_count: i32,

    /// Current hunger cap. Starts at `DEFAULT_MAX_HUNGER`; belly development
    /// can raise it (03 §2). The Java side does not currently model this —
    /// it's a Rust-side concept for now (see CHANGELOG notes).
    pub max_hunger: i32,

    /// Alias of `defeat_count` for clients that prefer the older name. Mirrors
    /// the Java `lowHpHits` field semantics (02 §1.1) — both names refer to
    /// the same persistent counter. When the two diverge we should split them;
    /// for now we keep them in lock-step.
    pub low_hp_hits: i32,

    /// Last write time, used for optimistic concurrency at the repository
    /// layer. Stored as `chrono::DateTime<Utc>` to round-trip cleanly with
    /// proto `google.protobuf.Timestamp`.
    pub updated_at: DateTime<Utc>,
}

impl PlayerStateSnapshot {
    /// Construct a fresh snapshot with the canonical spawn defaults.
    ///
    /// Defaults (this matches the user-supplied spec; see `SPAWN_HUNGER_RUST`
    /// doc comment for the deviation from the Java default):
    /// - `pleasure = 0.0`
    /// - `hunger = 50.0` *(Rust default; Java side currently 20.0)*
    /// - `hidden_hp = 20.0`
    /// - all 12 `parts` = `0.0`
    /// - `defeat_count = 0`
    /// - `max_hunger = 100`
    /// - `low_hp_hits = 0` (kept in lock-step with `defeat_count`)
    /// - `updated_at = now()`
    pub fn new(uuid: Uuid) -> Self {
        let now = Utc::now();
        let mut parts = BTreeMap::new();
        for part in BodyPart::ALL {
            parts.insert(part, 0.0);
        }
        Self {
            uuid,
            pleasure: SPAWN_PLEASURE,
            hunger: SPAWN_HUNGER_RUST,
            hidden_hp: SPAWN_HIDDEN_HP,
            parts,
            defeat_count: 0,
            max_hunger: DEFAULT_MAX_HUNGER,
            low_hp_hits: 0,
            updated_at: now,
        }
    }

    /// Construct a snapshot at a specific timestamp. Useful for tests and
    /// for replaying audit log entries.
    pub fn new_at(uuid: Uuid, updated_at: DateTime<Utc>) -> Self {
        let mut s = Self::new(uuid);
        s.updated_at = updated_at;
        s
    }

    // ── Pleasure / hunger ───────────────────────────────────────────

    /// Add `delta` to pleasure, clamping to `[STAT_MIN, DEFAULT_STAT_MAX]`.
    pub fn add_pleasure(&mut self, delta: f32) -> ClampResult {
        let before = self.pleasure;
        let raw = before + delta;
        let lo = STAT_MIN;
        let hi = DEFAULT_STAT_MAX;
        let after = raw.clamp(lo, hi);
        let clamped = after != raw;
        self.pleasure = after;
        self.touch();
        ClampResult::new(before, after, delta, clamped)
    }

    /// Add `delta` to hunger, clamping to `[STAT_MIN, max_hunger]`.
    pub fn add_hunger(&mut self, delta: f32) -> ClampResult {
        let before = self.hunger;
        let raw = before + delta;
        let lo = STAT_MIN;
        let hi = self.max_hunger as f32;
        let after = raw.clamp(lo, hi);
        let clamped = after != raw;
        self.hunger = after;
        self.touch();
        ClampResult::new(before, after, delta, clamped)
    }

    // ── Hidden HP / defeat ──────────────────────────────────────────

    /// Apply damage to the hidden HP pool.
    ///
    /// - Negative or zero `dmg` is a no-op (returns `Outcome::noop`).
    /// - If `hidden_hp - dmg < HIDDEN_HP_FLOOR`, the pool is clamped to
    ///   `HIDDEN_HP_FLOOR`, `absorbed` is the actual subtraction, and
    ///   `low_hp_hits` + `defeat_count` are both incremented
    ///   (they remain in lock-step — see struct docs).
    /// - Otherwise the full `dmg` is absorbed.
    pub fn add_hidden_damage(&mut self, dmg: f32) -> Outcome {
        if dmg <= 0.0 {
            return Outcome::noop(self.hidden_hp);
        }
        let before = self.hidden_hp;
        let after_raw = before - dmg;
        if after_raw < HIDDEN_HP_FLOOR {
            // Floor hit. The absorbed amount is the distance from `before`
            // down to the floor; anything beyond the floor is "defeated" and
            // the punitive debuff / event chain is fired by the caller.
            let absorbed = (before - HIDDEN_HP_FLOOR).max(0.0);
            self.hidden_hp = HIDDEN_HP_FLOOR;
            self.low_hp_hits = self.low_hp_hits.saturating_add(1);
            self.defeat_count = self.defeat_count.saturating_add(1);
            self.touch();
            Outcome {
                before,
                after: HIDDEN_HP_FLOOR,
                absorbed,
                floor_hit: true,
                defeat_triggered: true,
            }
        } else {
            self.hidden_hp = after_raw;
            self.touch();
            Outcome {
                before,
                after: after_raw,
                absorbed: dmg,
                floor_hit: false,
                defeat_triggered: false,
            }
        }
    }

    /// Heal the hidden HP pool, clamping to `DEFAULT_HIDDEN_HP`.
    /// Returns the actual amount healed.
    pub fn heal_hidden(&mut self, amount: f32) -> f32 {
        if amount <= 0.0 {
            return 0.0;
        }
        let before = self.hidden_hp;
        let after = (before + amount).min(DEFAULT_HIDDEN_HP);
        let healed = (after - before).max(0.0);
        if healed > 0.0 {
            self.hidden_hp = after;
            self.touch();
        }
        healed
    }

    // ── Body parts ──────────────────────────────────────────────────

    /// Look up the current development value of a part, defaulting to 0.0
    /// if the map is missing the entry (which should not happen for snapshots
    /// built via `new`, but is safe for snapshots deserialised from older
    /// payloads).
    pub fn get_part(&self, part: BodyPart) -> f32 {
        self.parts.get(&part).copied().unwrap_or(0.0)
    }

    /// Add `delta` to a body part's development, clamping to `[0.0, 100.0]`.
    ///
    /// Per 03 §2: values above 100 are treated as overflow but still apply.
    /// We follow the 02 §3 / Java `PlayerStateAttachment.addPart` precedent
    /// of clamping at 100 on the **storage** side; the *display* side (UI,
    /// `/biocapital stats`) is responsible for showing `>100%` (03 §5).
    /// If overflow tolerance is later relaxed, the clamp target moves to
    /// `f32::INFINITY` and the Java side follows.
    pub fn add_part_dev(&mut self, part: BodyPart, delta: f32) -> Result<PartChange, PartDevError> {
        if !delta.is_finite() {
            return Err(PartDevError::NonFiniteDelta(delta));
        }
        let before = self.get_part(part);
        let raw = before + delta;
        let after = raw.clamp(0.0, DEFAULT_STAT_MAX);
        let clamped = after != raw;
        self.parts.insert(part, after);
        self.touch();
        Ok(PartChange::new(before, after, delta, clamped))
    }

    /// Replace a body part's value outright. Clamped to `[0.0, 100.0]`.
    pub fn set_part(&mut self, part: BodyPart, value: f32) -> Result<PartChange, PartDevError> {
        if !value.is_finite() {
            return Err(PartDevError::NonFiniteDelta(value));
        }
        let before = self.get_part(part);
        let after = value.clamp(0.0, DEFAULT_STAT_MAX);
        let clamped = after != value;
        self.parts.insert(part, after);
        self.touch();
        // Set semantics: the "delta" reported is the actual change.
        Ok(PartChange::new(before, after, after - before, clamped))
    }

    // ── Internal ────────────────────────────────────────────────────

    /// Bump the `updated_at` timestamp to `now()`. Call this on every
    /// mutation that should be reflected in the audit log / push to clients.
    fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

impl Default for PlayerStateSnapshot {
    fn default() -> Self {
        // We deliberately use a nil UUID for `default()`; production code
        // should always go through `PlayerStateSnapshot::new(uuid)`. This impl
        // exists so the type can be used in container types like
        // `HashMap<_, PlayerStateSnapshot>` for tests.
        Self::new(Uuid::nil())
    }
}

/// Errors specific to body-part development operations.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum PartDevError {
    #[error("non-finite delta: {0}")]
    NonFiniteDelta(f32),
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> PlayerStateSnapshot {
        PlayerStateSnapshot::new(Uuid::new_v4())
    }

    #[test]
    fn new_defaults() {
        let s = snap();
        assert_eq!(s.pleasure, 0.0);
        assert_eq!(s.hunger, 50.0);
        assert_eq!(s.hidden_hp, 20.0);
        assert_eq!(s.defeat_count, 0);
        assert_eq!(s.low_hp_hits, 0);
        assert_eq!(s.max_hunger, 100);
        assert_eq!(s.parts.len(), 12);
        for v in s.parts.values() {
            assert_eq!(*v, 0.0);
        }
    }

    #[test]
    fn pleasure_clamps() {
        let mut s = snap();
        let r = s.add_pleasure(30.0);
        assert_eq!(s.pleasure, 30.0);
        assert!(!r.clamped);
        let r = s.add_pleasure(1000.0);
        assert_eq!(s.pleasure, 100.0);
        assert!(r.clamped);
        let r = s.add_pleasure(-1000.0);
        assert_eq!(s.pleasure, 0.0);
        assert!(r.clamped);
    }

    #[test]
    fn hunger_clamps_against_max_hunger() {
        let mut s = snap();
        s.max_hunger = 150;
        let r = s.add_hunger(120.0);
        assert_eq!(s.hunger, 150.0);
        assert!(r.clamped);
    }

    #[test]
    fn hidden_damage_floor_increments_counters() {
        let mut s = snap();
        let o = s.add_hidden_damage(5.0);
        assert_eq!(s.hidden_hp, 15.0);
        assert!(!o.floor_hit);
        assert!(!o.defeat_triggered);
        assert_eq!(s.low_hp_hits, 0);
        assert_eq!(s.defeat_count, 0);

        let o = s.add_hidden_damage(100.0);
        assert_eq!(s.hidden_hp, HIDDEN_HP_FLOOR);
        assert!(o.floor_hit);
        assert!(o.defeat_triggered);
        assert_eq!(s.low_hp_hits, 1);
        assert_eq!(s.defeat_count, 1);
    }

    #[test]
    fn negative_damage_is_noop() {
        let mut s = snap();
        let o = s.add_hidden_damage(-5.0);
        assert_eq!(s.hidden_hp, 20.0);
        assert_eq!(o.absorbed, 0.0);
        assert!(!o.floor_hit);
    }

    #[test]
    fn part_dev_clamps() {
        let mut s = snap();
        let r = s.add_part_dev(BodyPart::Genital, 60.0).unwrap();
        assert_eq!(s.get_part(BodyPart::Genital), 60.0);
        assert!(!r.clamped);
        let r = s.add_part_dev(BodyPart::Genital, 1000.0).unwrap();
        assert_eq!(s.get_part(BodyPart::Genital), 100.0);
        assert!(r.clamped);
    }

    #[test]
    fn part_dev_rejects_nan() {
        let mut s = snap();
        assert!(s.add_part_dev(BodyPart::Head, f32::NAN).is_err());
    }

    #[test]
    fn body_part_roundtrip() {
        for p in BodyPart::ALL {
            let s = p.to_string();
            let back: BodyPart = s.parse().unwrap();
            assert_eq!(p, back);
        }
    }

    #[test]
    fn body_part_rejects_unknown() {
        assert!(BodyPart::from_str("FOOT").is_err()); // legacy 6-value name
        assert!(BodyPart::from_str("feet").is_err()); // case-sensitive
        assert!(BodyPart::from_str("").is_err());
    }

    #[test]
    fn body_part_serialises_to_screaming_snake() {
        let s = serde_json::to_string(&BodyPart::LeftArm).unwrap();
        assert_eq!(s, "\"LEFT_ARM\"");
    }
}
