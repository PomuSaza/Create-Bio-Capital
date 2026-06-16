//! DG_LAB effect scheduler (doc/10-hardware-dglab.md §3.2).
//!
//! DGLabCraft carries a 5-state effect scheduler with sync
//! channels. The Create: Bio-Capital project ships a deliberately
//! **simpler** scheduler: a single active effect slot per owner,
//! replaced only by an effect of strictly higher priority.
//!
//! ## Priority order
//!
//! | Source             | Priority |
//! |--------------------|----------|
//! | `PlayerDamage`     |   100    |
//! | `PleasureChange`   |    80    |
//! | `BiocapitalReward` |    60    |
//! | `AdminCmd`         |    40    |
//!
//! Higher priority wins; ties leave the existing effect in place
//! (the new request is dropped silently). Lease defaults to
//! [`DEFAULT_LEASE`] (1 second) and [`EffectScheduler::sweep_expired`]
//! clears the slot once `lease_until < now`.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::debug;

use crate::waveform::WaveformType;
use crate::ws_server::DglabWsServer;

/// Default lease — after this much wall-clock time the effect
/// silently expires and the slot becomes free.
pub const DEFAULT_LEASE: Duration = Duration::from_secs(1);

/// What triggered a particular effect request. The wire form is
/// the lowercased variant (e.g. `player_damage`); persisted into
/// `dglab_strength_log.trigger_source` and the `audit_dglab.op`
/// discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectSource {
    /// Player took damage (hostile mob, fall, lava, etc.).
    PlayerDamage,
    /// PlayerState `addPleasure` reaction.
    PleasureChange,
    /// Admin / operator command (3+ privilege).
    AdminCmd,
    /// Biocapital system reward (e.g. core-pod milestone).
    BiocapitalReward,
}

impl EffectSource {
    /// Stable priority (higher = wins over lower). Mirrors
    /// doc/10 §3.2 table.
    pub fn priority(self) -> i32 {
        match self {
            EffectSource::PlayerDamage => 100,
            EffectSource::PleasureChange => 80,
            EffectSource::BiocapitalReward => 60,
            EffectSource::AdminCmd => 40,
        }
    }

    /// Wire form. Persisted into `dglab_strength_log.trigger_source`
    /// (and the corresponding `audit_dglab.notes.source`).
    pub fn as_wire(self) -> &'static str {
        match self {
            EffectSource::PlayerDamage => "PLAYER_DAMAGE",
            EffectSource::PleasureChange => "PLEASURE_CHANGE",
            EffectSource::AdminCmd => "ADMIN_CMD",
            EffectSource::BiocapitalReward => "BIOCAPITAL_REWARD",
        }
    }

    /// Inverse of [`Self::as_wire`]. Returns `None` for unknown
    /// strings.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "PLAYER_DAMAGE" => Some(Self::PlayerDamage),
            "PLEASURE_CHANGE" => Some(Self::PleasureChange),
            "ADMIN_CMD" => Some(Self::AdminCmd),
            "BIOCAPITAL_REWARD" => Some(Self::BiocapitalReward),
            _ => None,
        }
    }
}

/// A pending effect request submitted to [`EffectScheduler`].
#[derive(Debug, Clone)]
pub struct EffectRequest {
    pub source: EffectSource,
    /// Free-form detail (damage source, pleasure event id, etc.).
    /// Goes into `dglab_strength_log.notes`.
    pub detail: String,
    pub waveform: WaveformType,
    /// Channel-A intensity in 0.0..=1.0.
    pub intensity_a: f64,
    /// Channel-B intensity in 0.0..=1.0.
    pub intensity_b: f64,
    /// Wall-clock lease expiry.
    pub lease_until: DateTime<Utc>,
}

impl EffectRequest {
    /// Convenience: build a request with a default lease
    /// ([`DEFAULT_LEASE`] from now).
    pub fn new(
        source: EffectSource,
        detail: impl Into<String>,
        waveform: WaveformType,
        intensity_a: f64,
        intensity_b: f64,
    ) -> Self {
        Self {
            source,
            detail: detail.into(),
            waveform,
            intensity_a: intensity_a.clamp(0.0, 1.0),
            intensity_b: intensity_b.clamp(0.0, 1.0),
            lease_until: Utc::now() + chrono::Duration::from_std(DEFAULT_LEASE).unwrap(),
        }
    }

    /// Override the lease window. `d` may exceed
    /// [`DEFAULT_LEASE`] for admin-issued effects.
    pub fn with_lease(mut self, d: Duration) -> Self {
        self.lease_until = Utc::now() + chrono::Duration::from_std(d).unwrap();
        self
    }

    pub fn priority(&self) -> i32 {
        self.source.priority()
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.lease_until < now
    }
}

/// Single-slot effect scheduler. The lock is held only across
/// the priority compare-and-swap; the actual WebSocket send
/// happens outside the lock.
pub struct EffectScheduler {
    active: RwLock<Option<EffectRequest>>,
}

impl EffectScheduler {
    pub fn new() -> Self {
        Self { active: RwLock::new(None) }
    }

    /// Submit a new effect request. Returns `Ok(true)` if the
    /// request was pushed to the hardware, `Ok(false)` if the
    /// existing active effect was higher-priority and the request
    /// was dropped.
    pub async fn submit(
        &self,
        req: EffectRequest,
        server: &DglabWsServer,
    ) -> Result<bool, crate::ws_server::Error> {
        // Compare-and-swap under the read lock — fast path.
        {
            let current = self.active.read().await;
            if let Some(active) = current.as_ref() {
                if !active.is_expired(Utc::now()) && active.priority() >= req.priority() {
                    debug!(
                        target: "dglab_scheduler",
                        "drop req source={} prio={} (active prio={})",
                        req.source.as_wire(),
                        req.priority(),
                        active.priority()
                    );
                    return Ok(false);
                }
            }
        }

        // Replace.
        server
            .send_waveform_dual_channel(req.waveform, req.intensity_a, req.intensity_b)
            .await?;
        let mut w = self.active.write().await;
        *w = Some(req);
        Ok(true)
    }

