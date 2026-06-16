//! Per-tick production cycle for the core pod.
//!
//! Mirrors `04 §2.3` (Rust 重写后的形态) — the formula computation
//! happens on the Rust side; the Java `CorePodBlockEntity` just
//! broadcasts the result into Create's kinetic network.
//!
//! `tick_pod` is a pure function: it takes the current `CorePod` state,
//! a `ProductionFormula`, the host's hunger, and the current server tick,
//! and returns the outcome. The caller is responsible for persisting
//! the modified pod via `CorePodRepository::upsert_pod`.

use super::pod::{CorePod, ProductionFormula, FluidStack};

#[cfg(test)]
use super::pod::PodStatus;

/// Result of one production tick. Drives the gRPC `PodTickResult`
/// response and the `audit_core_pod` row written by the service layer.
#[derive(Debug, Clone, PartialEq)]
pub struct PodTickOutcome {
    /// Stress units contributed to Create's network this tick (matches
    /// `ProductionFormula::stress_per_tick`).
    pub stress_units: f32,
    /// Rotation speed (RPM) provided to Create's kinetic network. The
    /// Java side wires the value to `CorePodBlockEntity.GENERATED_RPM`,
    /// which the current implementation sets to `16.0`. The Rust side
    /// fixes it at `32.0` to match the Create default rotational unit;
    /// see `RPM_DEFAULT` below.
    pub rpm: f32,
    /// Millibuckets remaining in the input tank after this tick.
    pub input_fluid_mb: i32,
    /// Millibuckets present in the output tank after this tick.
    pub output_fluid_mb: i32,
    /// Cumulative byproduct count after this tick.
    pub byproduct_count: i64,
    /// Remaining endurance (ticks).
    pub endurance: f32,
    /// Whether the pod just exhausted its endurance (the caller should
    /// fire `CorePodStateChangeEvent` with `kind = "DEPLETED"`).
    pub depleted: bool,
    /// Whether the tick actually performed a production cycle (vs. a
    /// no-op because of cooldown / hunger / empty tank / depletion).
    pub produced: bool,
}

/// Default RPM broadcast to Create's kinetic network by a hosted pod.
///
/// The Java `CorePodBlockEntity.GENERATED_RPM` constant is `16.0`. The
/// Rust path standardises on `32.0` to align with Create's default
/// stress / SU unit (Create docs: a "small" motor runs at 32 RPM, a
/// "large" one at 64). The discrepancy is intentional — the Rust
/// path is the authoritative one once the gRPC service replaces the
/// Java fallback (see `04 §2.3`).
pub const RPM_DEFAULT: f32 = 32.0;

// ── tick_pod ────────────────────────────────────────────────────────────────

