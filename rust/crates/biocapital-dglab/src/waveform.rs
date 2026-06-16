//! 15 DG_LAB waveforms (doc/10-hardware-dglab.md §2.6).
//!
//! The DG_LAB hardware / app combo supports 15 named waveforms
//! that the mod transmits via `pulse-A:<waveform_id>` /
//! `pulse-B:<waveform_id>` frames. The wire IDs are the
//! lowercased Rust enum variants. Each variant also has a short
//! human-readable description for logging / audit purposes.

use serde::{Deserialize, Serialize};

/// 15 DG_LAB waveforms. Wire form is the lowercased variant name
/// (e.g. `adamage`, `continuous`); see [`WaveformType::waveform_id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WaveformType {
    /// A-channel damage — used for hostile-mob-triggered feedback.
    Adamage,
    /// B-channel damage.
    Bdamage,
    /// A-channel healing — used for biocapital-reward or recovery.
    Aheal,
    /// B-channel healing.
    Bheal,
    /// Continuous waveform (sustained low-frequency).
    Continuous,
    /// Pulse waveform (rhythmic short bursts).
    Pulse,
    /// Tapping waveform (light percussive).
    Tapping,
    /// Wave waveform (slow oscillation envelope).
    Wave,
    /// Vibration (high-frequency steady).
    Vibration,
    /// Pure sine.
    Sine,
    /// Pure square.
    Square,
    /// Pure triangle.
    Triangle,
    /// Linear ramp.
    Ramp,
    /// Random noise.
    Noise,
    /// User-defined custom waveform (passed through as `custom`).
    Custom,
}

impl WaveformType {
    /// Wire ID — the literal that goes into the `pulse-A:...` /
    /// `pulse-B:...` envelope's `message` field.
    pub fn waveform_id(self) -> &'static str {
        match self {
            Self::Adamage => "adamage",
            Self::Bdamage => "bdamage",
            Self::Aheal => "aheal",
            Self::Bheal => "bheal",
            Self::Continuous => "continuous",
            Self::Pulse => "pulse",
            Self::Tapping => "tapping",
            Self::Wave => "wave",
            Self::Vibration => "vibration",
            Self::Sine => "sine",
            Self::Square => "square",
            Self::Triangle => "triangle",
            Self::Ramp => "ramp",
            Self::Noise => "noise",
            Self::Custom => "custom",
        }
    }

    /// Short description (English) — handy for logs and the Web
    /// UI's audit view.
    pub fn description(self) -> &'static str {
        match self {
            Self::Adamage => "A-channel damage feedback",
            Self::Bdamage => "B-channel damage feedback",
            Self::Aheal => "A-channel healing feedback",
            Self::Bheal => "B-channel healing feedback",
            Self::Continuous => "Sustained continuous",
            Self::Pulse => "Rhythmic pulse",
            Self::Tapping => "Light tapping",
            Self::Wave => "Slow oscillation",
            Self::Vibration => "Steady vibration",
            Self::Sine => "Pure sine",
            Self::Square => "Pure square",
            Self::Triangle => "Pure triangle",
            Self::Ramp => "Linear ramp",
            Self::Noise => "Random noise",
            Self::Custom => "User-defined",
        }
    }

    /// Parse a wire id (case-insensitive). Used when reading
    /// stored `waveform_a` / `waveform_b` from the strength log.
    pub fn from_wire_id(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "adamage" => Some(Self::Adamage),
            "bdamage" => Some(Self::Bdamage),
            "aheal" => Some(Self::Aheal),
            "bheal" => Some(Self::Bheal),
            "continuous" => Some(Self::Continuous),
            "pulse" => Some(Self::Pulse),
            "tapping" => Some(Self::Tapping),
            "wave" => Some(Self::Wave),
            "vibration" => Some(Self::Vibration),
            "sine" => Some(Self::Sine),
            "square" => Some(Self::Square),
            "triangle" => Some(Self::Triangle),
            "ramp" => Some(Self::Ramp),
            "noise" => Some(Self::Noise),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    /// All 15 variants in the canonical order. Useful for
    /// exhaustive tests and the Web UI's "available waveforms"
    /// dropdown.
    pub const ALL: [WaveformType; 15] = [
        Self::Adamage,
        Self::Bdamage,
        Self::Aheal,
        Self::Bheal,
        Self::Continuous,
        Self::Pulse,
        Self::Tapping,
        Self::Wave,
        Self::Vibration,
        Self::Sine,
        Self::Square,
        Self::Triangle,
        Self::Ramp,
        Self::Noise,
        Self::Custom,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_waveform_ids_are_unique_and_lowercase() {
        let mut seen = std::collections::HashSet::new();
        for w in WaveformType::ALL.iter().copied() {
            let id = w.waveform_id();
            assert!(id.chars().all(|c| c.is_ascii_lowercase()), "non-lowercase: {id}");
            assert!(seen.insert(id), "duplicate waveform id: {id}");
        }
        assert_eq!(seen.len(), 15);
    }

    #[test]
    fn round_trip_wire_id() {
        for w in WaveformType::ALL.iter().copied() {
            let id = w.waveform_id();
            assert_eq!(WaveformType::from_wire_id(id), Some(w));
        }
    }

    #[test]
    fn from_wire_id_is_case_insensitive() {
        assert_eq!(WaveformType::from_wire_id("CONTINUOUS"), Some(WaveformType::Continuous));
        assert_eq!(WaveformType::from_wire_id("PuLsE"), Some(WaveformType::Pulse));
        assert_eq!(WaveformType::from_wire_id("nope"), None);
    }
}
