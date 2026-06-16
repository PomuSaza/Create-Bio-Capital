//! CreatureConfig + supporting sub-structures
//! (`doc/13-bio-customization.md` §2 + `doc/06-hostile-mobs.md` §6).
//!
//! This module defines the **authoritative** server-side shape of a
//! creature config (the per-variant metadata that drives both the
//! Java-side Geckolib rendering and the Rust-side damage / drop
//! routing). The on-disk JSON file `creatures.json` (13 §1.2 +
//! §2) and the PostgreSQL `creature_configs` table (13 §6.3) are
//! projections of this struct; the JSON keys map 1:1 to the fields
//! below.
//!
//! Task #9 scope: this file lands the **server-side domain types**
//! and their validation helpers. The full hot-reload of
//! `creatures.json` (file watcher + `CreatureService.ReloadCreatures`)
//! is task #11. The present task only consumes `CreatureConfig` to
//! resolve a `creature_id` → drop-chance / stat mapping at hostile-
//! mob replacement time (see `biocapital-grpc::hostile_mob_service`).

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use biocapital_core::player_state::BodyPart;

// ── Constants ───────────────────────────────────────────────────────────────

/// Default drop chance of a single Desire Fragment per variant
/// defeated (06 §5.2). 3 % matches the legacy Java `MobDropsHandler`
/// constant; remains the default for the Rust rewrite so existing
/// data carries over.
pub const DEFAULT_DESIRE_FRAGMENT_CHANCE: f32 = 0.03;

/// Cap on `min_count` / `max_count` of a single `DropEntry`. Pure
/// defensive bound — no real creature drops 1M of one item.
pub const MAX_DROP_STACK: i32 = 64 * 64;

// ── ModelSource ─────────────────────────────────────────────────────────────

/// Where the variant entity's visual model comes from. The
/// "vanilla" branch means "inherit the original vanilla mob's
/// model" (06 §1.3 / 13 §2 — same AI / HP / attack). The
/// "custom" branch means "load a GeckoLib / BBModel file from the
/// `assets/create_biocapital/geo/<creature_id>.geo.json` path".
///
/// **Persistence:** the table holds an enum-as-string; the wire
/// form is one of `"VANILLA"` / `"CUSTOM_GEO"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ModelSource {
    /// Inherit the original vanilla mob's rendering. `entity_type`
    /// (`minecraft:zombie` etc.) carries the identity.
    Vanilla,
    /// Custom GeckoLib / BBModel model loaded from
    /// `assets/create_biocapital/geo/<path>.geo.json`.
    CustomGeo { path: String },
}

impl ModelSource {
    /// Wire form used in the JSON / proto. `path` is **omitted**
    /// from the wire when the variant is `Vanilla`; callers must
    /// default to the empty string in that case.
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelSource::Vanilla => "VANILLA",
            ModelSource::CustomGeo { .. } => "CUSTOM_GEO",
        }
    }

    /// The GeckoLib geo path, or `None` for vanilla-inheriting
    /// variants. Callers should NOT use this as a primary key —
    /// `creature_id` is the unique identity.
    pub fn geo_path(&self) -> Option<&str> {
        match self {
            ModelSource::Vanilla => None,
            ModelSource::CustomGeo { path } => Some(path.as_str()),
        }
    }
}

impl fmt::Display for ModelSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ModelSource {
    type Err = ModelSourceParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "VANILLA" => Ok(ModelSource::Vanilla),
            "CUSTOM_GEO" => Ok(ModelSource::CustomGeo {
                path: String::new(),
            }),
            other => Err(ModelSourceParseError(other.to_string())),
        }
    }
}

/// Error returned by `ModelSource::from_str`.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("unknown ModelSource: {0} (expected VANILLA or CUSTOM_GEO)")]
pub struct ModelSourceParseError(pub String);

// ── ModelVariants ───────────────────────────────────────────────────────────

/// Subset of the model metadata that the server needs to know
/// about for **decision-making** (load order, hash-based cache
/// invalidation). The client reads the same fields for GeckoLib
/// binding; the server side does NOT actually parse the geo file
/// (task #15 handles that). All three fields are optional — a
/// creature with `None` for everything is treated as "use the
/// vanilla parent's geo".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelVariants {
    /// Path under `assets/create_biocapital/geo/`.
    pub geo_path: Option<String>,
    /// Path under `assets/create_biocapital/textures/entity/`.
    pub texture_path: Option<String>,
    /// Path under `assets/create_biocapital/animations/`.
    pub animation_path: Option<String>,
}

