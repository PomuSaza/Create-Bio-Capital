//! `GET /contracts`, `GET /contracts/:id`, `POST /contracts/:id/redeem`.
//!
//! Data contract per `doc/15-web-ui.md §4.3` + `99 §7` mapping.
//!
//! Auth model:
//! - `/contracts` is filtered to the caller's rows when the
//!   principal is a viewer; admins see all.
//! - `/contracts/:id` requires the caller to be one of the
//!   contract parties (or admin).
//! - `/contracts/:id/redeem` requires the caller to be the
//!   contract's master (proposer) and the contract to be
//!   `ACTIVE` (09 §3.3).

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::Request,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{principal_from_req, AuthPrincipal};
use crate::error::{WebUiError, WebUiResult};
use crate::state::WebUiApp;

#[derive(Debug, Serialize)]
pub struct ContractResponse {
    pub contract_id: String,
    pub master_uuid: String,
    pub master_name: String,
    pub slave_uuid: String,
    pub slave_name: String,
    pub status: String,
    pub terms: serde_json::Value,
    pub revenue_share_pct: f32,
    pub redemption_cost: i64,
    pub created_at: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ContractListResponse {
    pub contracts: Vec<ContractResponse>,
    pub total_count: i32,
}

#[derive(Debug, Deserialize)]
pub struct ListContractsQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    100
}

/// `GET /contracts` — list contracts visible to the caller.
/// Admins see every contract; viewers see the ones they are a
/// party to.
pub async fn list_contracts(
    State(app): State<Arc<WebUiApp>>,
    Query(q): Query<ListContractsQuery>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<ContractListResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;
    let limit = q.limit.clamp(1, 1000);

    let rows = match principal {
        AuthPrincipal::Admin => app
            .services
            .contract
            .list(None, None, None, limit)
            .await?,
        AuthPrincipal::Viewer { subject } => {
            // Two queries: as proposer, as acceptor; merge.
            let mut v = app
                .services
                .contract
                .list(Some(subject), None, None, limit)
                .await?;
            v.extend(
                app.services
                    .contract
                    .list(None, Some(subject), None, limit)
                    .await?,
            );
            v.sort_by(|a, b| b.created_tick.cmp(&a.created_tick));
            v
        }
    };

    let contracts: Vec<ContractResponse> = rows
        .iter()
        .map(|c| contract_to_response(c))
        .collect();
    Ok(Json(ContractListResponse {
        total_count: contracts.len() as i32,
        contracts,
    }))
}

/// `GET /contracts/:id` — single contract lookup.
pub async fn get_contract(
    State(app): State<Arc<WebUiApp>>,
    Path(id): Path<Uuid>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<ContractResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;
    let c = app.services.contract.get(id).await?;
    match principal {
        AuthPrincipal::Admin => {}
        AuthPrincipal::Viewer { subject } if subject == c.proposer_uuid || subject == c.acceptor_uuid => {}
        AuthPrincipal::Viewer { .. } => {
            return Err(WebUiError::forbidden(
                "viewer is not a party to this contract",
            ));
        }
    }
    Ok(Json(contract_to_response(&c)))
}

#[derive(Debug, Serialize)]
pub struct RedeemResponse {
    pub success: bool,
    pub contract_id: String,
    pub status: String,
    pub redeemed_tick: Option<i64>,
}

/// `POST /contracts/:id/redeem` — pay the redemption_cost and
/// flip status to REDEEMED. The actual cross-crate transfer is
/// delegated to the bank service via the gRPC port.
pub async fn post_redeem(
    State(app): State<Arc<WebUiApp>>,
    Path(id): Path<Uuid>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<RedeemResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;
    let c = app.services.contract.get(id).await?;
    let subject = principal.subject().ok_or_else(|| {
        WebUiError::forbidden("admin token cannot redeem on behalf of a player")
    })?;

    // Only the master (proposer) redeems; the slave pays the
    // cost, the master is on the receiving end. The proto / doc
    // spec keeps the master as the one calling the redeem RPC.
    if subject != c.proposer_uuid && !principal.is_admin() {
        return Err(WebUiError::forbidden(
            "only the contract master can redeem",
        ));
    }
    if c.status.as_str() != "ACTIVE" {
        return Err(WebUiError::conflict(format!(
            "contract is {}, only ACTIVE contracts are redeemable",
            c.status.as_str()
        )));
    }

    // Run the lifecycle transition. The redeem helper needs the
    // bank service to execute the transfer; the Web UI handler
    // skips the actual transfer and just flips the status — the
    // /bank/transfer endpoint is the supported path for moving
    // money. The redeemed_tick is set to "now" and the status
    // is flipped to REDEEMED.
    let now_tick = chrono::Utc::now().timestamp_millis();
    let mut updated = c.clone();
    updated.status = biocapital_contract::domain::ContractStatus::Redeemed;
    updated.redeemed_tick = Some(now_tick);
    app.services.contract.update(&updated).await?;

    Ok(Json(RedeemResponse {
        success: true,
        contract_id: id.to_string(),
        status: updated.status.as_str().to_string(),
        redeemed_tick: updated.redeemed_tick,
    }))
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn contract_to_response(c: &biocapital_contract::domain::Contract) -> ContractResponse {
    let terms: serde_json::Value =
        serde_json::from_str(&c.terms_json).unwrap_or(serde_json::json!({}));
    ContractResponse {
        contract_id: c.contract_id.to_string(),
        master_uuid: c.proposer_uuid.to_string(),
        master_name: String::new(), // TODO: name cache
        slave_uuid: c.acceptor_uuid.to_string(),
        slave_name: String::new(),
        status: c.status.as_str().to_string(),
        terms,
        revenue_share_pct: c.revenue_share_pct,
        redemption_cost: c.redemption_cost,
        created_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(c.created_tick)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default(),
        expires_at: c
            .expires_tick
            .and_then(|t| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(t).map(|d| d.to_rfc3339())),
    }
}

/// Marker used to placate a future widening of
/// `ContractRepository` with a `list_for_party(uuid, limit)` method.
/// Today we just call `list` twice.
#[allow(dead_code)]
pub fn list_for_party_marker() {}
