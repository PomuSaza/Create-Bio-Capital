//! Core-pod domain types — `CorePod`, `FluidStack`, `ProductionFormula`.
//!
//! See `doc/04-core-pod.md` §1 (方块规格) / §2 (应力输出) / §3 (流体 I/O) /
//! §4 (托管挂机) / §6 (Rust 重写后的形态) and
//! `doc/14-rust-services.md` §3.2 (proto schema).
//!
//! ## Wire ↔ domain
//!
//! The Rust types here are intentionally plain — no sqlx / no prost — so
//! the persistence layer (`biocapital-pg`) and the gRPC layer
//! (`biocapital-grpc`) can both borrow them without taking on the other's
//! dependencies. The `CorePod` is the 5-tuple identifier (`PodIdentifier`)
//! plus the mutable per-pod state. The `ProductionFormula` is the static
//! formula shipped with the mod; the `tick_pod` function consumes one per
//! tick.

use uuid::Uuid;

// ── Constants ───────────────────────────────────────────────────────────────

/// Default value of [`ProductionFormula::stress_per_tick`].
///
/// Mirrors `mo.dystopia.biocapital.block.CorePodBlockEntity.GENERATED_STRESS`
/// on the Java side (04 §2.2). The Java value is 4.0; the **Rust**
/// `ProductionFormula` uses 8.0 to leave headroom for the
/// `(endurance / 100)` modifier applied in [`compute::compute_stress`] —
/// see module-level doc in `compute.rs`.
pub const DEFAULT_STRESS_PER_TICK: f32 = 8.0;

/// Default value of [`ProductionFormula::input_fluid_per_tick`].
/// Mirrors the Java default `1 mB / cycle` scaled up to keep the
/// tick-driven Rust model working at 10 mB / 5 ticks (04 §1 table).
pub const DEFAULT_INPUT_FLUID_PER_TICK: f32 = 10.0;

/// Default value of [`ProductionFormula::output_fluid_per_tick`].
pub const DEFAULT_OUTPUT_FLUID_PER_TICK: f32 = 8.0;

/// Default value of [`ProductionFormula::byproduct_per_tick`].
/// 1 desire fragment per 20 ticks ≈ 0.05 / tick.
pub const DEFAULT_BYPRODUCT_PER_TICK: f32 = 0.05;

/// Default value of [`ProductionFormula::cooldown_ticks`].
/// 20 ticks = 1 second.
pub const DEFAULT_COOLDOWN_TICKS: i64 = 20;

/// Default value of [`ProductionFormula::endurance_per_tick`].
/// 20 minutes of endurance at 20 ticks/s ⇒ 24 000 ticks ⇒ 1 / 24 000 per
/// tick. The Rust production formula rounds to 0.01 (≈ 16 minutes
/// continuous use), which is the canonical "stress endurance" budget
/// used by the mod UI.
pub const DEFAULT_ENDURANCE_PER_TICK: f32 = 0.01;

/// Hunger threshold below which a host cannot enter the pod.
/// Mirrors `04 §4.2` ("玩家饥饿值 < 5") and the proto
/// `PodEnterRequest.hunger_above_5` field. The Java code reads the
/// threshold from the player state; this constant is the Rust mirror.
pub const MAX_HUNGER_THRESHOLD: f32 = 5.0;

/// Minimum input fluid (mB) the tank must hold before `enter_pod`
/// accepts the player. Mirrors `04 §4.2` "无流体输入 → 视觉停留"。
pub const INPUT_FLUID_MIN_ENTER_MB: i32 = 100;

// ── FluidStack ──────────────────────────────────────────────────────────────

/// A fluid id + amount, in millibuckets. Mirrors the proto shape used by
/// `PodState.input_fluid` / `output_fluid` (proto stores them as a
/// `string` + `int32_mb` pair, but the Rust domain unifies them for
/// arithmetic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FluidStack {
    /// Fluid registry id (e.g. `"create_biocapital:high_tide"`).
    /// Mirrors `Mo.dystopia.biocapital.fluid.ModFluids.HIGH_TIDE` on the
    /// Java side.
    pub fluid_id: String,
    /// Amount in millibuckets. Always non-negative; the repository clamps
    /// to `[0, 1000]` (one bucket).
    pub amount_mb: i32,
}

impl FluidStack {
    /// Convenience constructor for an empty stack of a named fluid.
    pub fn empty(fluid_id: impl Into<String>) -> Self {
        Self {
            fluid_id: fluid_id.into(),
            amount_mb: 0,
        }
    }