// ── AudioClips ──────────────────────────────────────────────────────────────

/// Audio clip references. Each named field is an OGG path
/// relative to the creature's working directory. `extra` carries
/// arbitrary `key → path` pairs the server does not need to
/// understand (e.g. `"taunt"`, `"breath"`). The Rust side
/// does **not** play sounds; it just round-trips the metadata
/// so `CreatureService.GetCreature` returns the same shape the
/// Java side already understands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioClips {
    pub ambient: Option<String>,
    pub hurt: Option<String>,
    pub death: Option<String>,
    pub step: Option<String>,
    pub attack: Option<String>,
    /// Arbitrary additional sound bindings (e.g. `"taunt"`,
    /// `"breath"`). Keys are user-chosen; values are relative
    /// OGG paths.
    #[serde(default)]
    pub extra: HashMap<String, String>,
}

// ── DropEntry ───────────────────────────────────────────────────────────────

/// One row in the per-creature drop table. `chance` is in
/// `[0.0, 1.0]`; `requires_part_dev` (when present) gates the
/// drop behind a body-part dev-value threshold — e.g. only drop
/// the "petals" item if the player's `GENITAL` dev ≥ `2.0`. This
/// is how 13 §2's `drops_override` field maps to a server-
/// enforceable rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropEntry {
    /// Registry name of the dropped item, e.g.
    /// `"create_biocapital:desire_fragment"`.
    pub item_id: String,
    pub min_count: i32,
    pub max_count: i32,
    /// Drop probability in `[0.0, 1.0]`. 1.0 = always.
    pub chance: f32,
    /// Optional body-part dev gate. `(BodyPart, threshold_dev)`.
    /// `None` means "always eligible".
    #[serde(default)]
    pub requires_part_dev: Option<DropPartDevGate>,
}

/// Body-part dev gate for a `DropEntry`. Mirrors proto wire form:
/// the `part` string is one of the 12 `BodyPart::as_str()` values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropPartDevGate {
    pub part: BodyPart,
    /// Player's `dev_value` for the part must be ≥ this to
    /// trigger the drop. Range `[0.0, 100.0]`.
    pub min_dev: f32,
}

impl DropEntry {
    /// Validate the invariant `0 < min_count ≤ max_count`,
    /// `chance ∈ [0, 1]`, and `min_dev ∈ [0, 100]` when the gate
    /// is present. Called by `CreatureConfig::validate`.
    pub fn validate(&self) -> Result<(), CreatureError> {
        if self.item_id.is_empty() {
            return Err(CreatureError::EmptyItemId);
        }
        if self.min_count < 1 {
            return Err(CreatureError::DropCountTooSmall {
                item: self.item_id.clone(),
                min: self.min_count,
            });
        }
        if self.max_count < self.min_count {
            return Err(CreatureError::DropCountRangeInverted {
                item: self.item_id.clone(),
                min: self.min_count,
                max: self.max_count,
            });
        }
        if self.min_count > MAX_DROP_STACK || self.max_count > MAX_DROP_STACK {
            return Err(CreatureError::DropCountTooLarge {
                item: self.item_id.clone(),
                max: self.max_count,
            });
        }
        if !self.chance.is_finite() || !(0.0..=1.0).contains(&self.chance) {
            return Err(CreatureError::DropChanceOutOfRange {
                item: self.item_id.clone(),
                chance: self.chance,
            });
        }
        if let Some(gate) = &self.requires_part_dev {
            if !gate.min_dev.is_finite() || !(0.0..=100.0).contains(&gate.min_dev) {
                return Err(CreatureError::PartDevGateOutOfRange {
                    item: self.item_id.clone(),
                    min_dev: gate.min_dev,
                });
            }
        }
        Ok(())
    }
}

// ── StatOverrides ───────────────────────────────────────────────────────────

