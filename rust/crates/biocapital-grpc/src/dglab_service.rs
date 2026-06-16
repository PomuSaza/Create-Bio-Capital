//! gRPC service for `DglabService` (`doc/14-rust-services.md` §3.2 +
//! doc/10-hardware-dglab.md §6).
//!
//! 2026-06-14 晚 task #110 (third rewrite) notes — aligned with
//! `20260614000004_dglab.sql`:
//! - Range is **0..=200 per channel** (doc/10 §2.5 — DGLab official
//!   v2 websocket + v3 蓝牙 both confirm). task #97's 0..=100
//!   regression is reverted; wire 0..=200 == PG 0..=200.
//! - `SetStrength` persists `waveform_a` / `waveform_b` to
//!   `dglab_strength_log` (schema already has these columns).
//! - `GenerateToken` is wired through `DglabRepository::upsert_token`
//!   and emits the `dglab.token.generate` audit op.
//! - `audit_dglab.op` widens to **9 values** (adds the override
//!   and config ops from doc/10 §3.4 + §3.5).
//! - **Service extends to 8 RPCs** (proto/biocapital.proto):
//!   - 0 SetStrength
//!   - 1 GetStrength
//!   - 2 GenerateToken
//!   - 3 RevokeToken
//!   - 4 ListTokens
//!   - 5 GetPlayerConfig     (new, 10 §3.2)
//!   - 6 SetPlayerConfig     (new, 10 §3.5)
//!   - 7 AdminOverride       (new, 10 §3.4)
//!
//! The 3 new RPCs (GetPlayerConfig / SetPlayerConfig / AdminOverride)
//! in this file are typed-method only — the implementations live
//! in a follow-up PR that wires the new repos in
//! `DglabServiceDeps` (biocapital-pg task #110 stubs are in place).
//! Method signatures are present so the proto-level dispatch
//! compiles and the trait surface stays stable.

use std::sync::Arc;

use chrono::Utc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use biocapital_dglab::domain::{DglabToken, StrengthSource};
use biocapital_dglab::scheduler::EffectScheduler;
use biocapital_dglab::waveform::WaveformType;
use biocapital_dglab::ws_server::DglabWsServer;
// `DglabAuditWriter` / `DglabOverrideRepository` /
// `DglabRepository` / `PlayerDglabConfigRepository` are only
// referenced by the test module below; `use super::*` re-exports
// the lib's top-level imports.
#[allow(unused_imports)]
use biocapital_pg::{
    DglabAuditEntry, DglabAuditWriter, DglabOverride, DglabOverrideRepository,
    DglabRepository, DglabRepoError, DglabServiceDeps, DglabStrengthLog,
    PlayerDglabConfig, PlayerDglabConfigRepository,
};

// ── Constants ───────────────────────────────────────────────────────────────

/// doc/10 §2.5: wire range is 0..=200 per channel. The PG `CHECK`
/// constraint mirrors this range exactly (task #110 third
/// rewrite; task #97's 0..=100 was a regression). Mirrors
/// `biocapital_pg::dglab::HARDWARE_MAX_STRENGTH`.
pub const HARDWARE_MAX_WIRE: i32 = 200;

// ── Opaque request/response types (proto-shaped) ───────────────────────────

#[derive(Debug, Clone, Default)]
pub struct PlayerIdentifier {
    pub player_uuid: Uuid,
}

