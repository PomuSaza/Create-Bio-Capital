//! Hosting state transitions — `enter_pod` / `exit_pod`.
//!
//! Mirrors `04 §4.2` (进入限制) / `04 §4.3` (退出) /
//! `04 §4.4` (离线托管). All error variants are stringified by
//! `PodError::reason()` so the gRPC layer can drop them straight
//! into the `PodEnterResponse.reason` field (proto enum-as-string).
//!
//! The Java side has equivalent logic in
//! `mo.dystopia.biocapital.block.CorePodHosting.enterPod` (called from
//! `CorePodBlock.useWithoutItem`). The Rust path is authoritative
//! once the gRPC service replaces the Java fallback.

use thiserror::Error;
use uuid::Uuid;

use super::pod::{CorePod, INPUT_FLUID_MIN_ENTER_MB, MAX_HUNGER_THRESHOLD};

// ── Errors ──────────────────────────────────────────────────────────────────

/// Reason an enter request was refused. The `reason()` method yields the
/// wire string used in `PodEnterResponse.reason` (proto enum-as-string).
#[derive(Debug, Error, PartialEq)]
pub enum PodError {
    /// The pod already has a host and the caller is a different player.
    #[error("pod is already occupied")]
    Occupied,

    /// The caller's hunger is `<= MAX_HUNGER_THRESHOLD` (4.5 by default;
    /// the threshold is 5.0 in the spec).
    #[error("host hunger is below the minimum threshold of {0}")]
    HungerTooLow(f32),

    /// The pod's input tank does not hold at least `INPUT_FLUID_MIN_ENTER_MB`
    /// millibuckets of bio fluid.
    #[error("input fluid is below the minimum {0} mB")]
    NoFluid(i32),

    /// The pod has zero endurance.
    #[error("pod endurance is exhausted")]
    NoEndurance,

    /// The pod has no host to exit.
    #[error("pod has no current host to exit")]
    NotHosted,
}

impl PodError {
    /// Wire form used in `PodEnterResponse.reason` (see proto doc on
    /// `PodEnterResponse.reason` — the field is free-form, but the Java
    /// `CorePodHosting.enterPod` only ever returns these four strings).
    pub fn reason(&self) -> &'static str {
        match self {
            PodError::Occupied => "occupied",
            PodError::HungerTooLow(_) => "hunger",
            PodError::NoFluid(_) => "no_fluid",
            PodError::NoEndurance => "no_endurance",
            PodError::NotHosted => "not_hosted",
        }
    }
}

// ── enter_pod ───────────────────────────────────────────────────────────────

/// Outcome of a successful `enter_pod` call. Maps onto the proto
/// `PodEnterResponse.accepted = true` shape.
#[derive(Debug, Clone, PartialEq)]
pub struct PodEnterResult {
    /// The host UUID after the call (always equal to `player_uuid`).
    pub host_uuid: Uuid,
}

/// Try to put `player_uuid` into the pod.
///
/// Per `04 §4.2`:
///   - hunger must be strictly greater than `MAX_HUNGER_THRESHOLD`
///   - endurance must be `> 0`
///   - input fluid mB must be `>= INPUT_FLUID_MIN_ENTER_MB` (100 mB)
///
/// The function returns `Ok(...)` if the player was set as the host,
/// or `Err(PodError)` otherwise. The pod's mutable state is **not**
/// changed when the function returns `Err` — we only mutate on success.
pub fn enter_pod(
    pod: &mut CorePod,
    player_uuid: Uuid,
    hunger: f32,
    endurance: f32,
    input_fluid_mb: i32,
) -> Result<PodEnterResult, PodError> {
    if let Some(current) = pod.host_uuid {
        if current != player_uuid {
            return Err(PodError::Occupied);
        }
        // Same player re-entering — refresh the host record but
        // accept unconditionally (matches CorePodHosting.enterPod
        // behaviour).
        return Ok(PodEnterResult { host_uuid: player_uuid });
    }

    if hunger <= MAX_HUNGER_THRESHOLD {
        return Err(PodError::HungerTooLow(MAX_HUNGER_THRESHOLD));
    }

    if endurance <= 0.0 {
        return Err(PodError::NoEndurance);
    }

    if input_fluid_mb < INPUT_FLUID_MIN_ENTER_MB {
        return Err(PodError::NoFluid(INPUT_FLUID_MIN_ENTER_MB));
    }

    pod.host_uuid = Some(player_uuid);
    Ok(PodEnterResult { host_uuid: player_uuid })
}

