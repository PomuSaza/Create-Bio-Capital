//! DG_LAB device endpoints — list / generate / revoke tokens.
//! Data contract per `doc/15-web-ui.md §4.4` + `99 §7`.
//!
//! Auth: viewers see / revoke only their own tokens; admins
//! can act on any player.

use std::sync::Arc;

use axum::{
    extract::{FromRequest, Path, Query, State},
    http::Request,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{principal_from_req, AuthPrincipal};
use crate::error::{WebUiError, WebUiResult};
use crate::state::WebUiApp;

#[derive(Debug, Serialize)]
pub struct DeviceResponse {
    pub token: String,
    pub player_uuid: String,
    pub player_name: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub enabled: bool,
    pub current_strength: i32,
}

#[derive(Debug, Serialize)]
pub struct DeviceListResponse {
    pub devices: Vec<DeviceResponse>,
    pub total_count: i32,
}

#[derive(Debug, Deserialize)]
pub struct DevicesQuery {
    pub player_uuid: Uuid,
}

/// `GET /devices?player_uuid=...` — list DG_LAB tokens for a
/// player. Viewers are constrained to their own UUID.
pub async fn list_devices(
    State(app): State<Arc<WebUiApp>>,
    Query(q): Query<DevicesQuery>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<DeviceListResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;
    match principal {
        AuthPrincipal::Admin => {}
        AuthPrincipal::Viewer { subject } if subject == q.player_uuid => {}
        AuthPrincipal::Viewer { .. } => {
            return Err(WebUiError::forbidden(
                "viewer cannot list another player's devices",
            ));
        }
    }
    let tokens = app.services.dglab.list_tokens(q.player_uuid).await?;
    let strength = app
        .services
        .dglab
        .get_strength(q.player_uuid)
        .await
        .ok();
    let current_strength = strength.map(|s| s.current_strength_a).unwrap_or(0);
    let devices: Vec<DeviceResponse> = tokens
        .into_iter()
        .map(|t| DeviceResponse {
            token: t.token,
            player_uuid: t.owner_uuid.to_string(),
            player_name: String::new(),
            created_at: t.created_at.to_rfc3339(),
            last_used_at: Some(t.last_used_at.to_rfc3339()),
            enabled: t.enabled,
            current_strength,
        })
        .collect();
    Ok(Json(DeviceListResponse {
        total_count: devices.len() as i32,
        devices,
    }))
}

#[derive(Debug, Deserialize)]
pub struct GenerateTokenRequest {
    pub player_uuid: Uuid,
}

/// `POST /devices` — generate a fresh DG_LAB token for a
/// player. Viewers may only generate for themselves; admins
/// for anyone.
pub async fn post_generate_token(
    State(app): State<Arc<WebUiApp>>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<DeviceResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;
    let body = axum::Json::<GenerateTokenRequest>::from_request(req, &())
        .await
        .map_err(|e| WebUiError::bad_request(format!("invalid JSON: {e}")))?
        .0;
    match principal {
        AuthPrincipal::Admin => {}
        AuthPrincipal::Viewer { subject } if subject == body.player_uuid => {}
        AuthPrincipal::Viewer { .. } => {
            return Err(WebUiError::forbidden(
                "viewer cannot generate a token for another player",
            ));
        }
    }
    let token_id = Uuid::new_v4();
    let now = Utc::now();
    let now_tick = now.timestamp_millis();
    let token = biocapital_dglab::domain::DglabToken {
        token: token_id.to_string(),
        owner_uuid: body.player_uuid,
        created_tick: now_tick,
        last_used_tick: 0,
        enabled: true,
        created_at: now,
        last_used_at: now,
    };
    app.services.dglab.upsert_token(&token).await?;
    Ok(Json(DeviceResponse {
        token: token.token.clone(),
        player_uuid: token.owner_uuid.to_string(),
        player_name: String::new(),
        created_at: now.to_rfc3339(),
        last_used_at: None,
        enabled: true,
        current_strength: 0,
    }))
}

/// `POST /devices/:token/revoke` — flip a token's enabled flag
/// to false. Idempotent.
pub async fn post_revoke_token(
    State(app): State<Arc<WebUiApp>>,
    Path(token): Path<String>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<DeviceResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;
    let existing = app.services.dglab.get_token(&token).await?;
    match principal {
        AuthPrincipal::Admin => {}
        AuthPrincipal::Viewer { subject } if subject == existing.owner_uuid => {}
        AuthPrincipal::Viewer { .. } => {
            return Err(WebUiError::forbidden(
                "viewer cannot revoke another player's device",
            ));
        }
    }
    let now_tick = Utc::now().timestamp_millis();
    let revoked = app
        .services
        .dglab
        .revoke_token(&token, now_tick)
        .await?;
    Ok(Json(DeviceResponse {
        token: revoked.token.clone(),
        player_uuid: revoked.owner_uuid.to_string(),
        player_name: String::new(),
        created_at: revoked.created_at.to_rfc3339(),
        last_used_at: Some(revoked.last_used_at.to_rfc3339()),
        enabled: revoked.enabled,
        current_strength: 0,
    }))
}
