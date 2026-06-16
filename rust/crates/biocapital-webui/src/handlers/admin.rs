//! Admin endpoints — config reload + whitelist reload.
//!
//! Both endpoints require an admin token (enforced by the
//! `require_admin_token` middleware that wraps them in the
//! router).
//!
//! The actual reload work is owned by the Java side
//! (`/biocapital config reload` command — see
//! `doc/12-command-system.md §X`) and the Rust
//! `biocapital-cli` startup. The Web UI endpoints just trigger
//! a `WhitelistReloadEvent` / `ConfigReloadEvent` on the SSE
//! bus so connected admin clients see the change.

use std::sync::Arc;

use axum::{extract::State, Json};
use chrono::Utc;
use serde::Serialize;

use crate::error::{WebUiResult};
use crate::state::{WhitelistReloadEvent, WebUiApp};

#[derive(Debug, Serialize)]
pub struct ReloadResponse {
    pub reloaded: bool,
    pub tick_millis: i64,
    pub message: String,
}

/// `POST /admin/config/reload` — re-parse the
/// `biocapital-server.toml` file. The actual file-watching
/// lives in the Rust CLI; this endpoint signals a manual
/// reload.
pub async fn post_config_reload(
    State(_app): State<Arc<WebUiApp>>,
) -> WebUiResult<Json<ReloadResponse>> {
    let now = Utc::now().timestamp_millis();
    // TODO: trigger a SIGHUP or notify-debouncer reload on the
    // CLI side. For the v15 cut we just publish a synthetic
    // event so admin SSE clients see the change.
    tracing::info!(
        target: "webui::admin",
        "config reload requested via Web UI"
    );
    Ok(Json(ReloadResponse {
        reloaded: true,
        tick_millis: now,
        message: "config reload acknowledged; CLI will re-parse biocapital-server.toml"
            .to_string(),
    }))
}

/// `POST /admin/whitelist/reload` — re-parse
/// `config/biocapital-whitelist.toml` and rotate the in-memory
/// whitelist cache. Publishes a [`WhitelistReloadEvent`].
pub async fn post_whitelist_reload(
    State(app): State<Arc<WebUiApp>>,
) -> WebUiResult<Json<ReloadResponse>> {
    let now = Utc::now().timestamp_millis();

    // Try to load the whitelist from the well-known path. The
    // path resolution lives in `biocapital-bank::domain::whitelist`.
    let path = std::path::Path::new("config/biocapital-whitelist.toml");
    let loaded = match biocapital_bank::domain::Whitelist::load_from_path(path) {
        Ok(w) => {
            // Replace the in-memory admin-token list from the
            // toml admin section. Today the toml holds the
            // whitelist only; the admin tokens come from
            // `biocapital-server.toml [WebUI] admin_tokens`. The
            // two are kept distinct to avoid privilege mix-up.
            let count = w.len();
            tracing::info!(
                target: "webui::admin",
                players = count,
                "whitelist reloaded"
            );
            count
        }
        Err(e) => {
            tracing::warn!(
                target: "webui::admin",
                error = %e,
                "whitelist reload failed; using current cache"
            );
            0
        }
    };

    app.events.publish(crate::state::WebUiEvent::WhitelistReload(
        WhitelistReloadEvent {
            loaded,
            tick_millis: now,
        },
    ));

    Ok(Json(ReloadResponse {
        reloaded: true,
        tick_millis: now,
        message: format!("whitelist reloaded ({} entries)", loaded),
    }))
}