    /// Returns `true` if the stack has zero mB.
    pub fn is_empty(&self) -> bool {
        self.amount_mb <= 0
    }
}

// ── PodStatus ───────────────────────────────────────────────────────────────

/// High-level state of the pod; mirrors the proto `PodState.status`
/// field and the gRPC `EventMeta.kind` taxonomy (see
/// `doc/14-rust-services.md` §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PodStatus {
    /// No host, no production.
    Idle,
    /// A player is currently hosting (online).
    Hosted,
    /// A player is hosting but is offline; production continues at 2×
    /// endurance cost (04 §4.4).
    OfflineHosted,
    /// Endurance has been exhausted; production halted.
    Depleted,
}

impl PodStatus {
    /// Wire form used in `PodState.status` (proto enum-as-string).
    pub fn as_str(self) -> &'static str {
        match self {
            PodStatus::Idle => "IDLE",
            PodStatus::Hosted => "HOSTED",
            PodStatus::OfflineHosted => "OFFLINE_HOSTED",
            PodStatus::Depleted => "DEPLETED",
        }
    }

    /// Parse the wire form back into the enum. Returns `None` for
    /// unknown strings (defensive — the SQL CHECK constraint should
    /// already prevent this).
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "IDLE" => Some(PodStatus::Idle),
            "HOSTED" => Some(PodStatus::Hosted),
            "OFFLINE_HOSTED" => Some(PodStatus::OfflineHosted),
            "DEPLETED" => Some(PodStatus::Depleted),
            _ => None,
        }
    }
}

// ── ProductionFormula ───────────────────────────────────────────────────────

/// The static production formula applied by `tick_pod`. Defaults are
/// the values listed in `04 §2.2` and the spec at the top of this
/// module (`DEFAULT_*` constants).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProductionFormula {
    /// Stress units contributed to the Create network per tick
    /// (Create's `GENERATED_STRESS` analogue).
    pub stress_per_tick: f32,

    /// Input fluid consumed per tick (mB).
    pub input_fluid_per_tick: f32,

    /// Output fluid produced per tick (mB).
    pub output_fluid_per_tick: f32,

    /// Byproduct accumulation rate (desire fragments / tick).
    pub byproduct_per_tick: f32,

    /// Cooldown between production ticks (ticks). The tick loop
    /// decrements `CorePod::recipe_cooldown`; when it reaches zero,
    /// `tick_pod` performs one cycle and resets.
    pub cooldown_ticks: i64,

    /// Endurance consumed per production tick.
    pub endurance_per_tick: f32,

    /// Whether the caller is required to verify that the host's
    /// hunger is strictly above the threshold. When `true`,
    /// `tick_pod` rejects ticks where `host_hunger <= max_hunger_threshold`.
    pub hunger_above_threshold: bool,

    /// Hunger threshold (the boundary *value* — must be strictly greater
    /// than this to pass). Default 5.0 (matches 04 §4.2 / Java).
    pub max_hunger_threshold: f32,
}

impl Default for ProductionFormula {
    fn default() -> Self {
        Self {
            stress_per_tick: DEFAULT_STRESS_PER_TICK,
            input_fluid_per_tick: DEFAULT_INPUT_FLUID_PER_TICK,
            output_fluid_per_tick: DEFAULT_OUTPUT_FLUID_PER_TICK,
            byproduct_per_tick: DEFAULT_BYPRODUCT_PER_TICK,
            cooldown_ticks: DEFAULT_COOLDOWN_TICKS,
            endurance_per_tick: DEFAULT_ENDURANCE_PER_TICK,
            hunger_above_threshold: true,
            max_hunger_threshold: MAX_HUNGER_THRESHOLD,
        }
    }
}

// ── CorePod ─────────────────────────────────────────────────────────────────

/// One core-pod's durable state. The 5-tuple identifier
/// (`world_uuid`, `dimension`, `pos_x`, `pos_y`, `pos_z`) is the primary
/// key (`core_pods` PRIMARY KEY) and the proto `PodIdentifier` shape.
///
/// This is the canonical state holder; `CorePodBlockEntity` keeps a
/// client-side mirror and asks Rust to compute authoritative values via
/// the Sable JNI bridge.
#[derive(Debug, Clone, PartialEq)]
pub struct CorePod {
    /// World UUID (per-server 5-tuple).
    pub world_uuid: Uuid,
    /// Dimension id, e.g. `"minecraft:overworld"`.
    pub dimension: String,
    /// Block coordinates (matches `BlockPos.getX/Y/Z()`; SQL BIGINT).
    pub pos_x: i64,
    pub pos_y: i64,
    pub pos_z: i64,