/// Per-creature stat override. Each field is `None` = inherit the
/// vanilla parent's value. All overrides are clamped to
/// "vanilla-sane" ranges in `validate()` so a stray `attack_damage:
/// 1e6` cannot make the mob one-shot a player.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StatOverrides {
    /// Max HP. Range `[1.0, 1024.0]`. `None` = inherit.
    pub max_health: Option<f32>,
    /// Per-hit damage. Range `[0.0, 100.0]`.
    pub attack_damage: Option<f32>,
    /// Movement speed in m/s. Range `[0.0, 1.0]` (vanilla
    /// `Monster`s sit in `[0.2, 0.4]`).
    pub movement_speed: Option<f32>,
    /// Armor points. Range `[0.0, 30.0]` (vanilla netherite
    /// armor caps around 20).
    pub armor: Option<f32>,
    /// Knockback resistance in `[0.0, 1.0]`. 1.0 = immune.
    pub knockback_resistance: Option<f32>,
    /// Follow / aggro range in blocks. Range `[0.0, 256.0]`.
    pub follow_range: Option<f32>,
}

impl StatOverrides {
    pub fn is_empty(&self) -> bool {
        self.max_health.is_none()
            && self.attack_damage.is_none()
            && self.movement_speed.is_none()
            && self.armor.is_none()
            && self.knockback_resistance.is_none()
            && self.follow_range.is_none()
    }

    pub fn validate(&self) -> Result<(), CreatureError> {
        check_range("max_health", self.max_health, 1.0, 1024.0)?;
        check_range("attack_damage", self.attack_damage, 0.0, 100.0)?;
        check_range("movement_speed", self.movement_speed, 0.0, 1.0)?;
        check_range("armor", self.armor, 0.0, 30.0)?;
        check_range("knockback_resistance", self.knockback_resistance, 0.0, 1.0)?;
        check_range("follow_range", self.follow_range, 0.0, 256.0)?;
        Ok(())
    }
}

fn check_range(
    field: &'static str,
    v: Option<f32>,
    lo: f32,
    hi: f32,
) -> Result<(), CreatureError> {
    if let Some(x) = v {
        if !x.is_finite() || !(lo..=hi).contains(&x) {
            return Err(CreatureError::StatOutOfRange {
                field,
                value: x,
                lo,
                hi,
            });
        }
    }
    Ok(())
}

// ── CreatureConfig ──────────────────────────────────────────────────────────

/// Authoritative per-variant metadata. The struct mirrors the
/// `creatures.json` schema in 13 §2.1 — the JSON keys below are
/// the canonical wire names. The proto `CreatureConfig` message
/// (`biocapital.proto`) is a **flat** projection; this struct
/// preserves nesting (e.g. `audio: AudioClips`) so the JSON path
/// stays 1:1 with the on-disk file.
///
/// `enabled = false` rows are kept in PG (task #11 reload
/// semantics) but are not exposed to the gRPC `ListCreatures`
/// response. `mob_replacements` rows reference `creature_id`
/// values; the FK is enforced at the service layer because the
/// `creature_configs` table is JSONB-backed and the
/// `mob_replacements.creature_id` is a string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatureConfig {
    /// Unique snake_case identifier. Wire form: `"variant_zombie"`.
    /// This is the value `mob_replacements.creature_id` references.
    pub id: String,

    /// I18n display name. Key = language tag (`"en_us"`,
    /// `"zh_cn"`); value = display string.
    #[serde(default)]
    pub display_name: HashMap<String, String>,

    /// Where the visual model comes from.
    pub model_source: ModelSource,

    /// Vanilla entity this variant extends. Wire form: a
    /// resource location, e.g. `"minecraft:zombie"`. Used by the
    /// Java side to pick the base class and by the Rust side to
    /// validate `replaces[]`.
    pub entity_type: String,

    /// Model asset paths. `None` on every field = inherit
    /// parent's model.
    #[serde(default)]
    pub model_variants: ModelVariants,

    /// Audio clip bindings. Empty struct = inherit parent's
    /// audio (which is what happens for vanilla-inheriting
    /// variants).
    #[serde(default)]
    pub audio_clips: AudioClips,

    /// Per-creature drop table. Empty = inherit vanilla drops
    /// (the 06 §5.1 contract: "完全等同原版").
    #[serde(default)]
    pub drops: Vec<DropEntry>,

    /// List of vanilla entity-type strings this variant
    /// **replaces**. Multiple entries are valid when the same
    /// visual can serve several vanilla parents (rare).
    #[serde(default)]
    pub replaces: Vec<String>,

    /// Free-form tags. Used by the gRPC layer to filter
    /// `ListCreatures` (e.g. `"monster"`, `"undead"`,
    /// `"biocapital_variant"`). Empty = unfiltered.
    #[serde(default)]
    pub tags: Vec<String>,

    /// When `false`, the row is hidden from `ListCreatures` and
    /// `mob_replacements` lookups return "no match" for this
    /// creature id. Defaults to `true`.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Per-creature stat overrides. Empty struct = inherit all
    /// vanilla stats.
    #[serde(default)]
    pub stat_overrides: StatOverrides,
}

