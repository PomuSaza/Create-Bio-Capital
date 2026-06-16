//! `GET /players/me` and `GET /players/:uuid` — player state
//! data contract per `doc/15-web-ui.md §4.1`.
//!
//! Auth: any valid token. Viewers can only see themselves
//! (`/players/me` returns the caller's row, `/players/:uuid`
//! returns 403 when the UUID doesn't match the caller's
//! subject). Admins can see any player.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::Request,
    Json,
};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::{principal_from_req, AuthPrincipal};
use crate::error::{WebUiError, WebUiResult};
use crate::state::WebUiApp;

#[derive(Debug, Serialize)]
pub struct PlayerStateResponse {
    pub player_uuid: String,
    pub player_name: String,
    pub pleasure: f32,
    pub hunger: f32,
    pub hidden_hp: f32,
    pub low_hp_hits: i32,
    pub parts: BTreeMap<String, f32>,
    pub balance: i64,
    pub active_contracts: i32,
    pub defeat_count: i32,
    pub updated_at: String,
}

/// `GET /players/me` — caller must be a viewer token; the
/// response is scoped to that viewer's subject UUID.
pub async fn get_me(
    State(app): State<Arc<WebUiApp>>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<PlayerStateResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;
    let subject = principal.subject().ok_or_else(|| {
        WebUiError::forbidden("admin token cannot use /players/me; use a viewer token")
    })?;
    fetch_player(&app, subject).await.map(Json)
}

/// `GET /players/:uuid` — viewer tokens may only fetch their
/// own UUID (403 otherwise). Admins may fetch any UUID.
pub async fn get_player(
    State(app): State<Arc<WebUiApp>>,
    Path(uuid): Path<Uuid>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<PlayerStateResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;
    match principal {
        AuthPrincipal::Admin => {}
        AuthPrincipal::Viewer { subject } if subject == uuid => {}
        AuthPrincipal::Viewer { .. } => {
            return Err(WebUiError::forbidden(
                "viewer token can only access its own /players/:uuid",
            ));
        }
    }
    fetch_player(&app, uuid).await.map(Json)
}

async fn fetch_player(
    app: &WebUiApp,
    uuid: Uuid,
) -> WebUiResult<PlayerStateResponse> {
    let snapshot = app.services.player_state.get(uuid).await?;
    let account = app.services.bank.get_account_by_owner(uuid).await?;
    let active_contracts = count_active_contracts(&app, uuid).await?;

    // parts: BTreeMap<BodyPart, f32> → JSON object with SCREAMING_SNAKE keys.
    let parts = snapshot
        .parts
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), *v))
        .collect();

    Ok(PlayerStateResponse {
        player_uuid: snapshot.uuid.to_string(),
        // TODO: hook into the player-name table once it exists.
        player_name: String::new(),
        pleasure: snapshot.pleasure,
        hunger: snapshot.hunger,
        hidden_hp: snapshot.hidden_hp,
        low_hp_hits: snapshot.low_hp_hits,
        parts,
        balance: account.balance,
        active_contracts,
        defeat_count: snapshot.defeat_count,
        updated_at: snapshot.updated_at.to_rfc3339(),
    })
}

async fn count_active_contracts(app: &WebUiApp, uuid: Uuid) -> WebUiResult<i32> {
    // List contracts where the player is either the proposer
    // (master) or the acceptor (slave). Filter to open states
    // (PROPOSED / ACTIVE).
    let all = app
        .services
        .contract
        .list(Some(uuid), None, None, 1000)
        .await?;
    let as_acceptor = app
        .services
        .contract
        .list(None, Some(uuid), None, 1000)
        .await?;
    let count = all
        .into_iter()
        .chain(as_acceptor.into_iter())
        .filter(|c| {
            let s = c.status.as_str();
            s == "PROPOSED" || s == "ACTIVE"
        })
        .count();
    Ok(count as i32)
}