// ── exit_pod ────────────────────────────────────────────────────────────────

/// Outcome of a successful `exit_pod` call. Maps onto the proto
/// `PodExitResponse.success = true` shape.
#[derive(Debug, Clone, PartialEq)]
pub struct PodExitResult {
    /// The host UUID **before** the exit. `None` when the pod was idle.
    pub previous_host: Option<Uuid>,
}

/// Remove the host from the pod. Per `04 §4.3` the output fluid is
/// retained (we do **not** drain `output_fluid` here).
///
/// Returns `Err(PodError::NotHosted)` if the pod was idle.
pub fn exit_pod(pod: &mut CorePod) -> Result<PodExitResult, PodError> {
    let previous = pod.host_uuid.take();
    match previous {
        None => Err(PodError::NotHosted),
        Some(uuid) => Ok(PodExitResult { previous_host: Some(uuid) }),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> CorePod {
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

    #[test]
    fn enter_succeeds_for_fresh_player() {
        let mut p = fixture();
        let player = Uuid::new_v4();
        let result = enter_pod(&mut p, player, 10.0, 100.0, 200).unwrap();
        assert_eq!(result.host_uuid, player);
        assert_eq!(p.host_uuid, Some(player));
    }

    #[test]
    fn enter_rejects_when_occupied_by_other_player() {
        let mut p = fixture();
        p.host_uuid = Some(Uuid::new_v4());
        let me = Uuid::new_v4();
        let err = enter_pod(&mut p, me, 10.0, 100.0, 200).unwrap_err();
        assert_eq!(err, PodError::Occupied);
        assert_eq!(err.reason(), "occupied");
    }

    #[test]
    fn enter_rejects_low_hunger() {
        let mut p = fixture();
        let err = enter_pod(&mut p, Uuid::new_v4(), 4.5, 100.0, 200).unwrap_err();
        assert_eq!(err, PodError::HungerTooLow(MAX_HUNGER_THRESHOLD));
        assert_eq!(err.reason(), "hunger");
    }

    #[test]
    fn enter_rejects_no_endurance() {
        let mut p = fixture();
        let err = enter_pod(&mut p, Uuid::new_v4(), 10.0, 0.0, 200).unwrap_err();
        assert_eq!(err, PodError::NoEndurance);
        assert_eq!(err.reason(), "no_endurance");
    }

    #[test]
    fn enter_rejects_no_fluid() {
        let mut p = fixture();
        let err = enter_pod(&mut p, Uuid::new_v4(), 10.0, 100.0, 50).unwrap_err();
        assert_eq!(err, PodError::NoFluid(INPUT_FLUID_MIN_ENTER_MB));
        assert_eq!(err.reason(), "no_fluid");
    }

    #[test]
    fn exit_clears_host_and_returns_previous() {
        let mut p = fixture();
        let host = Uuid::new_v4();
        p.host_uuid = Some(host);
        let result = exit_pod(&mut p).unwrap();
        assert_eq!(result.previous_host, Some(host));
        assert!(p.host_uuid.is_none());
    }

    #[test]
    fn exit_rejects_idle_pod() {
        let mut p = fixture();
        let err = exit_pod(&mut p).unwrap_err();
        assert_eq!(err, PodError::NotHosted);
        assert_eq!(err.reason(), "not_hosted");
    }
}