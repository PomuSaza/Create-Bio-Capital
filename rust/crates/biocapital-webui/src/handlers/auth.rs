//! Hardware-token endpoints (18 §7).
//!
//! Auth model: viewer tokens can issue / list / bind /
//! authenticate for themselves. Admin tokens can do all of the
//! above on behalf of any player.

use std::sync::Arc;

use axum::{
    extract::{FromRequest, Query, State},
    http::Request,
    Json,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{principal_from_req, AuthPrincipal};
use crate::error::{WebUiError, WebUiResult};
use crate::state::WebUiApp;

#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub token_id: String,
    pub owner_uuid: String,
    pub status: String,
    pub issued_at: String,
    pub expires_at: String,
    pub bound_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RequestTokenBody {
    pub player_uuid: Uuid,
}

/// `POST /auth/request-token` — issue a fresh hardware token
/// for a player. Viewers may only request for themselves.
pub async fn post_request_hardware_token(
    State(app): State<Arc<WebUiApp>>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<TokenResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;
    let body = axum::Json::<RequestTokenBody>::from_request(req, &())
        .await
        .map_err(|e| WebUiError::bad_request(format!("invalid JSON: {e}")))?
        .0;
    match principal {
        AuthPrincipal::Admin => {}
        AuthPrincipal::Viewer { subject } if subject == body.player_uuid => {}
        AuthPrincipal::Viewer { .. } => {
            return Err(WebUiError::forbidden(
                "viewer cannot request a token for another player",
            ));
        }
    }
    let token_id = Uuid::new_v4();
    let issued_at = Utc::now();
    let expires_at = issued_at + Duration::days(
        biocapital_bank::domain::HARDWARE_TOKEN_EXPIRY_DAYS as i64,
    );
    let token = app
        .services
        .hardware_token
        .create_token(body.player_uuid, token_id, issued_at, expires_at)
        .await
        .map_err(WebUiError::from)?;
    Ok(Json(TokenResponse {
        token_id: token.token_id.to_string(),
        owner_uuid: token.owner_uuid.to_string(),
        status: token.status.as_str().to_string(),
        issued_at: token.issued_at.to_rfc3339(),
        expires_at: token.expires_at.to_rfc3339(),
        bound_at: token.bound_at.map(|d| d.to_rfc3339()),
    }))
}

#[derive(Debug, Deserialize)]
pub struct BindHardwareBody {
    pub player_uuid: Uuid,
    pub token_id: Uuid,
    pub hardware_id_hash: String,
}

/// `POST /auth/bind-hardware` — bind a hardware-id-hash to a
/// previously-issued token. Viewers may only bind for themselves.
pub async fn post_bind_hardware(
    State(app): State<Arc<WebUiApp>>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<TokenResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;
    let body = axum::Json::<BindHardwareBody>::from_request(req, &())
        .await
        .map_err(|e| WebUiError::bad_request(format!("invalid JSON: {e}")))?
        .0;
    match principal {
        AuthPrincipal::Admin => {}
        AuthPrincipal::Viewer { subject } if subject == body.player_uuid => {}
        AuthPrincipal::Viewer { .. } => {
            return Err(WebUiError::forbidden(
                "viewer cannot bind hardware for another player",
            ));
        }
    }
    let token = app
        .services
        .hardware_token
        .bind_hardware(body.token_id, body.player_uuid, body.hardware_id_hash)
        .await
        .map_err(WebUiError::from)?;
    Ok(Json(TokenResponse {
        token_id: token.token_id.to_string(),
        owner_uuid: token.owner_uuid.to_string(),
        status: token.status.as_str().to_string(),
        issued_at: token.issued_at.to_rfc3339(),
        expires_at: token.expires_at.to_rfc3339(),
        bound_at: token.bound_at.map(|d| d.to_rfc3339()),
    }))
}

#[derive(Debug, Deserialize)]
pub struct ListHardwareQuery {
    pub player_uuid: Uuid,
    #[serde(default)]
    pub include_replaced: bool,
}

#[derive(Debug, Serialize)]
pub struct ListHardwareResponse {
    pub tokens: Vec<TokenResponse>,
}

/// `GET /auth/hardware?player_uuid=...` — list hardware tokens
/// for a player.
pub async fn list_hardware(
    State(app): State<Arc<WebUiApp>>,
    Query(q): Query<ListHardwareQuery>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<ListHardwareResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;
    match principal {
        AuthPrincipal::Admin => {}
        AuthPrincipal::Viewer { subject } if subject == q.player_uuid => {}
        AuthPrincipal::Viewer { .. } => {
            return Err(WebUiError::forbidden(
                "viewer cannot list another player's hardware",
            ));
        }
    }
    let rows = app
        .services
        .hardware_token
        .list_for_owner(q.player_uuid, q.include_replaced)
        .await
        .map_err(WebUiError::from)?;
    let tokens: Vec<TokenResponse> = rows
        .into_iter()
        .map(|t| TokenResponse {
            token_id: t.token_id.to_string(),
            owner_uuid: t.owner_uuid.to_string(),
            status: t.status.as_str().to_string(),
            issued_at: t.issued_at.to_rfc3339(),
            expires_at: t.expires_at.to_rfc3339(),
            bound_at: t.bound_at.map(|d| d.to_rfc3339()),
        })
        .collect();
    Ok(Json(ListHardwareResponse { tokens }))
}

#[derive(Debug, Deserialize)]
pub struct AuthenticateBody {
    pub player_uuid: Uuid,
    pub player_username: String,
    pub hardware_id_hash: String,
}

#[derive(Debug, Serialize)]
pub struct AuthenticateResponse {
    pub allowed: bool,
    pub reason: String,
}

/// `POST /auth/authenticate` — perform the 18 §3.1 login-time
/// authentication decision: whitelist pass → hardware-token
/// pass → deny. The actual side-effects (audit log row) live
/// in the gRPC service; this endpoint is a read-only mirror.
pub async fn post_authenticate(
    State(_app): State<Arc<WebUiApp>>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<AuthenticateResponse>> {
    let body = axum::Json::<AuthenticateBody>::from_request(req, &())
        .await
        .map_err(|e| WebUiError::bad_request(format!("invalid JSON: {e}")))?
        .0;
    // The whitelist check is best-effort here. A full
    // implementation would call into the gRPC `Authenticate` RPC
    // (which holds the authoritative logic and writes the audit
    // row). For the v15 cut we report `allowed = false` and
    // tell the caller to use the gRPC path; this keeps the
    // route reserved for future read-only display.
    let _ = body;
    Ok(Json(AuthenticateResponse {
        allowed: false,
        reason: "use_grpc_authenticate".to_string(),
    }))
}