fn default_enabled() -> bool {
    true
}

impl CreatureConfig {
    /// Build the canonical placeholder `variant_zombie` config
    /// (13 §7 default creature list). Used by tests + the
    /// initial seed migration (task #11).
    pub fn placeholder_variant_zombie() -> Self {
        let mut display_name = HashMap::new();
        display_name.insert("en_us".to_string(), "Variant Zombie".to_string());
        display_name.insert("zh_cn".to_string(), "变体僵尸".to_string());
        Self {
            id: "variant_zombie".to_string(),
            display_name,
            model_source: ModelSource::Vanilla,
            entity_type: "minecraft:zombie".to_string(),
            model_variants: ModelVariants::default(),
            audio_clips: AudioClips::default(),
            drops: Vec::new(),
            replaces: vec!["minecraft:zombie".to_string()],
            tags: vec!["monster".to_string(), "undead".to_string()],
            enabled: true,
            stat_overrides: StatOverrides::default(),
        }
    }

    /// Validate the whole config. Called by the JSON loader
    /// and by the gRPC `ReloadCreatures` path.
    pub fn validate(&self) -> Result<(), CreatureError> {
        if self.id.is_empty() {
            return Err(CreatureError::EmptyId);
        }
        if self.id.len() > 64 {
            return Err(CreatureError::IdTooLong {
                id: self.id.clone(),
                max: 64,
            });
        }
        if self.entity_type.is_empty() {
            return Err(CreatureError::EmptyEntityType);
        }
        for d in &self.drops {
            d.validate()?;
        }
        self.stat_overrides.validate()?;
        Ok(())
    }

    /// Display name lookup with a fallback. Returns the
    /// `"en_us"` entry if `lang` is missing; otherwise the
    /// first available entry; otherwise the raw `id`.
    pub fn display_name_for(&self, lang: &str) -> &str {
        if let Some(v) = self.display_name.get(lang) {
            return v.as_str();
        }
        if let Some(v) = self.display_name.get("en_us") {
            return v.as_str();
        }
        if let Some((_, v)) = self.display_name.iter().next() {
            return v.as_str();
        }
        self.id.as_str()
    }
}

// ── Error ───────────────────────────────────────────────────────────────────