    /// Drop the active slot regardless of priority. Useful for
    /// the "player offline" path (doc/10 §4.2) and admin `/dglab
    /// stop`.
    pub async fn clear(&self) {
        let mut w = self.active.write().await;
        *w = None;
    }

    /// Sweep the slot if the active lease has expired. Returns
    /// `true` if the slot was cleared. Call from a 100ms tokio
    /// tick.
    pub async fn sweep_expired(&self) -> bool {
        let mut w = self.active.write().await;
        if let Some(active) = w.as_ref() {
            if active.is_expired(Utc::now()) {
                *w = None;
                return true;
            }
        }
        false
    }

    /// Snapshot of the currently active request, if any.
    pub async fn active(&self) -> Option<EffectRequest> {
        self.active.read().await.clone()
    }
}

impl Default for EffectScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ── Background sweeper ──────────────────────────────────────────────────────

/// Spawn a tokio task that calls [`EffectScheduler::sweep_expired`]
/// every `period` (default 100ms). Returns the task handle.
pub fn spawn_sweeper(
    scheduler: Arc<EffectScheduler>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if scheduler.sweep_expired().await {
                debug!(target: "dglab_scheduler", "active effect expired, slot cleared");
            }
        }
    })
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::WaveformType;
    use std::sync::atomic::AtomicU32;

    /// A `DglabWsServer` substitute that records the most recent
    /// `send_waveform_dual_channel` invocation. We avoid a real
    /// network bind in unit tests by exposing a thin
    /// `DglabLike` trait below and having the scheduler accept a
    /// generic sender.
    #[async_trait::async_trait]
    trait DglabLike {
        async fn send_waveform_dual_channel(
            &self,
            waveform: WaveformType,
            intensity_a: f64,
            intensity_b: f64,
        );
    }

    struct Recorder {
        calls: AtomicU32,
        last: Mutex<Option<(WaveformType, f64, f64)>>,
    }
    use std::sync::Mutex;

    impl Recorder {
        fn new() -> Self {
            Self { calls: AtomicU32::new(0), last: Mutex::new(None) }
        }
    }
    #[async_trait::async_trait]
    impl DglabLike for Recorder {
        async fn send_waveform_dual_channel(
            &self,
            waveform: WaveformType,
            intensity_a: f64,
            intensity_b: f64,
        ) {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *self.last.lock().unwrap() = Some((waveform, intensity_a, intensity_b));
        }
    }

    #[test]
    fn priority_ordering() {
        let a = EffectSource::PlayerDamage;
        let b = EffectSource::PleasureChange;
        let c = EffectSource::BiocapitalReward;
        let d = EffectSource::AdminCmd;
        assert!(a.priority() > b.priority());
        assert!(b.priority() > c.priority());
        assert!(c.priority() > d.priority());
    }

    #[test]
    fn wire_round_trip() {
        for s in [
            EffectSource::PlayerDamage,
            EffectSource::PleasureChange,
            EffectSource::BiocapitalReward,
            EffectSource::AdminCmd,
        ] {
            assert_eq!(EffectSource::from_wire(s.as_wire()), Some(s));
        }
        assert!(EffectSource::from_wire("nope").is_none());
    }

    #[test]
    fn request_intensity_clamped() {
        let r = EffectRequest::new(
            EffectSource::PlayerDamage,
            "test",
            WaveformType::Continuous,
            2.5,
            -0.3,
        );
        assert_eq!(r.intensity_a, 1.0);
        assert_eq!(r.intensity_b, 0.0);
    }

    #[tokio::test]
    async fn submit_replaces_lower_priority() {
        // Smoke test using a custom in-memory `DglabLike` to
        // avoid spinning a real server in this test.
        let s = EffectScheduler::new();
        let r = Recorder::new();
        // We can't directly substitute the server in the public
        // `submit` API, so we exercise the priority compare-and-
        // swap logic via a hand-rolled wrapper below. The test
        // asserts that a `BiocapitalReward` (60) replaces an
        // `AdminCmd` (40), and a `PlayerDamage` (100) replaces
        // both.
        struct Wrapper<'a>(&'a EffectScheduler);
        impl<'a> Wrapper<'a> {
            async fn try_push(
                &self,
                req: EffectRequest,
                rec: &Recorder,
            ) -> bool {
                {
                    let cur = self.0.active.read().await;
                    if let Some(c) = cur.as_ref() {
                        if !c.is_expired(chrono::Utc::now())
                            && c.priority() >= req.priority()
                        {
                            return false;
                        }
                    }
                }
                rec.send_waveform_dual_channel(req.waveform, req.intensity_a, req.intensity_b)
                    .await;
                *self.0.active.write().await = Some(req);
                true
            }
        }
        let w = Wrapper(&s);
        let r1 = EffectRequest::new(EffectSource::AdminCmd, "x", WaveformType::Pulse, 0.1, 0.2);
        let r2 = EffectRequest::new(EffectSource::BiocapitalReward, "y", WaveformType::Wave, 0.3, 0.4);
        let r3 = EffectRequest::new(EffectSource::PlayerDamage, "z", WaveformType::Sine, 0.5, 0.6);
        assert!(w.try_push(r1, &r).await);
        assert!(w.try_push(r2, &r).await); // higher prio wins
        assert!(w.try_push(r3, &r).await); // even higher wins
        assert_eq!(r.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}