#[derive(Debug, Clone, Default)]
pub struct AccountRequest {
    pub player: PlayerIdentifier,
    pub device_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SetStrengthRequest {
    pub player: PlayerIdentifier,
    pub channel: i32, // 0 = A, 1 = B (proto)
    pub strength: i32,
    pub source: String,
    pub request_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct SetStrengthResponse {
    pub success: bool,
    pub new_strength: i32,
    pub max_strength_applied: i32,
    pub event_meta: EventMeta,
}

#[derive(Debug, Clone)]
pub struct StrengthResponse {
    pub channel_a: i32,
    pub channel_b: i32,
    pub max_strength: i32,
    pub player_online: bool,
    pub event_meta: EventMeta,
}

#[derive(Debug, Clone)]
pub struct TokenRequest {
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct TokenResponse {
    pub token: DglabToken,
    pub event_meta: EventMeta,
}

#[derive(Debug, Clone)]
pub struct TokenListResponse {
    pub tokens: Vec<DglabToken>,
    pub total_count: i32,
    pub event_meta: EventMeta,
}

#[derive(Debug, Clone, Default)]
pub struct ListRequest {
    pub limit: i32,
    pub offset: i32,
    pub player_filter: Option<PlayerIdentifier>,
}

// ── 8 RPC 新增 3 个（task #110）─────────────────────────────────────

/// doc/10 §3.2：读玩家自设 base/max/waveform。
#[derive(Debug, Clone, Default)]
pub struct SetPlayerConfigRequest {
    pub player: PlayerIdentifier,
    pub base_intensity: i32,        // 0..=200
    pub max_intensity: i32,         // 0..=200, >= base
    pub waveform_a: String,         // 15 官方 id
    pub waveform_b: String,         // 15 官方 id
}

/// doc/10 §3.4：OP 临时覆写。`param` 5 值之一：
/// "base" | "max" | "waveform_a" | "waveform_b" | "clear"
#[derive(Debug, Clone, Default)]
pub struct AdminOverrideRequest {
    pub target_player: PlayerIdentifier,
    pub param: String,
    pub value_int: Option<i32>,     // 0..=200 当 param=base/max
    pub value_str: Option<String>,  // 15 官方 id 当 param=waveform_a/waveform_b
    pub duration_seconds: i32,      // 持续时间（秒）；0 = 立即清除
    pub issued_by: Uuid,
}

// ── Event meta ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct EventMeta {
    pub kind: String,
    pub payload_json: String,
}

// ── gRPC service trait ──────────────────────────────────────────────────────

#[tonic::async_trait]
pub trait DglabRpc: Send + Sync + 'static {
    async fn set_strength(
        &self,
        request: Request<SetStrengthRequest>,
    ) -> Result<Response<SetStrengthResponse>, Status>;

    async fn get_strength(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<StrengthResponse>, Status>;

    async fn generate_token(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<TokenResponse>, Status>;

    async fn revoke_token(
        &self,
        request: Request<TokenRequest>,
    ) -> Result<Response<TokenResponse>, Status>;

    async fn list_tokens(
        &self,
        request: Request<ListRequest>,
    ) -> Result<Response<TokenListResponse>, Status>;

    // ── task #110 新增 3 RPC（10 §3.2 + §3.4 + §3.5）──

    /// doc/10 §3.2: 读玩家自设 base/max/waveform。
    async fn get_player_config(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<PlayerDglabConfig>, Status>;

    /// doc/10 §3.5: 玩家自己设 base/max/waveform。Rust 端校验
    /// `max >= base` 并写 `audit_dglab.op = "dglab.config.set"`。
    async fn set_player_config(
        &self,
        request: Request<SetPlayerConfigRequest>,
    ) -> Result<Response<PlayerDglabConfig>, Status>;

    /// doc/10 §3.4: OP 临时覆写；写 `dglab_overrides` 表 + 审计
    /// `dglab.override.issue`。返回当前 effective config
    /// (player_dglab_config + 覆写合并视图)。
    async fn admin_override(
        &self,
        request: Request<AdminOverrideRequest>,
    ) -> Result<Response<PlayerDglabConfig>, Status>;
}

// ── Implementation ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct DglabGrpc {
    deps: DglabServiceDeps,
    /// Live DG_LAB WebSocket server. May be `None` when the
    /// hardware binding is disabled by config.
    ws_server: Option<Arc<DglabWsServer>>,
    /// Effect scheduler. See `biocapital_dglab::scheduler`.
    scheduler: Option<Arc<EffectScheduler>>,
    /// Logical clock. Default: `Utc::now().timestamp_millis()`.
    tick_millis: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl DglabGrpc {
    pub fn new(deps: DglabServiceDeps) -> Self {
        Self {
            deps,
            ws_server: None,
            scheduler: None,
            tick_millis: Arc::new(|| Utc::now().timestamp_millis()),
        }
    }

    pub fn with_clock(
        deps: DglabServiceDeps,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            deps,
            ws_server: None,
            scheduler: None,
            tick_millis: clock,
        }
    }

    /// Wire the live DG_LAB WebSocket server (set by the
    /// orchestrator after the WS server has been bound to its
    /// port).
    pub fn with_ws_server(mut self, ws: Arc<DglabWsServer>) -> Self {
        self.ws_server = Some(ws);
        self
    }

    /// Wire the effect scheduler (controls the per-owner single
    /// active effect).
    pub fn with_scheduler(mut self, sched: Arc<EffectScheduler>) -> Self {
        self.scheduler = Some(sched);
        self
    }
}

#[tonic::async_trait]
impl DglabRpc for DglabGrpc {
    // ── SetStrength ─────────────────────────────────────────────
    async fn set_strength(
        &self,
        request: Request<SetStrengthRequest>,
    ) -> Result<Response<SetStrengthResponse>, Status> {
        let SetStrengthRequest {
            player,
            channel,
            strength,
            source,
            request_id,
        } = request.into_inner();

        // Validate the proto enum-as-string.
        let source_enum = StrengthSource::from_wire(&source)
            .ok_or_else(|| Status::invalid_argument(format!("unknown source: {source}")))?;
        if !(0..=1).contains(&channel) {
            return Err(Status::invalid_argument(format!(
                "channel must be 0 (A) or 1 (B), got {channel}"
            )));
        }
        if !(0..=HARDWARE_MAX_WIRE).contains(&strength) {
            return Err(Status::invalid_argument(format!(
                "strength must be 0..={HARDWARE_MAX_WIRE}, got {strength}"
            )));
        }

        // Fetch the prior snapshot (wire 0..=100).
        let mut prior = self
            .deps
            .repo
            .get_strength(player.player_uuid)
            .await
            .map_err(repo_status)?;
        let before_a = prior.current_strength_a;
        let before_b = prior.current_strength_b;

        if channel == 0 {
            prior.current_strength_a = strength;
        } else {
            prior.current_strength_b = strength;
        }
        prior.clamp();

        let tick = (self.tick_millis)();
        // Pick the waveform for the channel being mutated; the
        // other channel's waveform is left as None (i.e. the
        // schema's NULL = "no change since last log row"). The
        // mapping mirrors the scheduler call below (the two
        // sites must agree on which `WaveformType` to use for a
        // given `StrengthSource`).
        let waveform_for = match source_enum {
            StrengthSource::PleasureChange => WaveformType::Aheal,
            StrengthSource::BiocapitalReward => WaveformType::Wave,
            StrengthSource::AdminCmd => WaveformType::Pulse,
            StrengthSource::Client => WaveformType::Continuous,
        };
        let (waveform_a, waveform_b) = if channel == 0 {
            (Some(waveform_for.waveform_id().to_owned()), None)
        } else {
            (None, Some(waveform_for.waveform_id().to_owned()))
        };
        self.deps
            .repo
            .record_strength(&DglabStrengthLog {
                log_id: Uuid::new_v4(),
                owner_uuid: player.player_uuid,
                channel_a: prior.current_strength_a,
                channel_b: prior.current_strength_b,
                waveform_a,
                waveform_b,
                trigger_source: source_enum,
                tick_millis: tick,
                request_id: Some(request_id),
            })
            .await
            .map_err(repo_status)?;

        // Push to the hardware through the WS server (when
        // wired). Failures here are **not** fatal — the audit
        // row is the source of truth; the WS push is a best-
        // effort side effect.
        if let (Some(server), Some(sched)) = (&self.ws_server, &self.scheduler) {
            use biocapital_dglab::scheduler::{EffectRequest, EffectSource as ES};
            let es = match source_enum {
                StrengthSource::PleasureChange => ES::PleasureChange,
                StrengthSource::AdminCmd => ES::AdminCmd,
                StrengthSource::Client => ES::AdminCmd, // map CLIENT → RUST_SERVICE-ish
                StrengthSource::BiocapitalReward => ES::BiocapitalReward,
            };
            // Reuse the same waveform selection that the
            // `dglab_strength_log` row already recorded (the two
            // must agree — if you change one, change the other).
            let intensity_a = if channel == 0 { strength as f64 / 100.0 } else { 0.0 };
            let intensity_b = if channel == 1 { strength as f64 / 100.0 } else { 0.0 };
            let req = EffectRequest::new(es, source.clone(), waveform_for, intensity_a, intensity_b);
            if let Err(e) = sched.submit(req, server).await {
                tracing::warn!(
                    target: "dglab_grpc",
                    "scheduler.submit failed for player={} channel={}: {e}",
                    player.player_uuid,
                    channel
                );
            }
        }

        // Audit row.
        let actor_type = match source_enum {
            StrengthSource::AdminCmd => "ADMIN_CMD",
            StrengthSource::Client => "PLAYER",
            StrengthSource::PleasureChange => "RUST_SERVICE",
            StrengthSource::BiocapitalReward => "RUST_SERVICE",
        };
        let after_a = if channel == 0 { strength } else { before_a };
        let after_b = if channel == 1 { strength } else { before_b };

        self.deps
            .audit
            .write(DglabAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: player.player_uuid,
                actor_type,
                target_owner_uuid: Some(player.player_uuid),
                op: "dglab.strength.set",
                before_strength_a: Some(before_a),
                after_strength_a: Some(after_a),
                before_strength_b: Some(before_b),
                after_strength_b: Some(after_b),
                tick_millis: tick,
                request_id: Some(request_id),
                notes: Some(serde_json::json!({
                    "channel": channel,
                    "value": strength,
                    "source": source,
                })),
            })
            .await
            .map_err(repo_status)?;

        let new_strength = if channel == 0 { after_a } else { after_b };
        Ok(Response::new(SetStrengthResponse {
            success: true,
            new_strength,
            max_strength_applied: HARDWARE_MAX_WIRE,
            event_meta: EventMeta {
                kind: "dglab.strength".to_owned(),
                payload_json: serde_json::json!({
                    "owner_uuid": player.player_uuid,
                    "channel": channel,
                    "new_strength": new_strength,
                    "max_strength_applied": HARDWARE_MAX_WIRE,
                    "source": source,
                })
                .to_string(),
            },
        }))
    }

    // ── GetStrength ─────────────────────────────────────────────
    async fn get_strength(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<StrengthResponse>, Status> {
        let AccountRequest { player, .. } = request.into_inner();
        let mut s = self
            .deps
            .repo
            .get_strength(player.player_uuid)
            .await
            .map_err(repo_status)?;
        // 10 §4.2: offline player → force 0.
        if !s.player_online {
            s.current_strength_a = 0;
            s.current_strength_b = 0;
        }
        s.clamp();
        Ok(Response::new(StrengthResponse {
            channel_a: s.current_strength_a,
            channel_b: s.current_strength_b,
            max_strength: HARDWARE_MAX_WIRE,
            player_online: s.player_online,
            event_meta: EventMeta::default(),
        }))
    }

    // ── GenerateToken ───────────────────────────────────────────
    async fn generate_token(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<TokenResponse>, Status> {
        let AccountRequest { player, .. } = request.into_inner();
        let tick = (self.tick_millis)();
        let now = Utc::now();
        let token = DglabToken {
            token: Uuid::new_v4().to_string(),
            owner_uuid: player.player_uuid,
            created_tick: tick,
            last_used_tick: 0,
            enabled: true,
            created_at: now,
            last_used_at: now,
        };
        token
            .validate()
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        self.deps
            .repo
            .upsert_token(&token)
            .await
            .map_err(repo_status)?;

        self.deps
            .audit
            .write(DglabAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: player.player_uuid,
                actor_type: "PLAYER",
                target_owner_uuid: Some(player.player_uuid),
                op: "dglab.token.generate",
                before_strength_a: None,
                after_strength_a: None,
                before_strength_b: None,
                after_strength_b: None,
                tick_millis: tick,
                request_id: None,
                notes: Some(serde_json::json!({
                    "token": token.token,
                })),
            })
            .await
            .map_err(repo_status)?;

        let token_for_resp = token.clone();
        Ok(Response::new(TokenResponse {
            token: token_for_resp,
            event_meta: EventMeta {
                kind: "dglab.token".to_owned(),
                payload_json: serde_json::json!({
                    "owner_uuid": player.player_uuid,
                    "token": token.token,
                })
                .to_string(),
            },
        }))
    }

    // ── RevokeToken ─────────────────────────────────────────────
    async fn revoke_token(
        &self,
        request: Request<TokenRequest>,
    ) -> Result<Response<TokenResponse>, Status> {
        let TokenRequest { token } = request.into_inner();
        if token.is_empty() {
            return Err(Status::invalid_argument("token is empty"));
        }
        let tick = (self.tick_millis)();
        let revoked = self
            .deps
            .repo
            .revoke_token(&token, tick)
            .await
            .map_err(repo_status)?;

        self.deps
            .audit
            .write(DglabAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: revoked.owner_uuid,
                actor_type: "PLAYER",
                target_owner_uuid: Some(revoked.owner_uuid),
                op: "dglab.token.revoke",
                before_strength_a: None,
                after_strength_a: None,
                before_strength_b: None,
                after_strength_b: None,
                tick_millis: tick,
                request_id: None,
                notes: Some(serde_json::json!({
                    "token": token,
                })),
            })
            .await
            .map_err(repo_status)?;

        let token_for_resp = revoked.clone();
        Ok(Response::new(TokenResponse {
            token: token_for_resp,
            event_meta: EventMeta {
                kind: "dglab.token".to_owned(),
                payload_json: serde_json::json!({
                    "owner_uuid": revoked.owner_uuid,
                    "token": revoked.token,
                    "enabled": revoked.enabled,
                })
                .to_string(),
            },
        }))
    }

    // ── ListTokens ──────────────────────────────────────────────
    async fn list_tokens(
        &self,
        request: Request<ListRequest>,
    ) -> Result<Response<TokenListResponse>, Status> {
        let ListRequest {
            limit,
            offset,
            player_filter,
        } = request.into_inner();
        let owner = match player_filter {
            Some(p) => p.player_uuid,
            None => {
                return Err(Status::invalid_argument(
                    "list_tokens requires a player_filter (10 §3.2)",
                ));
            }
        };
        let mut tokens = self
            .deps
            .repo
            .list_tokens(owner)
            .await
            .map_err(repo_status)?;
        let total = tokens.len() as i32;
        let offset = offset.max(0) as usize;
        if offset >= tokens.len() {
            tokens.clear();
        } else {
            tokens = tokens.split_off(offset);
        }
        let limit = if limit <= 0 { 64 } else { limit.min(1024) } as usize;
        tokens.truncate(limit);

        Ok(Response::new(TokenListResponse {
            tokens,
            total_count: total,
            event_meta: EventMeta::default(),
        }))
    }

    // ── GetPlayerConfig (task #110 新增, doc/10 §3.2) ──────────
    async fn get_player_config(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<PlayerDglabConfig>, Status> {
        let AccountRequest { player, .. } = request.into_inner();
        // Fast-path: read PG `player_dglab_config`; if no row
        // (player never called SetPlayerConfig), return the
        // canonical defaults (10 §3.2) merged with any active
        // overrides.
        let config = match self
            .deps
            .player_config
            .get(player.player_uuid)
            .await
        {
            Ok(c) => c,
            Err(DglabRepoError::ConfigNotFound { .. }) => PlayerDglabConfig {
                player_uuid: player.player_uuid,
                ..PlayerDglabConfig::default()
            },
            Err(e) => return Err(repo_status(e)),
        };
        // Apply any active override for has_active_override +
        // override_expires_at fields.
        let active = self
            .deps
            .overrides
            .list_active_for_player(player.player_uuid)
            .await
            .map_err(repo_status)?;
        let (has_active, expires_at) = match active.first() {
            Some(o) => (true, o.expires_at),
            None => (false, Utc::now()),
        };
        Ok(Response::new(PlayerDglabConfig {
            player_uuid: config.player_uuid,
            base_intensity: config.base_intensity,
            max_intensity: config.max_intensity,
            waveform_a: config.waveform_a,
            waveform_b: config.waveform_b,
            updated_tick: config.updated_tick,
            has_active_override: has_active,
            override_expires_at: if has_active { Some(expires_at) } else { None },
        }))
    }

    // ── SetPlayerConfig (task #110 新增, doc/10 §3.5) ──────────
    async fn set_player_config(
        &self,
        request: Request<SetPlayerConfigRequest>,
    ) -> Result<Response<PlayerDglabConfig>, Status> {
        let SetPlayerConfigRequest {
            player,
            base_intensity,
            max_intensity,
            waveform_a,
            waveform_b,
        } = request.into_inner();

        // Range + max>=base 校验（10 §3.2）：先在 process 内做
        // typed Status::invalid_argument 短路，PG CHECK 兜底。
        if !(0..=HARDWARE_MAX_WIRE).contains(&base_intensity) {
            return Err(Status::invalid_argument(format!(
                "base_intensity must be 0..={HARDWARE_MAX_WIRE}, got {base_intensity}"
            )));
        }
        if !(0..=HARDWARE_MAX_WIRE).contains(&max_intensity) {
            return Err(Status::invalid_argument(format!(
                "max_intensity must be 0..={HARDWARE_MAX_WIRE}, got {max_intensity}"
            )));
        }
        if max_intensity < base_intensity {
            return Err(Status::invalid_argument(format!(
                "max_intensity ({max_intensity}) < base_intensity ({base_intensity})"
            )));
        }
        if waveform_a.is_empty() || waveform_b.is_empty() {
            return Err(Status::invalid_argument(
                "waveform_a / waveform_b must be non-empty".to_owned(),
            ));
        }

        let tick = (self.tick_millis)();
        let config = PlayerDglabConfig {
            player_uuid: player.player_uuid,
            base_intensity: base_intensity as i16,
            max_intensity: max_intensity as i16,
            waveform_a: waveform_a.clone(),
            waveform_b: waveform_b.clone(),
            updated_tick: tick,
            has_active_override: false,
            override_expires_at: None,
        };
        config
            .validate()
            .map_err(|e| Status::invalid_argument(e))?;
        self.deps
            .player_config
            .upsert(&config)
            .await
            .map_err(repo_status)?;

        // Audit row.
        self.deps
            .audit
            .write(DglabAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: player.player_uuid,
                actor_type: "PLAYER",
                target_owner_uuid: Some(player.player_uuid),
                op: "dglab.config.set",
                before_strength_a: None,
                after_strength_a: None,
                before_strength_b: None,
                after_strength_b: None,
                tick_millis: tick,
                request_id: None,
                notes: Some(serde_json::json!({
                    "base_intensity": base_intensity,
                    "max_intensity": max_intensity,
                    "waveform_a": waveform_a,
                    "waveform_b": waveform_b,
                })),
            })
            .await
            .map_err(repo_status)?;

        // Return the freshly-stored config + override status.
        let active = self
            .deps
            .overrides
            .list_active_for_player(player.player_uuid)
            .await
            .map_err(repo_status)?;
        let (has_active, expires_at) = match active.first() {
            Some(o) => (true, o.expires_at),
            None => (false, Utc::now()),
        };
        Ok(Response::new(PlayerDglabConfig {
            player_uuid: config.player_uuid,
            base_intensity: config.base_intensity,
            max_intensity: config.max_intensity,
            waveform_a: config.waveform_a,
            waveform_b: config.waveform_b,
            updated_tick: config.updated_tick,
            has_active_override: has_active,
            override_expires_at: if has_active { Some(expires_at) } else { None },
        }))
    }

    // ── AdminOverride (task #110 新增, doc/10 §3.4) ────────────
    async fn admin_override(
        &self,
        request: Request<AdminOverrideRequest>,
    ) -> Result<Response<PlayerDglabConfig>, Status> {
        let AdminOverrideRequest {
            target_player,
            param,
            value_int,
            value_str,
            duration_seconds,
            issued_by,
        } = request.into_inner();

        // 5 值 param 校验（10 §3.4 + migration CHECK）。
        let param_enum = biocapital_pg::DglabOverrideParam::from_wire(&param)
            .ok_or_else(|| {
                Status::invalid_argument(format!(
                    "invalid dglab override param: {param} \
                     (must be base | max | waveform_a | waveform_b | clear)"
                ))
            })?;

        // value_int 范围校验（仅 base/max 用）。
        if matches!(param_enum, biocapital_pg::DglabOverrideParam::Base | biocapital_pg::DglabOverrideParam::Max) {
            let v = value_int.ok_or_else(|| {
                Status::invalid_argument(format!("value_int required for param={param}"))
            })?;
            if !(0..=HARDWARE_MAX_WIRE).contains(&v) {
                return Err(Status::invalid_argument(format!(
                    "value_int must be 0..={HARDWARE_MAX_WIRE}, got {v}"
                )));
            }
        }

        // duration > 0 (clear 路径允许 0 = 立即清除所有 active 覆写)
        if duration_seconds < 0 {
            return Err(Status::invalid_argument(format!(
                "duration_seconds must be >= 0, got {duration_seconds}"
            )));
        }

        let tick = (self.tick_millis)();
        let now = Utc::now();

        // `clear` 路径：直接 UPDATE active=FALSE 该玩家所有 active 覆写。
        if matches!(param_enum, biocapital_pg::DglabOverrideParam::Clear) {
            let n = self
                .deps
                .overrides
                .clear_for_player(target_player.player_uuid)
                .await
                .map_err(repo_status)?;
            // Audit row.
            self.deps
                .audit
                .write(DglabAuditEntry {
                    log_id: Uuid::new_v4(),
                    actor_uuid: issued_by,
                    actor_type: "ADMIN_CMD",
                    target_owner_uuid: Some(target_player.player_uuid),
                    op: "dglab.override.issue",
                    before_strength_a: None,
                    after_strength_a: None,
                    before_strength_b: None,
                    after_strength_b: None,
                    tick_millis: tick,
                    request_id: None,
                    notes: Some(serde_json::json!({
                        "param": "clear",
                        "cleared_count": n,
                        "issued_by": issued_by,
                    })),
                })
                .await
                .map_err(repo_status)?;
            // Return current effective config (no override).
            return self.get_player_config(Request::new(AccountRequest {
                player: target_player.clone(),
                device_id: None,
            })).await;
        }

        // 颁发新 override 行。
        let expires_at = now + chrono::Duration::seconds(duration_seconds as i64);
        let row = biocapital_pg::DglabOverride {
            override_id: Uuid::new_v4(),
            target_player_uuid: target_player.player_uuid,
            param: param_enum,
            value_int: value_int.map(|v| v as i16),
            value_str: value_str.clone(),
            issued_by,
            issued_at: now,
            expires_at,
            active: true,
        };
        self.deps
            .overrides
            .issue(&row)
            .await
            .map_err(repo_status)?;

        // Audit row.
        self.deps
            .audit
            .write(DglabAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: issued_by,
                actor_type: "ADMIN_CMD",
                target_owner_uuid: Some(target_player.player_uuid),
                op: "dglab.override.issue",
                before_strength_a: None,
                after_strength_a: None,
                before_strength_b: None,
                after_strength_b: None,
                tick_millis: tick,
                request_id: None,
                notes: Some(serde_json::json!({
                    "param": param_enum.as_str(),
                    "value_int": value_int,
                    "value_str": value_str,
                    "duration_seconds": duration_seconds,
                    "override_id": row.override_id,
                    "issued_by": issued_by,
                })),
            })
            .await
            .map_err(repo_status)?;

        // Return effective config (with the new override active).
        Ok(Response::new(PlayerDglabConfig {
            player_uuid: target_player.player_uuid,
            base_intensity: 0,
            max_intensity: 0,
            waveform_a: String::new(),
            waveform_b: String::new(),
            updated_tick: tick,
            has_active_override: true,
            override_expires_at: Some(expires_at),
        }))
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn repo_status(e: DglabRepoError) -> Status {
    use DglabRepoError as R;
    match e {
        R::TokenNotFound { token } => {
            Status::not_found(format!("dglab token {token} not found"))
        }
        R::StateNotFound { owner } => {
            Status::not_found(format!("dglab state for player {owner} not found"))
        }
        R::OverrideNotFound { override_id } => {
            Status::not_found(format!("dglab override {override_id} not found"))
        }
        R::ConfigNotFound { owner } => {
            Status::not_found(format!("dglab config for player {owner} not found"))
        }
        R::InvalidOverrideParam(p) => {
            Status::invalid_argument(format!("invalid dglab override param: {p}"))
        }
        R::Sqlx(sqlx::Error::RowNotFound) => {
            Status::not_found("dglab row not found")
        }
        R::Sqlx(e) => Status::internal(format!("postgres error: {e}")),
        R::Migrate(e) => Status::internal(format!("migration error: {e}")),
        R::InvalidUuid { column, value } => {
            Status::invalid_argument(format!("invalid UUID in {column}: {value}"))
        }
        R::InvalidSource { column, value } => {
            Status::invalid_argument(format!(
                "invalid StrengthSource in {column}: {value}"
            ))
        }
    }
}

// ── Tests (no live DB) ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use biocapital_dglab::domain::Waveform;
    use biocapital_dglab::domain::StrengthState as DomainStrengthState;

    struct MemRepo {
        tokens: Mutex<HashMap<String, DglabToken>>,
        last_log: Mutex<HashMap<Uuid, DglabStrengthLog>>,
    }

    impl MemRepo {
        fn new() -> Self {
            Self {
                tokens: Mutex::new(HashMap::new()),
                last_log: Mutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl DglabRepository for MemRepo {
        async fn upsert_token(&self, token: &DglabToken) -> Result<(), DglabRepoError> {
            let mut g = self.tokens.lock().unwrap();
            for (_k, t) in g.iter_mut() {
                if t.owner_uuid == token.owner_uuid && t.enabled {
                    t.enabled = false;
                }
            }
            g.insert(token.token.clone(), token.clone());
            Ok(())
        }
        async fn revoke_token(
            &self,
            token: &str,
            tick_millis: i64,
        ) -> Result<DglabToken, DglabRepoError> {
            let mut g = self.tokens.lock().unwrap();
            let t = g
                .get_mut(token)
                .ok_or_else(|| DglabRepoError::TokenNotFound { token: token.to_owned() })?;
            t.enabled = false;
            t.last_used_tick = tick_millis;
            Ok(t.clone())
        }
        async fn list_tokens(
            &self,
            owner: Uuid,
        ) -> Result<Vec<DglabToken>, DglabRepoError> {
            let g = self.tokens.lock().unwrap();
            let mut out: Vec<_> = g
                .values()
                .filter(|t| t.owner_uuid == owner)
                .cloned()
                .collect();
            out.sort_by_key(|t| std::cmp::Reverse(t.created_tick));
            Ok(out)
        }
        async fn get_token(
            &self,
            token: &str,
        ) -> Result<DglabToken, DglabRepoError> {
            self.tokens
                .lock()
                .unwrap()
                .get(token)
                .cloned()
                .ok_or_else(|| DglabRepoError::TokenNotFound { token: token.to_owned() })
        }
        async fn record_strength(
            &self,
            log: &DglabStrengthLog,
        ) -> Result<(), DglabRepoError> {
            self.last_log
                .lock()
                .unwrap()
                .insert(log.owner_uuid, log.clone());
            Ok(())
        }
        async fn get_strength(
            &self,
            owner: Uuid,
        ) -> Result<DomainStrengthState, DglabRepoError> {
            Ok(self
                .last_log
                .lock()
                .unwrap()
                .get(&owner)
                .map(|log| DomainStrengthState {
                    max_strength: HARDWARE_MAX_WIRE,
                    current_strength_a: log.channel_a,
                    current_strength_b: log.channel_b,
                    waveform_a: Waveform::default(),
                    waveform_b: Waveform::default(),
                    player_online: true,
                })
                .unwrap_or_default())
        }
    }

    struct MemAudit {
        rows: Mutex<Vec<DglabAuditEntry>>,
    }
    impl MemAudit {
        fn new() -> Self {
            Self { rows: Mutex::new(Vec::new()) }
        }
    }
    #[async_trait]
    impl DglabAuditWriter for MemAudit {
        async fn write(&self, entry: DglabAuditEntry) -> Result<(), DglabRepoError> {
            self.rows.lock().unwrap().push(entry);
            Ok(())
        }
    }

    // task #110 新增：测试用 in-memory override / config repo。
    struct MemOverrides {
        rows: Mutex<HashMap<Uuid, DglabOverride>>,
    }
    impl MemOverrides {
        fn new() -> Self {
            Self { rows: Mutex::new(HashMap::new()) }
        }
    }
    #[async_trait]
    impl DglabOverrideRepository for MemOverrides {
        async fn issue(&self, o: &DglabOverride) -> Result<(), DglabRepoError> {
            self.rows.lock().unwrap().insert(o.override_id, o.clone());
            Ok(())
        }
        async fn sweep_expired(&self, now: chrono::DateTime<chrono::Utc>) -> Result<Vec<Uuid>, DglabRepoError> {
            let mut g = self.rows.lock().unwrap();
            let mut out = Vec::new();
            for (k, v) in g.iter_mut() {
                if v.active && v.expires_at <= now {
                    v.active = false;
                    out.push(*k);
                }
            }
            Ok(out)
        }
        async fn list_active_for_player(&self, p: Uuid) -> Result<Vec<DglabOverride>, DglabRepoError> {
            Ok(self.rows.lock().unwrap().values()
                .filter(|o| o.target_player_uuid == p && o.active)
                .cloned().collect())
        }
        async fn clear_for_player(&self, p: Uuid) -> Result<u64, DglabRepoError> {
            let mut g = self.rows.lock().unwrap();
            let mut n = 0u64;
            for v in g.values_mut() {
                if v.target_player_uuid == p && v.active {
                    v.active = false;
                    n += 1;
                }
            }
            Ok(n)
        }
    }

    struct MemConfigs {
        rows: Mutex<HashMap<Uuid, PlayerDglabConfig>>,
    }
    impl MemConfigs {
        fn new() -> Self {
            Self { rows: Mutex::new(HashMap::new()) }
        }
    }
    #[async_trait]
    impl PlayerDglabConfigRepository for MemConfigs {
        async fn get(&self, p: Uuid) -> Result<PlayerDglabConfig, DglabRepoError> {
            self.rows.lock().unwrap().get(&p).cloned()
                .ok_or(DglabRepoError::ConfigNotFound { owner: p })
        }
        async fn upsert(&self, c: &PlayerDglabConfig) -> Result<(), DglabRepoError> {
            self.rows.lock().unwrap().insert(c.player_uuid, c.clone());
            Ok(())
        }
    }

    fn make_service() -> (DglabGrpc, Arc<MemRepo>, Arc<MemAudit>, Arc<MemOverrides>, Arc<MemConfigs>) {
        let repo = Arc::new(MemRepo::new());
        let audit = Arc::new(MemAudit::new());
        let overrides = Arc::new(MemOverrides::new());
        let configs = Arc::new(MemConfigs::new());
        let deps = DglabServiceDeps::new(
            repo.clone() as Arc<dyn DglabRepository>,
            audit.clone() as Arc<dyn DglabAuditWriter>,
        )
        .with_overrides(overrides.clone() as Arc<dyn DglabOverrideRepository>)
        .with_player_config(configs.clone() as Arc<dyn PlayerDglabConfigRepository>);
        (DglabGrpc::new(deps), repo, audit, overrides, configs)
    }

    #[tokio::test]
    async fn set_strength_records_and_audits() {
        let (svc, _repo, audit, _ov, _cfg) = make_service();
        let player = Uuid::new_v4();
        let resp = svc
            .set_strength(Request::new(SetStrengthRequest {
                player: PlayerIdentifier { player_uuid: player },
                channel: 0,
                strength: 80,
                source: "PLEASURE_CHANGE".to_owned(),
                request_id: Uuid::new_v4(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.new_strength, 80);
        assert_eq!(resp.max_strength_applied, HARDWARE_MAX_WIRE);
        let rows = audit.rows.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].op, "dglab.strength.set");
        assert_eq!(rows[0].actor_type, "RUST_SERVICE");
    }

    #[tokio::test]
    async fn set_strength_rejects_out_of_range() {
        let (svc, _repo, _audit, _ov, _cfg) = make_service();
        let player = Uuid::new_v4();
        let err = svc
            .set_strength(Request::new(SetStrengthRequest {
                player: PlayerIdentifier { player_uuid: player },
                channel: 0,
                strength: HARDWARE_MAX_WIRE + 1,
                source: "CLIENT".to_owned(),
                request_id: Uuid::new_v4(),
            }))
            .await
            .err()
            .unwrap();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn set_strength_rejects_unknown_source() {
        let (svc, _repo, _audit, _ov, _cfg) = make_service();
        let player = Uuid::new_v4();
        let err = svc
            .set_strength(Request::new(SetStrengthRequest {
                player: PlayerIdentifier { player_uuid: player },
                channel: 0,
                strength: 10,
                source: "BOGUS".to_owned(),
                request_id: Uuid::new_v4(),
            }))
            .await
            .err()
            .unwrap();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_strength_default_when_no_log() {
        let (svc, _repo, _audit, _ov, _cfg) = make_service();
        let player = Uuid::new_v4();
        let resp = svc
            .get_strength(Request::new(AccountRequest {
                player: PlayerIdentifier { player_uuid: player },
                device_id: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.channel_a, 0);
        assert_eq!(resp.channel_b, 0);
        assert_eq!(resp.max_strength, HARDWARE_MAX_WIRE);
    }

    #[tokio::test]
    async fn generate_token_replaces_old() {
        let (svc, _repo, audit, _ov, _cfg) = make_service();
        let player = Uuid::new_v4();
        let t1 = svc
            .generate_token(Request::new(AccountRequest {
                player: PlayerIdentifier { player_uuid: player },
                device_id: None,
            }))
            .await
            .unwrap()
            .into_inner()
            .token;
        let t2 = svc
            .generate_token(Request::new(AccountRequest {
                player: PlayerIdentifier { player_uuid: player },
                device_id: None,
            }))
            .await
            .unwrap()
            .into_inner()
            .token;
        assert_ne!(t1.token, t2.token);
        let rows = audit.rows.lock().unwrap();
        let generates = rows
            .iter()
            .filter(|r| r.op == "dglab.token.generate")
            .count();
        assert_eq!(generates, 2);
    }

    #[tokio::test]
    async fn revoke_token_flips_enabled() {
        let (svc, _repo, _audit, _ov, _cfg) = make_service();
        let player = Uuid::new_v4();
        let issued = svc
            .generate_token(Request::new(AccountRequest {
                player: PlayerIdentifier { player_uuid: player },
                device_id: None,
            }))
            .await
            .unwrap()
            .into_inner()
            .token;
        let revoked = svc
            .revoke_token(Request::new(TokenRequest {
                token: issued.token.clone(),
            }))
            .await
            .unwrap()
            .into_inner()
            .token;
        assert!(!revoked.enabled);
    }

    #[tokio::test]
    async fn list_tokens_requires_filter() {
        let (svc, _repo, _audit, _ov, _cfg) = make_service();
        let err = svc
            .list_tokens(Request::new(ListRequest::default()))
            .await
            .err()
            .unwrap();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn list_tokens_paginates() {
        let (svc, _repo, _audit, _ov, _cfg) = make_service();
        let player = Uuid::new_v4();
        for _ in 0..3 {
            let _ = svc
                .generate_token(Request::new(AccountRequest {
                    player: PlayerIdentifier { player_uuid: player },
                    device_id: None,
                }))
                .await
                .unwrap();
        }
        let resp = svc
            .list_tokens(Request::new(ListRequest {
                limit: 2,
                offset: 0,
                player_filter: Some(PlayerIdentifier { player_uuid: player }),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.tokens.len(), 2);
        assert_eq!(resp.total_count, 3);
    }
}