    /// Hosting player UUID, if any.
    pub host_uuid: Option<Uuid>,

    /// Remaining endurance in **ticks** (not millibuckets). Range `[0, MAX]`;
    /// the PG CHECK constraint enforces `[0, 100]`, but `MAX` is supplied
    /// by the runtime configuration and may exceed 100 in some test
    /// configurations — the repository clamps on write.
    pub endurance: f32,

    /// Recipe cooldown counter (ticks). When `> 0`, `tick_pod` is a no-op.
    pub recipe_cooldown: i64,

    /// Current input fluid stack. `None` ⇔ tank is empty.
    pub input_fluid: Option<FluidStack>,

    /// Current output fluid stack. `None` ⇔ tank is empty.
    pub output_fluid: Option<FluidStack>,

    /// Cumulative byproduct count (desire fragments). Driven by
    /// `ProductionFormula.byproduct_per_tick`.
    pub byproduct_count: i64,

    /// Server tick (millis) when this pod was first registered.
    pub created_tick: i64,

    /// Server tick (millis) of the last successful write.
    pub updated_tick: i64,
}

impl CorePod {
    /// Returns `true` iff a player is currently hosting (online or offline).
    pub fn is_hosted(&self) -> bool {
        self.host_uuid.is_some()
    }

    /// Returns `true` iff endurance has been exhausted.
    pub fn is_depleted(&self) -> bool {
        self.endurance <= 0.0
    }

    /// Compose the proto-shaped 5-tuple identifier (used by the
    /// repository and gRPC layers).
    pub fn identifier(&self) -> PodIdentifierRef<'_> {
        PodIdentifierRef {
            world_uuid: self.world_uuid,
            dimension: &self.dimension,
            pos_x: self.pos_x,
            pos_y: self.pos_y,
            pos_z: self.pos_z,
        }
    }
}

/// A borrowed view of the 5-tuple identifying a pod. Used as a
/// parameter type where we don't need to clone the strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PodIdentifierRef<'a> {
    pub world_uuid: Uuid,
    pub dimension: &'a str,
    pub pos_x: i64,
    pub pos_y: i64,
    pub pos_z: i64,
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_formula_defaults_match_spec() {
        let f = ProductionFormula::default();
        assert_eq!(f.stress_per_tick, 8.0);
        assert_eq!(f.input_fluid_per_tick, 10.0);
        assert_eq!(f.output_fluid_per_tick, 8.0);
        assert_eq!(f.byproduct_per_tick, 0.05);
        assert_eq!(f.cooldown_ticks, 20);
        assert_eq!(f.endurance_per_tick, 0.01);
        assert!(f.hunger_above_threshold);
        assert_eq!(f.max_hunger_threshold, 5.0);
    }

    #[test]
    fn pod_status_round_trips_wire_form() {
        for s in [
            PodStatus::Idle,
            PodStatus::Hosted,
            PodStatus::OfflineHosted,
            PodStatus::Depleted,
        ] {
            assert_eq!(PodStatus::from_wire(s.as_str()), Some(s));
        }
        assert_eq!(PodStatus::from_wire("nope"), None);
    }

    #[test]
    fn fluid_stack_helpers() {
        let s = FluidStack::empty("create_biocapital:high_tide");
        assert!(s.is_empty());
        assert_eq!(s.fluid_id, "create_biocapital:high_tide");
        assert_eq!(s.amount_mb, 0);
    }

    #[test]
    fn corepod_predicates() {
        let mut p = pod_fixture();
        p.host_uuid = None;
        assert!(!p.is_hosted());
        p.host_uuid = Some(Uuid::new_v4());
        assert!(p.is_hosted());
        p.endurance = 0.0;
        assert!(p.is_depleted());
    }

    fn pod_fixture() -> CorePod {
        CorePod {
            world_uuid: Uuid::nil(),
            dimension: "minecraft:overworld".into(),
            pos_x: 0,
            pos_y: 64,
            pos_z: 0,
            host_uuid: None,
            endurance: 100.0,
            recipe_cooldown: 0,
            input_fluid: None,
            output_fluid: None,
            byproduct_count: 0,
            created_tick: 0,
            updated_tick: 0,
        }
    }
}