/// Error returned by `CreatureConfig::validate`,
/// `DropEntry::validate`, and `StatOverrides::validate`.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum CreatureError {
    #[error("creature id is empty")]
    EmptyId,

    #[error("creature id too long: {id} (max {max})")]
    IdTooLong { id: String, max: usize },

    #[error("entity_type is empty")]
    EmptyEntityType,

    #[error("drop item_id is empty")]
    EmptyItemId,

    #[error("drop count too small for {item}: {min}")]
    DropCountTooSmall { item: String, min: i32 },

    #[error("drop count range inverted for {item}: min={min} max={max}")]
    DropCountRangeInverted { item: String, min: i32, max: i32 },

    #[error("drop count too large for {item}: {max} > {MAX_DROP_STACK}")]
    DropCountTooLarge { item: String, max: i32 },

    #[error("drop chance out of range for {item}: {chance} (expected 0..=1)")]
    DropChanceOutOfRange { item: String, chance: f32 },

    #[error("part_dev gate out of range for {item}: {min_dev} (expected 0..=100)")]
    PartDevGateOutOfRange { item: String, min_dev: f32 },

    #[error("stat {field}={value} out of range [{lo}, {hi}]")]
    StatOutOfRange {
        field: &'static str,
        value: f32,
        lo: f32,
        hi: f32,
    },
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn good_drop() -> DropEntry {
        DropEntry {
            item_id: "create_biocapital:desire_fragment".to_string(),
            min_count: 1,
            max_count: 2,
            chance: 0.03,
            requires_part_dev: None,
        }
    }

    #[test]
    fn validate_accepts_placeholder() {
        let c = CreatureConfig::placeholder_variant_zombie();
        c.validate().expect("placeholder must validate");
    }

    #[test]
    fn validate_rejects_empty_id() {
        let mut c = CreatureConfig::placeholder_variant_zombie();
        c.id = String::new();
        assert!(matches!(c.validate(), Err(CreatureError::EmptyId)));
    }

    #[test]
    fn validate_rejects_empty_entity_type() {
        let mut c = CreatureConfig::placeholder_variant_zombie();
        c.entity_type = String::new();
        assert!(matches!(c.validate(), Err(CreatureError::EmptyEntityType)));
    }

    #[test]
    fn validate_rejects_drop_chance_out_of_range() {
        let mut c = CreatureConfig::placeholder_variant_zombie();
        c.drops.push(DropEntry {
            chance: 1.5,
            ..good_drop()
        });
        assert!(matches!(
            c.validate(),
            Err(CreatureError::DropChanceOutOfRange { .. })
        ));
    }

    #[test]
    fn validate_rejects_drop_count_inverted() {
        let mut c = CreatureConfig::placeholder_variant_zombie();
        c.drops.push(DropEntry {
            min_count: 5,
            max_count: 1,
            ..good_drop()
        });
        assert!(matches!(
            c.validate(),
            Err(CreatureError::DropCountRangeInverted { .. })
        ));
    }

    #[test]
    fn validate_rejects_part_dev_gate_out_of_range() {
        let mut c = CreatureConfig::placeholder_variant_zombie();
        c.drops.push(DropEntry {
            requires_part_dev: Some(DropPartDevGate {
                part: BodyPart::Genital,
                min_dev: 250.0,
            }),
            ..good_drop()
        });
        assert!(matches!(
            c.validate(),
            Err(CreatureError::PartDevGateOutOfRange { .. })
        ));
    }

    #[test]
    fn validate_rejects_stat_out_of_range() {
        let mut c = CreatureConfig::placeholder_variant_zombie();
        c.stat_overrides.max_health = Some(1_000_000.0);
        assert!(matches!(
            c.validate(),
            Err(CreatureError::StatOutOfRange { .. })
        ));
    }

    #[test]
    fn stat_overrides_is_empty_default() {
        let s = StatOverrides::default();
        assert!(s.is_empty());
    }

    #[test]
    fn stat_overrides_is_empty_when_all_none() {
        let s = StatOverrides {
            max_health: None,
            attack_damage: None,
            movement_speed: None,
            armor: None,
            knockback_resistance: None,
            follow_range: None,
        };
        assert!(s.is_empty());
    }

    #[test]
    fn stat_overrides_is_not_empty_with_one_field() {
        let s = StatOverrides {
            max_health: Some(20.0),
            ..StatOverrides::default()
        };
        assert!(!s.is_empty());
    }

    #[test]
    fn model_source_vanilla_geo_path_is_none() {
        let m = ModelSource::Vanilla;
        assert_eq!(m.as_str(), "VANILLA");
        assert!(m.geo_path().is_none());
    }

    #[test]
    fn model_source_custom_geo_carries_path() {
        let m = ModelSource::CustomGeo {
            path: "geo/variant_zombie.geo.json".to_string(),
        };
        assert_eq!(m.as_str(), "CUSTOM_GEO");
        assert_eq!(m.geo_path(), Some("geo/variant_zombie.geo.json"));
    }

    #[test]
    fn model_source_parse_roundtrip_vanilla() {
        let parsed: ModelSource = "VANILLA".parse().unwrap();
        assert_eq!(parsed, ModelSource::Vanilla);
    }

    #[test]
    fn model_source_parse_roundtrip_custom_geo() {
        let parsed: ModelSource = "CUSTOM_GEO".parse().unwrap();
        assert_eq!(
            parsed,
            ModelSource::CustomGeo {
                path: String::new()
            }
        );
    }

    #[test]
    fn model_source_parse_rejects_unknown() {
        let err: ModelSourceParseError = "WEIRD".parse::<ModelSource>().unwrap_err();
        assert_eq!(err.0, "WEIRD");
    }

    #[test]
    fn display_name_lookup_falls_back() {
        let c = CreatureConfig::placeholder_variant_zombie();
        assert_eq!(c.display_name_for("zh_cn"), "变体僵尸");
        assert_eq!(c.display_name_for("en_us"), "Variant Zombie");
        assert_eq!(c.display_name_for("ja_jp"), "Variant Zombie"); // falls back to en_us
    }
}
