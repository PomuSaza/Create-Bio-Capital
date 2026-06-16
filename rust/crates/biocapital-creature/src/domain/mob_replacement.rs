//! MobReplacement domain type — the row that ties a vanilla
//! `EntityType` ID to a `CreatureConfig.id` for the
//! hostile-mob replacement pipeline
//! (`doc/06-hostile-mobs.md` §3.3 + §5.2 + §8).
//!
//! One vanilla id can map to multiple `CreatureConfig`s (e.g.
//! during A/B testing, or "zombie" mapping to a Halloween
//! variant in October). The `priority` field lets the gRPC
//! layer break ties deterministically: the **highest-priority
//! enabled** row wins when several match a given vanilla id.
//!
//! Persistence: see `rust/migrations/20260614000007_hostile_mobs.sql`.
//! The PG row carries a UUID PK (`mob_replacement_id`) for
//! stable referencing by `audit_*` rows; the wire-facing
//! `(vanilla_id, creature_id)` pair is unique-indexed.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use super::creature::DEFAULT_DESIRE_FRAGMENT_CHANCE;

// ── MobReplacement ──────────────────────────────────────────────────────────

/// One row in the `mob_replacements` table. The combination of
/// `(vanilla_id, creature_id)` is the natural key; the table
/// also carries a synthetic `mob_replacement_id` UUID PK so
/// `audit_*` tables can FK-reference a single column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MobReplacement {
    /// Synthetic PK (UUIDv4 minted at insert time).
    pub mob_replacement_id: Uuid,

    /// Vanilla entity type to be replaced, e.g.
    /// `"minecraft:zombie"`. Mirrors proto wire form and the
    /// `EntityType.getKey(...).toString()` value on the Java
    /// side.
    pub vanilla_id: String,

    /// `CreatureConfig.id` to spawn in place of the vanilla mob.
    /// Task #9 does not FK-enforce this at the DB level (the
    /// `creature_configs` table is JSONB-backed and we don't
    /// want a JSON-schema check inside the migration); the
    /// gRPC service validates it before returning
    /// `GetDropChance`.
    pub creature_id: String,

    /// Probability of dropping a single `desire_fragment` on
    /// defeat, in `[0.0, 1.0]`. Default
    /// [`DEFAULT_DESIRE_FRAGMENT_CHANCE`] (3 %).
    pub drop_chance_desire_fragment: f32,

    /// When `false`, the row is hidden from `GetDropChance` and
    /// the Java-side `EntityJoinLevelEvent` skip-list. Default
    /// `true`.
    pub enabled: bool,

    /// Tie-breaker when multiple rows match the same
    /// `vanilla_id`. Higher wins. Defaults to `0`.
    pub priority: i32,

    /// Free-form tags (e.g. `"halloween"`, `"pvp_world"`). The
    /// gRPC layer does not currently filter on tags; this is
    /// left in place for the world-specific toggle that
    /// task #11 will add.
    pub tags: Vec<String>,

    /// Server tick at which the row was created.
    pub created_tick: i64,

    /// Server tick at which the row was last updated.
    pub updated_tick: i64,
}

impl MobReplacement {
    /// Build a new row with sensible defaults: 3 % drop chance,
    /// `enabled = true`, `priority = 0`, empty tags, equal
    /// created/updated tick.
    pub fn new(
        mob_replacement_id: Uuid,
        vanilla_id: impl Into<String>,
        creature_id: impl Into<String>,
        tick: i64,
    ) -> Self {
        Self {
            mob_replacement_id,
            vanilla_id: vanilla_id.into(),
            creature_id: creature_id.into(),
            drop_chance_desire_fragment: DEFAULT_DESIRE_FRAGMENT_CHANCE,
            enabled: true,
            priority: 0,
            tags: Vec::new(),
            created_tick: tick,
            updated_tick: tick,
        }
    }

    /// Validate the row. Called by the gRPC `Upsert` path and
    /// the test suite.
    pub fn validate(&self) -> Result<(), MobReplacementError> {
        if self.vanilla_id.is_empty() {
            return Err(MobReplacementError::EmptyVanillaId);
        }
        if self.vanilla_id.len() > 64 {
            return Err(MobReplacementError::VanillaIdTooLong {
                id: self.vanilla_id.clone(),
                max: 64,
            });
        }
        if self.creature_id.is_empty() {
            return Err(MobReplacementError::EmptyCreatureId);
        }
        if self.creature_id.len() > 64 {
            return Err(MobReplacementError::CreatureIdTooLong {
                id: self.creature_id.clone(),
                max: 64,
            });
        }
        if !self.drop_chance_desire_fragment.is_finite()
            || !(0.0..=1.0).contains(&self.drop_chance_desire_fragment)
        {
            return Err(MobReplacementError::DropChanceOutOfRange {
                chance: self.drop_chance_desire_fragment,
            });
        }
        Ok(())
    }

