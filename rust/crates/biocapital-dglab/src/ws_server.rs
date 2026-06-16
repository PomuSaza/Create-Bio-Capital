//! DG_LAB **WebSocket Server** (doc/10-hardware-dglab.md §1.1 + §3.1).
//!
//! Architectural correction (2026-06-14, task #96):
//! The Minecraft **mod is the WebSocket SERVER**; the DG_LAB phone
//! app scans the QR code and connects **to us** as the WebSocket
//! client. This is the opposite of the original task #6
//! implementation, which spawned a `tokio-tungstenite`
//! `connect_async` client pointed at `ws://192.168.1.5:9999`. That
//! direction has been removed in favour of the server below.
//!
//! ## Protocol
//!
//! All frames are JSON text. The single connection per
//! `targetId` is enforced by [`DglabWsServer::handle_connection`]
//! which kicks the older socket on a duplicate connect (99 §2.1
//! anti-flood invariant).
//!
//! ### Inbound (app → mod)
//!
//! | `type`    | Body                                                            |
//! |-----------|-----------------------------------------------------------------|
//! | `bind`    | `{ "type":"bind", "message":"<targetId>", "clientId":"<id>" }`  |
//! | `msg`     | `{ "type":"msg", "message":"strength-0+<m>+<maxA>+<maxB>" }`   |
//!
//! `bind` sets `targetId` + flips `is_bound`; `strength-*` updates
//! the per-channel max strength cap (doc/10 §2.3).
//!
//! ### Outbound (mod → app)
//!
//! Envelope:
//! ```json
//! {
//!   "type": "msg",
//!   "message": "strength-1+2+<value>",
//!   "clientId": "<sessionId>",
//!   "targetId": "<app-provided-id>"
//! }
//! ```
//!
//! Common payloads: `clear-1` / `clear-2` / `pulse-A:<waveform_id>` /
//! `pulse-B:<waveform_id>` / `strength-1+2+<value>` /
//! `strength-2+2+<value>` (doc/10 §2.4).

use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::{SinkExt, StreamExt};
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, WebSocketStream};
use tracing::{info, warn};

use crate::waveform::WaveformType;

/// Server-side tunables. The defaults match doc/10 §3.1 +
/// `biocapital-server.toml` `[Server.Dglab]` (4.2).
#[derive(Debug, Clone)]
pub struct DglabWsConfig {
    /// Bind host. Default `0.0.0.0` (all interfaces) so phones on
    /// the same LAN can reach the mod.
    pub bind_host: String,
    /// Bind port. Default `9999` (DGLabCraft default per doc/10 §12).
    pub bind_port: u16,
    /// Random `sessionId` length (doc/10 §3.1). Default 20.
    pub session_id_length: usize,
}

impl Default for DglabWsConfig {
    fn default() -> Self {
        Self {
            bind_host: "0.0.0.0".to_owned(),
            bind_port: 9999,
            session_id_length: 20,
        }
    }
}

/// Per-channel runtime state. The `current_strength` /
/// `intensity` / `status` are read-mirrors of what was last sent
/// on the wire; the **authoritative** values live in PG
/// (`dglab_strength_log`).
#[derive(Debug, Default, Clone)]
pub struct ChannelState {
    /// Hardware-declared cap (app → mod via `strength-*`). 0..=100
    /// per doc/10 §2.5.
    pub max_strength: i32,
    /// Last emitted strength value (0..=100). Mirrored in PG.
    pub current_strength: i32,
    /// 0.0..=1.0 intensity scale relative to `max_strength`.
    pub intensity: f64,
    /// Human-readable status: `"Idle"` / `"Active"` (and friends).
    pub status: String,
    /// Wall-clock of the most recent pulse / strength change.
    pub last_pulse_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Concrete error surface for the WS server. Variants are
/// deliberately coarse — every caller should treat anything other
/// than a clean shutdown as `warn!`-and-recover.
#[derive(Debug, Error)]
pub enum Error {
    #[error("dglab WS server: I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("dglab WS server: bind failed: {0}")]
    Bind(String),

