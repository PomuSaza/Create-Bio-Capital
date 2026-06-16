//! Direct-call helpers for the Sable JNI bridge.
//!
//! `compute_stress` is the **synchronous** SU computation the Java
//! `CorePodBlockEntity.tick()` calls every server tick. It does **not**
//! touch gRPC, **not** persist to PostgreSQL, and **not** write
//! `audit_core_pod`. It is a pure function — `(host_uuid_str,
//! endurance) → stress_units` — intended for low-latency in-process
//! evaluation. See `doc/16-sable-bridge.md` §3.4 (direct compute
//! helpers) and `doc/04-core-pod.md` §2.3 (Rust 重写后的形态).

// ── Constants ───────────────────────────────────────────────────────────────

/// Stress units contributed when endurance is at 100%.
///
/// Mirrors the Java `CorePodBlockEntity.GENERATED_STRESS` default of
/// `4.0`; the Rust path uses `8.0` to give the
/// `(endurance / 100) * 0.8` modifier room to swing without going
/// negative. Tunable via the `[CorePod]` config section (out of scope
/// for task #5 — see 99 §6).
pub const STRESS_BASE: f32 = 8.0;

/// Decay factor applied on top of the `(endurance / 100)` ratio.
///
/// Final formula: `STRESS_BASE * (endurance / 100) * STRESS_DECAY`.
/// The `0.8` factor caps the effective SU at 80% of the base — see
/// `04 §2.2` (默认 4.0 SU) and the design note in the Java block.
pub const STRESS_DECAY: f32 = 0.8;

// ── compute_stress ──────────────────────────────────────────────────────────

/// Compute the per-tick stress units (SU) for a core pod.
///
/// # Arguments
///
/// * `host_uuid_str` — canonical 36-char UUID string of the host
///   player. Empty / unparseable values yield `0.0` — the function
///   never panics on a malformed UUID (matches the Java
///   `NativeRustBindings.computePodStress` contract that returns
///   `0.0f` on null / empty inputs).
/// * `endurance` — current endurance ticks (0..=100). The PG CHECK
///   constraint caps this at 100, but `compute_stress` does not
///   clamp; the caller is expected to have a sanitised value.
///
/// # Returns
///
/// The computed SU as an `f32`. The Java side reads this as a
/// `float` and assigns to `CorePodBlockEntity.calculateAddedStressCapacity`.
pub fn compute_stress(host_uuid_str: &str, endurance: i32) -> f32 {
    // Empty / unparseable UUID → zero (matches Java).
    if host_uuid_str.is_empty() {
        return 0.0;
    }
    if uuid::Uuid::parse_str(host_uuid_str).is_err() {
        return 0.0;
    }
    let endurance_f = endurance.max(0).min(100) as f32;
    STRESS_BASE * (endurance_f / 100.0) * STRESS_DECAY
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_uuid_returns_zero() {
        assert_eq!(compute_stress("", 100), 0.0);
    }

    #[test]
    fn invalid_uuid_returns_zero() {
        assert_eq!(compute_stress("not-a-uuid", 100), 0.0);
    }

    #[test]
    fn full_endurance_yields_base_times_decay() {
        let uuid = uuid::Uuid::new_v4().to_string();
        let s = compute_stress(&uuid, 100);
        assert!((s - STRESS_BASE * STRESS_DECAY).abs() < 1e-6);
        // 8.0 * 1.0 * 0.8 = 6.4
        assert!((s - 6.4).abs() < 1e-6);
    }

    #[test]
    fn half_endurance_halves_the_ratio() {
        let uuid = uuid::Uuid::new_v4().to_string();
        let full = compute_stress(&uuid, 100);
        let half = compute_stress(&uuid, 50);
        assert!((full - 2.0 * half).abs() < 1e-6);
    }

    #[test]
    fn zero_endurance_returns_zero() {
        let uuid = uuid::Uuid::new_v4().to_string();
        assert_eq!(compute_stress(&uuid, 0), 0.0);
    }

    #[test]
    fn out_of_range_endurance_is_clamped() {
        let uuid = uuid::Uuid::new_v4().to_string();
        assert_eq!(compute_stress(&uuid, -5), 0.0);
        assert_eq!(compute_stress(&uuid, 250), compute_stress(&uuid, 100));
    }
}