    /// Tie-break helper: when two `MobReplacement`s match the
    /// same `vanilla_id`, the gRPC layer calls this to pick
    /// the winner. Higher `priority` wins; ties resolve to
    /// the row with the **larger** `mob_replacement_id` UUID
    /// (lex order on bytes — deterministic, free).
    pub fn is_higher_priority_than(&self, other: &Self) -> bool {
        if self.priority != other.priority {
            return self.priority > other.priority;
        }
        self.mob_replacement_id > other.mob_replacement_id
    }
}

// ── Error ───────────────────────────────────────────────────────────────────

/// Error returned by [`MobReplacement::validate`].
#[derive(Debug, Error, Clone, PartialEq)]
pub enum MobReplacementError {
    #[error("vanilla_id is empty")]
    EmptyVanillaId,

    #[error("vanilla_id too long: {id} (max {max})")]
    VanillaIdTooLong { id: String, max: usize },

    #[error("creature_id is empty")]
    EmptyCreatureId,

    #[error("creature_id too long: {id} (max {max})")]
    CreatureIdTooLong { id: String, max: usize },

    #[error("drop_chance_desire_fragment out of range: {chance} (expected 0..=1)")]
    DropChanceOutOfRange { chance: f32 },
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn sample() -> MobReplacement {
        MobReplacement::new(
            Uuid::new_v4(),
            "minecraft:zombie",
            "variant_zombie",
            1000,
        )
    }

    #[test]
    fn new_row_has_sensible_defaults() {
        let r = sample();
        assert_eq!(r.vanilla_id, "minecraft:zombie");
        assert_eq!(r.creature_id, "variant_zombie");
        assert!((r.drop_chance_desire_fragment - 0.03).abs() < 1e-6);
        assert!(r.enabled);
        assert_eq!(r.priority, 0);
        assert!(r.tags.is_empty());
        assert_eq!(r.created_tick, 1000);
        assert_eq!(r.updated_tick, 1000);
    }

    #[test]
    fn validate_accepts_default_row() {
        sample().validate().expect("default row must validate");
    }

    #[test]
    fn validate_rejects_empty_vanilla_id() {
        let mut r = sample();
        r.vanilla_id = String::new();
        assert!(matches!(
            r.validate(),
            Err(MobReplacementError::EmptyVanillaId)
        ));
    }

    #[test]
    fn validate_rejects_empty_creature_id() {
        let mut r = sample();
        r.creature_id = String::new();
        assert!(matches!(
            r.validate(),
            Err(MobReplacementError::EmptyCreatureId)
        ));
    }

    #[test]
    fn validate_rejects_drop_chance_out_of_range() {
        let mut r = sample();
        r.drop_chance_desire_fragment = 1.5;
        assert!(matches!(
            r.validate(),
            Err(MobReplacementError::DropChanceOutOfRange { .. })
        ));
    }

    #[test]
    fn validate_rejects_nan_drop_chance() {
        let mut r = sample();
        r.drop_chance_desire_fragment = f32::NAN;
        assert!(matches!(
            r.validate(),
            Err(MobReplacementError::DropChanceOutOfRange { .. })
        ));
    }

    #[test]
    fn validate_rejects_negative_drop_chance() {
        let mut r = sample();
        r.drop_chance_desire_fragment = -0.01;
        assert!(matches!(
            r.validate(),
            Err(MobReplacementError::DropChanceOutOfRange { .. })
        ));
    }

    #[test]
    fn is_higher_priority_than_prefers_higher_priority() {
        let mut a = sample();
        a.priority = 5;
        let mut b = sample();
        b.priority = 3;
        assert!(a.is_higher_priority_than(&b));
        assert!(!b.is_higher_priority_than(&a));
    }

    #[test]
    fn is_higher_priority_breaks_ties_on_uuid_lex() {
        let lower = Uuid::nil();
        // any non-nil UUID is greater than nil
        let higher = Uuid::from_bytes([0xFF; 16]);
        let mut a = sample();
        a.mob_replacement_id = higher;
        let mut b = sample();
        b.mob_replacement_id = lower;
        a.priority = 0;
        b.priority = 0;
        assert!(a.is_higher_priority_than(&b));
        assert!(!b.is_higher_priority_than(&a));
    }
}