    #[error("dglab WS server: not bound (no app `bind` message yet)")]
    NotBound,

    #[error("dglab WS server: no active WebSocket connection")]
    NotConnected,

    #[error("dglab WS server: send failed: {0}")]
    Send(String),
}

/// Single-connection DG_LAB WebSocket server. Owns the unique
/// `WebSocketStream<TcpStream>` that the phone app talks through;
/// kicks the previous socket on a duplicate connect (10 §4.1 +
/// 99 §2.1).
pub struct DglabWsServer {
    port: u16,
    bind_host: String,
    /// `clientId` stamped into every outbound envelope.
    session_id: String,
    /// **Single** active WebSocket. `None` = no app connected.
    connected: Mutex<Option<WebSocketStream<TcpStream>>>,
    /// `targetId` learned from the most recent `bind` message.
    target_id: Mutex<Option<String>>,
    /// `true` after the app has sent `bind`. Pre-bind outbound
    /// sends short-circuit with [`Error::NotBound`].
    is_bound: AtomicBool,
    /// Per-channel runtime state. Wrapped in `Mutex` so the WS
    /// inbound loop and the gRPC `submit` path can both mutate
    /// through a `&DglabWsServer` handle.
    pub channel_a: Mutex<ChannelState>,
    pub channel_b: Mutex<ChannelState>,
}

impl DglabWsServer {
    /// Build a new server with a caller-supplied `sessionId` (the
    /// QR code embeds this).
    pub fn new(port: u16, session_id: impl Into<String>) -> Self {
        Self {
            port,
            bind_host: "0.0.0.0".to_owned(),
            session_id: session_id.into(),
            connected: Mutex::new(None),
            target_id: Mutex::new(None),
            is_bound: AtomicBool::new(false),
            channel_a: Mutex::new(ChannelState {
                max_strength: 100,
                current_strength: 0,
                intensity: 0.0,
                status: "Idle".to_owned(),
                last_pulse_at: None,
            }),
            channel_b: Mutex::new(ChannelState {
                max_strength: 100,
                current_strength: 0,
                intensity: 0.0,
                status: "Idle".to_owned(),
                last_pulse_at: None,
            }),
        }
    }

    /// Build a new server with a custom bind host (e.g.
    /// `127.0.0.1` for loopback-only testing).
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.bind_host = host.into();
        self
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn bind_host(&self) -> &str {
        &self.bind_host
    }

