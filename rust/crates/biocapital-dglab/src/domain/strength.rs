//! Strength state (10 §2.3 + §4.2).
//!
//! `pleasure 0..=100` maps to `strength 0..=200` (10 §2.3 formula).
//! The wire range mirrors the DG_LAB hardware's documented maximum
//! of 200 (TODO: confirm with DG_LAB vX — see doc/10 §2.3).

/// Hardware maximum strength value (10 §2.3 / §4.2). Mirrored in
/// `dglab_strength_log.channel_a` / `channel_b` `CHECK` constraints
/// and in `audit_dglab` `before_strength_*` / `after_strength_*`
/// `CHECK` constraints.
pub const MAX_STRENGTH: i32 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrengthSource {
    PleasureChange,
    AdminCmd,
    Client,
    BiocapitalReward,
}

impl StrengthSource {
    /// Wire form persisted in `dglab_strength_log.trigger_source` and
    /// the `audit_dglab.op` discriminators. Mirrors the SQL CHECK
    /// constraint declared in
    /// `rust/migrations/20260614000004_dglab.sql`.
    pub fn as_str(self) -> &'static str {
        match self {
            StrengthSource::PleasureChange => "PLEASURE_CHANGE",
            StrengthSource::AdminCmd => "ADMIN_CMD",
            StrengthSource::Client => "CLIENT",
            StrengthSource::BiocapitalReward => "BIOCAPITAL_REWARD",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "PLEASURE_CHANGE" => Some(StrengthSource::PleasureChange),
            "ADMIN_CMD" => Some(StrengthSource::AdminCmd),
            "CLIENT" => Some(StrengthSource::Client),
            "BIOCAPITAL_REWARD" => Some(StrengthSource::BiocapitalReward),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Waveform {
    pub frequency_hz: f32,
    pub intensity: f32,
}

impl Default for Waveform {
    fn default() -> Self {
        Self {
            frequency_hz: 0.0,
            intensity: 0.0,
        }
    }
}

/// Per-player snapshot of DG_LAB strength state. The PG
/// `dglab_strength_log` is append-only; this struct is the in-memory
/// reconstructed snapshot the gRPC layer exposes to callers
/// (`DglabService.GetStrength`).
#[derive(Debug, Clone, PartialEq)]
pub struct StrengthState {
    pub max_strength: i32,        // server-side cap, 10 §4.2
    pub current_strength_a: i32,  // channel A
    pub current_strength_b: i32,  // channel B
    pub waveform_a: Waveform,
    pub waveform_b: Waveform,
    pub player_online: bool,      // 10 §4.2: offline → forced 0
}

impl Default for StrengthState {
    fn default() -> Self {
        Self {
            max_strength: 200,
            current_strength_a: 0,
            current_strength_b: 0,
            waveform_a: Waveform::default(),
            waveform_b: Waveform::default(),
            player_online: true,
        }
    }
}

impl StrengthState {
    /// Server-side cap (10 §2.3 / §4.2). Clamps the input to the
    /// inclusive `0..=200` range so a poisoned or hostile caller
    /// can't push the hardware past its limit.
    pub fn clamp(&mut self) {
        self.current_strength_a = self.current_strength_a.clamp(0, self.max_strength);
        self.current_strength_b = self.current_strength_b.clamp(0, self.max_strength);
        self.max_strength = self.max_strength.clamp(0, 200);
    }
}
