//! `biocapital-webui` — Web UI HTTP API (axum) per `doc/15-web-ui.md`.
//!
//! Module map:
//! - [`error`]               — typed [`WebUiError`] + JSON envelope
//! - [`auth`]                — Bearer Token admin auth (constant-time compare)
//! - [`state`]               — [`WebUiApp`] state container + config
//! - [`handlers`]            — 8 handler modules (players / bank / contracts
//!                             / devices / audit / admin / auth / events)
//!
//! Route table lives in [`router`]. All routes sit at the root
//! (`/players/...`, `/bank/...`, etc.) — there is no `/api/v1`
//! prefix in this implementation, matching the spec in the user task.
//! `doc/15-web-ui.md §5.1` notes a `/api/v1/` prefix is the
//! forward-looking contract; that change is a v15 follow-up.
//!
//! Authentication: a Bearer Token model. Admin tokens are read
//! from `biocapital-server.toml [WebUI] admin_tokens`. Player
//! viewer tokens are issued in-game via the
//! `/biocapital admin grant_viewer <player>` command (the actual
//! issuance is on the Java side; the Web UI only validates the
//! token via [`auth::verify_viewer_token`]).
//!
//! SSE: the `/events` endpoint streams `BankTransactionEvent` /
//! `DglabStrengthChangeEvent` / `PlayerStateChangeEvent` to
//! authenticated viewers. See [`handlers::events`].

pub mod auth;
pub mod error;
pub mod handlers;
pub mod state;

pub use error::{ErrorCode, WebUiError, WebUiResult};
pub use state::{
    BankTransactionEvent, CreatureConfigReloadEvent, DglabStrengthChangeEvent, EventBus,
    PlayerStateChangeEvent, ServiceBundle, TokenSet, WebUiApp, WebUiConfig, WebUiEvent,
    WhitelistReloadEvent,
};

use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

/// Build the axum [`Router`] for the Web UI HTTP API.
///
/// All 16 routes described in `doc/15-web-ui.md §5.3` plus the
/// SSE endpoint from §8 are mounted here. The state is the
/// [`WebUiApp`] that owns the PG pool + service bundle + admin
/// token list.
pub fn router(state: Arc<WebUiApp>) -> Router {
    Router::new()
        // ── 玩家数据 (15 §5.3) ─────────────────────────────────────
        .route("/players/me", get(handlers::players::get_me))
        .route("/players/:uuid", get(handlers::players::get_player))
        // ── 银行 ───────────────────────────────────────────────────
        .route("/bank/transfer", post(handlers::bank::post_transfer))
        .route("/bank/history", get(handlers::bank::get_history))
        // ── 合约 ───────────────────────────────────────────────────
        .route("/contracts", get(handlers::contracts::list_contracts))
        .route("/contracts/:id", get(handlers::contracts::get_contract))
        .route("/contracts/:id/redeem", post(handlers::contracts::post_redeem))
        // ── DG_LAB 设备 ────────────────────────────────────────────
        .route("/devices", get(handlers::devices::list_devices))
        .route("/devices", post(handlers::devices::post_generate_token))
        .route("/devices/:token/revoke", post(handlers::devices::post_revoke_token))
        // ── 审计 ───────────────────────────────────────────────────
        .route("/audit/query", get(handlers::audit::query_audit))
        .route("/audit/export", get(handlers::audit::export_audit))
        // ── 管理员 ─────────────────────────────────────────────────
        .route("/admin/config/reload", post(handlers::admin::post_config_reload))
        .route("/admin/whitelist/reload", post(handlers::admin::post_whitelist_reload))
        // ── 硬件 token（18 模块，18 §7）───────────────────────────
        .route("/auth/request-token", post(handlers::auth::post_request_hardware_token))
        .route("/auth/bind-hardware", post(handlers::auth::post_bind_hardware))
        .route("/auth/hardware", get(handlers::auth::list_hardware))
        .route("/auth/authenticate", post(handlers::auth::post_authenticate))
        // ── 健康检查 ───────────────────────────────────────────────
        .route("/health", get(handlers::health::get_health))
        // ── SSE（15 §8）───────────────────────────────────────────
        .route("/events", get(handlers::events::sse_events))
        .with_state(state)
}

/// Total number of HTTP routes mounted by [`router`]. Used by
/// integration tests + the integration matrix verification.
pub const ROUTE_COUNT: usize = 17;