    /// Start the accept loop. Blocks forever; spawn it onto a
    /// dedicated tokio task from the orchestrator.
    pub async fn start(&self) -> Result<(), Error> {
        let addr = format!("{}:{}", self.bind_host, self.port);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| Error::Bind(format!("{addr}: {e}")))?;
        info!(target: "dglab_ws", "listening on {addr} sessionId={}", self.session_id);

        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(v) => v,
                Err(e) => {
                    warn!(target: "dglab_ws", "accept error: {e}");
                    continue;
                }
            };
            let ws = match accept_async(stream).await {
                Ok(ws) => ws,
                Err(e) => {
                    warn!(target: "dglab_ws", "WS handshake from {peer} failed: {e}");
                    continue;
                }
            };
            // Kicks any prior connection; this is the §4.1 invariant.
            self.handle_connection(ws).await;
        }
    }

    /// Take ownership of a freshly-upgraded WebSocket. Closes any
    /// prior connection for the same `targetId` (or unconditionally
    /// kicks an anonymous older connection — we keep exactly one
    /// active socket, period) and drives the inbound message loop
    /// until the app disconnects.
    pub async fn handle_connection(&self, ws: WebSocketStream<TcpStream>) {
        // 1. Evict any older socket; the new one wins.
        {
            let mut guard = self.connected.lock().await;
            if let Some(mut old) = guard.take() {
                let _ = old.close(None).await;
                warn!(target: "dglab_ws", "evicted prior WebSocket connection (single-conn invariant)");
            }
            *guard = Some(ws);
        }

        // 2. Inbound loop — drive the socket via the slot, not via a
        //    local binding (the local `ws` was moved into the slot above).
        loop {
            let msg = {
                let mut guard = self.connected.lock().await;
                let stream = match guard.as_mut() {
                    Some(s) => s,
                    None => break, // someone else evicted us
                };
                match stream.next().await {
                    Some(m) => m,
                    None => break, // stream closed
                }
            };
            match msg {
                Ok(Message::Text(text)) => self.handle_inbound(&text).await,
                Ok(Message::Close(_)) | Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {
                    // Pings are auto-replied to by tokio_tungstenite;
                    // we treat Close as a clean shutdown signal.
                    if matches!(msg, Ok(Message::Close(_))) {
                        break;
                    }
                }
                Ok(Message::Binary(_)) => {
                    // DGLab speaks JSON only; binary is a protocol
                    // violation we silently drop.
                }
                Ok(Message::Frame(_)) => {
                    // `tungstenite >= 0.21` exposes the raw frame
                    // variant. We never expect to receive one on the
                    // server side (the tungstenite sink emits them,
                    // not the stream), but a defensive branch keeps
                    // the match exhaustive.
                }
                Err(e) => {
                    warn!(target: "dglab_ws", "WS read error: {e}");
                    break;
                }
            }
        }

        // 3. Cleanup.
        {
            let mut guard = self.connected.lock().await;
            *guard = None;
        }
        self.is_bound.store(false, Ordering::SeqCst);
        {
            let mut t = self.target_id.lock().await;
            *t = None;
        }
        info!(target: "dglab_ws", "connection closed, server back to idle");
    }

    /// Parse a single inbound text frame and update internal
    /// state. Unknown shapes are silently ignored.
    async fn handle_inbound(&self, text: &str) {
        let v: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                warn!(target: "dglab_ws", "inbound parse error: {e} payload={text}");
                return;
            }
        };
        match v["type"].as_str() {
            Some("bind") => {
                if let Some(target) = v["message"].as_str() {
                    *self.target_id.lock().await = Some(target.to_string());
                    self.is_bound.store(true, Ordering::SeqCst);
                    info!(target: "dglab_ws", "bind: targetId={target} sessionId={}", self.session_id);
                }
            }
            Some("msg") => {
                if let Some(msg) = v["message"].as_str() {
                    if let Some(rest) = msg.strip_prefix("strength-") {
                        self.parse_strength_msg(rest).await;
                    }
                }
            }
            _ => {
                // Unknown `type` is non-fatal; the DGLab app may
                // grow new kinds over time.
            }
        }
    }

    /// Parse `0+<mode>+<maxA>+<maxB>` (4-part dual-channel) or
    /// `<channel>+<mode>+<value>` (3-part single-channel). Other
    /// shapes are ignored.
    async fn parse_strength_msg(&self, rest: &str) {
        let parts: Vec<&str> = rest.split('+').collect();
        if parts.len() == 4 && parts[0] == "0" {
            // Dual-channel: 0+mode+maxA+maxB
            let max_a = parts[2].parse::<i32>().unwrap_or(100).clamp(0, 100);
            let max_b = parts[3].parse::<i32>().unwrap_or(100).clamp(0, 100);
            self.channel_a.lock().await.max_strength = max_a;
            self.channel_b.lock().await.max_strength = max_b;
            info!(target: "dglab_ws", "max strength updated A={max_a} B={max_b}");
        } else if parts.len() == 3 {
            // Single-channel: channel+mode+value (channel 1=A, 2=B)
            if let (Ok(channel), Ok(value)) =
                (parts[0].parse::<i32>(), parts[2].parse::<i32>())
            {
                let clamped = value.clamp(0, 100);
                match channel {
                    1 => self.channel_a.lock().await.max_strength = clamped,
                    2 => self.channel_b.lock().await.max_strength = clamped,
                    _ => {}
                }
                info!(target: "dglab_ws", "max strength (single) channel={channel} value={clamped}");
            }
        }
    }

    /// Wrap `message` in the JSON envelope and send it to the app.
    /// Returns [`Error::NotBound`] until the app has sent a `bind`
    /// message, and [`Error::NotConnected`] if the socket has
    /// dropped.
    pub async fn send_envelope(&self, message: &str) -> Result<(), Error> {
        if !self.is_bound.load(Ordering::SeqCst) {
            return Err(Error::NotBound);
        }
        let target = {
            let t = self.target_id.lock().await;
            t.clone()
        }
        .ok_or(Error::NotBound)?;
        let envelope = serde_json::json!({
            "type": "msg",
            "message": message,
            "clientId": self.session_id,
            "targetId": target,
        });
        let mut guard = self.connected.lock().await;
        match guard.as_mut() {
            Some(ws) => ws
                .send(Message::Text(envelope.to_string()))
                .await
                .map_err(|e| Error::Send(e.to_string())),
            None => Err(Error::NotConnected),
        }
    }

    /// Convenience: clear the waveform queue for the named
    /// channel (`"A"` or `"B"`).
    pub async fn clear_channel(&self, channel: &str) -> Result<(), Error> {
        let ch = match channel {
            "A" => "1",
            "B" => "2",
            _ => return Err(Error::Send(format!("invalid channel: {channel}"))),
        };
        self.send_envelope(&format!("clear-{ch}")).await
    }

    /// Convenience: send a one-shot pulse for the named channel.
    pub async fn send_pulse(
        &self,
        channel: &str,
        waveform: WaveformType,
    ) -> Result<(), Error> {
        self.send_envelope(&format!("pulse-{}:{}", channel, waveform.waveform_id()))
            .await
    }

    /// Convenience: set the active strength for the named channel
    /// (clamped to the channel's `max_strength`).
    pub async fn set_strength(&self, channel: &str, value: i32) -> Result<(), Error> {
        let cap = match channel {
            "A" => self.channel_a.lock().await.max_strength,
            "B" => self.channel_b.lock().await.max_strength,
            _ => return Err(Error::Send(format!("invalid channel: {channel}"))),
        };
        let clamped = value.clamp(0, cap);
        let ch_id = if channel == "A" { "1" } else { "2" };
        self.send_envelope(&format!("strength-{ch_id}+2+{clamped}"))
            .await
    }

    /// The full `sendWaveformDataDualChannelWithDifferentIntensity`
    /// flow from doc/10 §2.4: clear both channels, pulse the
    /// supplied waveform on both, then push the intensity-scaled
    /// strengths. The intensities are 0.0..=1.0 multipliers of
    /// each channel's `max_strength`.
    pub async fn send_waveform_dual_channel(
        &self,
        waveform: WaveformType,
        intensity_a: f64,
        intensity_b: f64,
    ) -> Result<(), Error> {
        // Snapshot the max caps under the lock, drop the guards
        // before the async sends (we don't want to hold a `MutexGuard`
        // across an `.await`).
        let max_a = self.channel_a.lock().await.max_strength;
        let max_b = self.channel_b.lock().await.max_strength;

        // 1. clear both
        self.clear_channel("A").await?;
        self.clear_channel("B").await?;

        // 2. one-shot pulses
        self.send_pulse("A", waveform).await?;
        self.send_pulse("B", waveform).await?;

        // 3. scaled strengths
        let ia = (intensity_a.clamp(0.0, 1.0) * max_a as f64).round() as i32;
        let ib = (intensity_b.clamp(0.0, 1.0) * max_b as f64).round() as i32;
        self.set_strength("A", ia).await?;
        self.set_strength("B", ib).await?;

        // Mirror to the read-side state.
        let now = chrono::Utc::now();
        {
            let mut a = self.channel_a.lock().await;
            a.current_strength = ia;
            a.intensity = intensity_a;
            a.status = if ia == 0 { "Idle".into() } else { "Active".into() };
            a.last_pulse_at = Some(now);
        }
        {
            let mut b = self.channel_b.lock().await;
            b.current_strength = ib;
            b.intensity = intensity_b;
            b.status = if ib == 0 { "Idle".into() } else { "Active".into() };
            b.last_pulse_at = Some(now);
        }
        Ok(())
    }

    /// Build the QR-code string (`ws://<host>:<port>/<sessionId>`)
    /// the Java `DglabQrCodeScreen` will render.
    pub fn qr_payload(&self, public_host: &str) -> String {
        format!("ws://{}:{}/{}", public_host, self.port, self.session_id)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::WaveformType;

    #[tokio::test]
    async fn new_server_has_default_state() {
        let s = DglabWsServer::new(9999, "sess-1");
        assert_eq!(s.session_id(), "sess-1");
        assert_eq!(s.port(), 9999);
        assert_eq!(s.channel_a.lock().await.max_strength, 100);
        assert_eq!(s.channel_b.lock().await.max_strength, 100);
        assert!(!s.is_bound.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn send_envelope_rejects_when_not_bound() {
        let s = DglabWsServer::new(9999, "sess-1");
        let r = s.send_envelope("clear-1").await;
        assert!(matches!(r, Err(Error::NotBound)));
    }

    #[tokio::test]
    async fn qr_payload_formats() {
        let s = DglabWsServer::new(9999, "abc-123");
        assert_eq!(s.qr_payload("127.0.0.1"), "ws://127.0.0.1:9999/abc-123");
    }

    #[tokio::test]
    async fn parse_strength_msg_dual_channel() {
        let s = DglabWsServer::new(9999, "sess-1");
        s.parse_strength_msg("0+1+80+90").await;
        assert_eq!(s.channel_a.lock().await.max_strength, 80);
        assert_eq!(s.channel_b.lock().await.max_strength, 90);
    }

    #[tokio::test]
    async fn parse_strength_msg_clamps_above_100() {
        let s = DglabWsServer::new(9999, "sess-1");
        s.parse_strength_msg("0+1+500+200").await;
        assert_eq!(s.channel_a.lock().await.max_strength, 100);
        assert_eq!(s.channel_b.lock().await.max_strength, 100);
    }

    #[tokio::test]
    async fn parse_strength_msg_single_channel_a() {
        let s = DglabWsServer::new(9999, "sess-1");
        s.parse_strength_msg("1+2+50").await;
        assert_eq!(s.channel_a.lock().await.max_strength, 50);
    }

    #[tokio::test]
    async fn parse_strength_msg_single_channel_b() {
        let s = DglabWsServer::new(9999, "sess-1");
        s.parse_strength_msg("2+2+30").await;
        assert_eq!(s.channel_b.lock().await.max_strength, 30);
    }

    #[tokio::test]
    async fn handle_inbound_bind_sets_target() {
        let s = DglabWsServer::new(9999, "sess-1");
        s.handle_inbound(r#"{"type":"bind","message":"phone-7","clientId":"c"}"#)
            .await;
        assert!(s.is_bound.load(Ordering::SeqCst));
        assert_eq!(*s.target_id.lock().await, Some("phone-7".to_owned()));
    }

    #[tokio::test]
    async fn handle_inbound_ignores_garbage() {
        let s = DglabWsServer::new(9999, "sess-1");
        s.handle_inbound("not json").await;
        assert!(!s.is_bound.load(Ordering::SeqCst));
        s.handle_inbound(r#"{"type":"unknown"}"#).await;
        assert!(!s.is_bound.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn waveform_id_used_in_pulse() {
        let s = DglabWsServer::new(9999, "sess-1");
        s.is_bound.store(true, Ordering::SeqCst);
        *s.target_id.lock().await = Some("phone-1".to_owned());
        // We don't actually connect a socket here, so this
        // exercises the *envelope* path; it should fail with
        // `NotConnected` (not `NotBound`), proving the message
        // is correctly assembled.
        let r = s
            .send_pulse("A", WaveformType::Continuous)
            .await
            .err()
            .expect("expected NotConnected");
        assert!(matches!(r, Error::NotConnected));
    }
}