/// Advance one production cycle.
///
/// The caller is expected to have already validated the `host_uuid`
/// against the `input_fluid` (i.e. that the host is still online or
/// that the offline-mode policy applies). `tick_pod` itself only
/// enforces the production prerequisites:
/// 1. `recipe_cooldown == 0`
/// 2. `endurance > 0`
/// 3. If `formula.hunger_above_threshold`, `host_hunger > max_hunger_threshold`
/// 4. `input_fluid.amount_mb >= formula.input_fluid_per_tick`
///
/// Returns `PodTickOutcome` with `produced = false` when any of the
/// above short-circuits, and `depleted = true` when the endurance
/// counter hit zero as a consequence of this tick.
pub fn tick_pod(
    pod: &mut CorePod,
    formula: &ProductionFormula,
    host_hunger: f32,
    current_tick: i64,
) -> PodTickOutcome {
    // 1) Cooldown.
    if pod.recipe_cooldown > 0 {
        pod.recipe_cooldown -= 1;
        pod.updated_tick = current_tick;
        return PodTickOutcome {
            stress_units: 0.0,
            rpm: 0.0,
            input_fluid_mb: pod.input_fluid.as_ref().map(|f| f.amount_mb).unwrap_or(0),
            output_fluid_mb: pod
                .output_fluid
                .as_ref()
                .map(|f| f.amount_mb)
                .unwrap_or(0),
            byproduct_count: pod.byproduct_count,
            endurance: pod.endurance,
            depleted: pod.is_depleted(),
            produced: false,
        };
    }

    // 2) Endurance gate.
    if pod.is_depleted() {
        return PodTickOutcome {
            stress_units: 0.0,
            rpm: 0.0,
            input_fluid_mb: pod.input_fluid.as_ref().map(|f| f.amount_mb).unwrap_or(0),
            output_fluid_mb: pod
                .output_fluid
                .as_ref()
                .map(|f| f.amount_mb)
                .unwrap_or(0),
            byproduct_count: pod.byproduct_count,
            endurance: 0.0,
            depleted: true,
            produced: false,
        };
    }

    // 3) Hunger gate.
    if formula.hunger_above_threshold && host_hunger <= formula.max_hunger_threshold {
        return PodTickOutcome {
            stress_units: 0.0,
            rpm: 0.0,
            input_fluid_mb: pod.input_fluid.as_ref().map(|f| f.amount_mb).unwrap_or(0),
            output_fluid_mb: pod
                .output_fluid
                .as_ref()
                .map(|f| f.amount_mb)
                .unwrap_or(0),
            byproduct_count: pod.byproduct_count,
            endurance: pod.endurance,
            depleted: false,
            produced: false,
        };
    }

    // 4) Input fluid gate.
    let input_amount = pod.input_fluid.as_ref().map(|f| f.amount_mb).unwrap_or(0);
    let needed = formula.input_fluid_per_tick.round() as i32;
    if input_amount < needed {
        return PodTickOutcome {
            stress_units: 0.0,
            rpm: 0.0,
            input_fluid_mb: input_amount,
            output_fluid_mb: pod
                .output_fluid
                .as_ref()
                .map(|f| f.amount_mb)
                .unwrap_or(0),
            byproduct_count: pod.byproduct_count,
            endurance: pod.endurance,
            depleted: false,
            produced: false,
        };
    }

    // ── Production cycle ────────────────────────────────────────
    // 5) Drain input fluid.
    if let Some(in_fluid) = pod.input_fluid.as_mut() {
        in_fluid.amount_mb -= needed;
        if in_fluid.amount_mb <= 0 {
            pod.input_fluid = None;
        }
    }

    // 6) Produce output fluid (default: same as input fluid id).
    let produced_mb = formula.output_fluid_per_tick.round() as i32;
    let output_fluid_id = pod
        .input_fluid
        .as_ref()
        .map(|f| f.fluid_id.clone())
        .unwrap_or_else(|| "create_biocapital:high_tide".to_owned());
    pod.output_fluid = Some(match pod.output_fluid.take() {
        None => FluidStack {
            fluid_id: output_fluid_id.clone(),
            amount_mb: produced_mb,
        },
        Some(mut existing) => {
            existing.amount_mb += produced_mb;
            existing
        }
    });
    let output_fluid_mb = pod
        .output_fluid
        .as_ref()
        .map(|f| f.amount_mb)
        .unwrap_or(0);

    // 7) Accumulate byproducts (desire fragments).
    pod.byproduct_count += formula.byproduct_per_tick.round() as i64;

    // 8) Decrement endurance. The Java path uses ticks; we mirror
    //    that — 1 unit per cycle is far too coarse. Multiply
    //    by `cooldown_ticks` so an online host running 20-tick
    //    cycles loses 1 unit per 20 ticks (= 1 unit / second).
    let endurance_delta = formula.endurance_per_tick * formula.cooldown_ticks as f32;
    pod.endurance = (pod.endurance - endurance_delta).max(0.0);
    let depleted = pod.is_depleted();
    if depleted {
        // 04 §4.4: terminate production and clear host.
        pod.host_uuid = None;
    }

    // 9) Reset cooldown.
    pod.recipe_cooldown = formula.cooldown_ticks;
    pod.updated_tick = current_tick;

    PodTickOutcome {
        stress_units: formula.stress_per_tick,
        rpm: RPM_DEFAULT,
        input_fluid_mb: pod
            .input_fluid
            .as_ref()
            .map(|f| f.amount_mb)
            .unwrap_or(0),
        output_fluid_mb,
        byproduct_count: pod.byproduct_count,
        endurance: pod.endurance,
        depleted,
        produced: true,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn fixture() -> CorePod {
        CorePod {
            world_uuid: Uuid::nil(),
            dimension: "minecraft:overworld".into(),
            pos_x: 0,
            pos_y: 64,
            pos_z: 0,
            host_uuid: Some(Uuid::new_v4()),
            endurance: 100.0,
            recipe_cooldown: 0,
            input_fluid: Some(FluidStack {
                fluid_id: "create_biocapital:high_tide".into(),
                amount_mb: 1000,
            }),
            output_fluid: None,
            byproduct_count: 0,
            created_tick: 0,
            updated_tick: 0,
        }
    }

    #[test]
    fn cooldown_blocks_production() {
        let mut p = fixture();
        p.recipe_cooldown = 5;
        let outcome = tick_pod(&mut p, &ProductionFormula::default(), 10.0, 1);
        assert!(!outcome.produced);
        assert_eq!(outcome.stress_units, 0.0);
        assert_eq!(p.recipe_cooldown, 4);
    }

    #[test]
    fn hunger_below_threshold_blocks_production() {
        let mut p = fixture();
        let outcome = tick_pod(&mut p, &ProductionFormula::default(), 4.0, 1);
        assert!(!outcome.produced);
        // No consumption.
        assert_eq!(
            p.input_fluid.as_ref().map(|f| f.amount_mb),
            Some(1000)
        );
    }

    #[test]
    fn empty_input_blocks_production() {
        let mut p = fixture();
        p.input_fluid = None;
        let outcome = tick_pod(&mut p, &ProductionFormula::default(), 10.0, 1);
        assert!(!outcome.produced);
        assert_eq!(outcome.stress_units, 0.0);
    }

    #[test]
    fn happy_path_produces_fluid_and_stress() {
        let mut p = fixture();
        let outcome = tick_pod(&mut p, &ProductionFormula::default(), 10.0, 1);
        assert!(outcome.produced);
        assert_eq!(outcome.stress_units, 8.0);
        assert_eq!(outcome.rpm, RPM_DEFAULT);
        assert_eq!(outcome.input_fluid_mb, 990);
        assert_eq!(outcome.output_fluid_mb, 8);
        assert_eq!(outcome.byproduct_count, 0); // 0.05 rounds to 0
        assert!(!outcome.depleted);
        assert_eq!(p.recipe_cooldown, 20);
    }

    #[test]
    fn depleted_clears_host() {
        let mut p = fixture();
        p.endurance = 0.01; // one tick of 1.0 delta clears it
        let outcome = tick_pod(&mut p, &ProductionFormula::default(), 10.0, 1);
        assert!(outcome.produced);
        assert!(outcome.depleted);
        assert!(p.host_uuid.is_none());
    }

    #[test]
    fn status_inference() {
        // Sanity-check the wire ↔ domain mapping for `PodStatus` lives
        // here too (it's exported from the `pod` module).
        assert_eq!(PodStatus::Hosted.as_str(), "HOSTED");
    }
}