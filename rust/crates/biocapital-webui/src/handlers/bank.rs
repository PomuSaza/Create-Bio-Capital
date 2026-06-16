//! `POST /bank/transfer` and `GET /bank/history` — bank
//! operations exposed to the Web UI (doc/15 §4.2 + §5.3).
//!
//! Both endpoints require an authenticated principal.
//! `transfer` enforces that the caller's subject owns the source
//! account (admins can transfer on behalf of any player; viewers
//! can only move their own money).

use std::sync::Arc;

use axum::{
    extract::{FromRequest, Query, State},
    http::Request,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{principal_from_req, AuthPrincipal};
use crate::error::{WebUiError, WebUiResult};
use crate::state::{BankTransactionEvent, WebUiApp};

#[derive(Debug, Deserialize)]
pub struct TransferRequestBody {
    pub from_account_uuid: Uuid,
    pub to_player_name: String,
    pub amount: i64,
    pub request_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct TransferResponse {
    pub success: bool,
    pub actual_amount: i64,
    pub balance_after: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// `POST /bank/transfer` — execute a cat-grass transfer. The
/// destination is resolved by player name (not UUID) to match
/// the in-game `/biocapital bank transfer <name> <amount>`
/// command.
pub async fn post_transfer(
    State(app): State<Arc<WebUiApp>>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<TransferResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;

    // Extract the JSON body from the request. We use the
    // `FromRequest` impl for `axum::Json` directly so we can
    // re-use the existing principal extraction above.
    let body = axum::Json::<TransferRequestBody>::from_request(req, &())
        .await
        .map_err(|e| WebUiError::bad_request(format!("invalid JSON: {e}")))?
        .0;

    if body.amount <= 0 {
        return Err(WebUiError::bad_request("amount must be > 0"));
    }
    if body.to_player_name.is_empty() {
        return Err(WebUiError::bad_request("to_player_name is empty"));
    }

    // Resolve source account + check ownership.
    let from_account = app
        .services
        .bank
        .get_account(body.from_account_uuid)
        .await?;
    enforce_owner_or_admin(&principal, from_account.owner_uuid)?;

    // Resolve destination by username. The Web UI does not own
    // a player-name table; for the v15 cut we lean on the
    // account's existing owner UUID. A real implementation will
    // look up the destination player's `owner_uuid` via a
    // `player_names` cache or the `whitelist.usernames` list.
    let to_owner = resolve_player_name_to_uuid(&app, &body.to_player_name)
        .await
        .ok_or_else(|| {
            WebUiError::not_found(format!("destination player '{}' not found", body.to_player_name))
        })?;
    let to_account = app.services.bank.get_account_by_owner(to_owner).await?;

    // Perform the transfer. Idempotent on `request_id`.
    let now_tick = Utc::now().timestamp_millis();
    let counterparty_name = Some(body.to_player_name.clone());
    let (out_tx, _in_tx) = match app
        .services
        .bank
        .atomic_transfer(
            from_account.account_uuid,
            to_account.account_uuid,
            body.amount,
            counterparty_name,
            body.request_id,
            now_tick,
        )
        .await
    {
        Ok(txs) => txs,
        Err(e) => {
            // Map common PG errors to friendly codes.
            return Err(map_bank_error(e));
        }
    };

    // Publish to the SSE bus.
    app.events.publish(crate::state::WebUiEvent::BankTransaction(
        BankTransactionEvent {
            account_uuid: from_account.account_uuid,
            op: "TRANSFER_OUT".to_string(),
            amount: out_tx.amount,
            balance_after: out_tx.balance_after,
            tick_millis: out_tx.tick_millis,
        },
    ));

    Ok(Json(TransferResponse {
        success: true,
        actual_amount: out_tx.amount,
        balance_after: out_tx.balance_after,
        error_code: None,
        error_message: None,
    }))
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub account_uuid: Uuid,
    #[serde(default = "default_history_limit")]
    pub limit: i64,
}

fn default_history_limit() -> i64 {
    16
}

#[derive(Debug, Serialize)]
pub struct HistoryResponse {
    pub entries: Vec<HistoryEntry>,
    pub next_before_tick_millis: i64,
}

#[derive(Debug, Serialize)]
pub struct HistoryEntry {
    pub tx_id: String,
    pub op: String,
    pub amount: i64,
    pub balance_after: i64,
    pub counterparty_uuid: Option<String>,
    pub counterparty_name: Option<String>,
    pub tick_millis: i64,
    pub request_id: String,
}

/// `GET /bank/history?account_uuid=...&limit=16` — page through
/// the account's transaction history (newest first).
pub async fn get_history(
    State(app): State<Arc<WebUiApp>>,
    Query(q): Query<HistoryQuery>,
    req: Request<axum::body::Body>,
) -> WebUiResult<Json<HistoryResponse>> {
    let principal = principal_from_req(&req)
        .ok_or_else(|| WebUiError::unauthorized("missing principal"))?;
    let account = app.services.bank.get_account(q.account_uuid).await?;
    enforce_owner_or_admin(&principal, account.owner_uuid)?;

    let limit = q.limit.clamp(1, 1000);
    let txs = app.services.bank.get_history(q.account_uuid, limit).await?;

    let entries: Vec<HistoryEntry> = txs
        .into_iter()
        .map(|t| HistoryEntry {
            tx_id: t.tx_id.to_string(),
            op: t.op.as_str().to_string(),
            amount: t.amount,
            balance_after: t.balance_after,
            counterparty_uuid: t.counterparty_uuid.map(|u| u.to_string()),
            counterparty_name: t.counterparty_name,
            tick_millis: t.tick_millis,
            request_id: t.request_id.to_string(),
        })
        .collect();
    let next_before_tick_millis = entries
        .last()
        .map(|e| e.tick_millis)
        .unwrap_or(-1);

    Ok(Json(HistoryResponse {
        entries,
        next_before_tick_millis,
    }))
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn enforce_owner_or_admin(principal: &AuthPrincipal, owner: Uuid) -> WebUiResult<()> {
    match principal {
        AuthPrincipal::Admin => Ok(()),
        AuthPrincipal::Viewer { subject } if *subject == owner => Ok(()),
        AuthPrincipal::Viewer { .. } => Err(WebUiError::forbidden(
            "viewer token cannot act on another player's account",
        )),
    }
}

fn map_bank_error(e: biocapital_pg::BankRepoError) -> WebUiError {
    use biocapital_pg::BankRepoError;
    match e {
        BankRepoError::Sqlx(sqlx::Error::Database(db)) => {
            // Postgres CHECK constraints surface as `Database` errors
            // with SQLSTATE codes. A balance going negative yields
            // `23514`.
            let msg = db.message().to_string();
            if msg.contains("balance") || msg.contains("check constraint") {
                WebUiError::conflict(format!("balance constraint: {msg}"))
            } else {
                WebUiError::internal(format!("postgres error: {msg}"))
            }
        }
        BankRepoError::AccountNotFound { account_uuid } => {
            WebUiError::not_found(format!("account {account_uuid} not found"))
        }
        other => WebUiError::internal(format!("bank error: {other}")),
    }
}

/// Best-effort UUID resolution by player name.
///
/// The current `biocapital-bank` whitelist module owns the
/// in-memory whitelist cache (18 §2.3) but does **not** yet
/// expose a global singleton. For the v15 cut we accept that
/// name resolution returns `None` for everyone except the
/// caller — the `/admin/whitelist/reload` endpoint and a
/// follow-up `player_names` table are the explicit mechanisms
/// for fixing this. The transfer handler returns
/// `404 not_found` when the destination cannot be resolved.
async fn resolve_player_name_to_uuid(
    _app: &WebUiApp,
    _name: &str,
) -> Option<Uuid> {
    None
}

// Re-export axum's Json extractor for the post_transfer body parse.
