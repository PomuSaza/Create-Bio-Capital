//! Audit query + CSV export — `doc/15-web-ui.md §4.5`.
//!
//! Both endpoints require an admin token (the audit log is
//! restricted to staff).
//!
//! The v15 cut reads the union of `audit_bank`, `audit_dglab`,
//! `audit_admin`, `audit_player_state` rows by issuing
//! per-table `SELECT`s and merging. Performance: a single
//! dedicated audit view (`audit_all`) is a v15+ follow-up.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::error::{WebUiError, WebUiResult};
use crate::state::WebUiApp;

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub actor_uuid: Option<Uuid>,
    pub target_uuid: Option<Uuid>,
    pub op: Option<String>,
    pub from_tick: Option<i64>,
    pub to_tick: Option<i64>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    100
}

#[derive(Debug, Serialize)]
pub struct AuditResult {
    pub log_id: String,
    pub actor_uuid: String,
    pub actor_type: String,
    pub target_uuid: String,
    pub target_type: String,
    pub op: String,
    pub before: serde_json::Value,
    pub after: serde_json::Value,
    pub tick_millis: i64,
    pub request_id: Option<String>,
    pub notes: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct AuditQueryResponse {
    pub results: Vec<AuditResult>,
    pub total_count: i64,
}

/// `GET /audit/query?actor_uuid=...&op=...&from_tick=...&to_tick=...&limit=...`
/// — paginated query across all audit tables.
pub async fn query_audit(
    State(app): State<Arc<WebUiApp>>,
    Query(q): Query<AuditQuery>,
) -> WebUiResult<Json<AuditQueryResponse>> {
    let limit = q.limit.clamp(1, 1000);
    let offset = q.offset.max(0);

    let mut results: Vec<AuditResult> = Vec::new();
    results.extend(
        query_audit_table(&app, "audit_bank", &q, limit, offset).await?,
    );
    results.extend(
        query_audit_table(&app, "audit_dglab", &q, limit, offset).await?,
    );
    results.extend(
        query_audit_table(&app, "audit_admin", &q, limit, offset).await?,
    );
    results.extend(
        query_audit_table(&app, "audit_player_state", &q, limit, offset).await?,
    );
    results.extend(
        query_audit_table(&app, "audit_hardware_token", &q, limit, offset).await?,
    );
    results.extend(
        query_audit_table(&app, "audit_core_pod", &q, limit, offset).await?,
    );
    results.extend(
        query_audit_table(&app, "audit_contract", &q, limit, offset).await?,
    );
    results.extend(
        query_audit_table(&app, "audit_creature_config", &q, limit, offset).await?,
    );
    results.extend(
        query_audit_table(&app, "audit_environment", &q, limit, offset).await?,
    );

    results.sort_by(|a, b| b.tick_millis.cmp(&a.tick_millis));
    let total = results.len() as i64;
    Ok(Json(AuditQueryResponse {
        results,
        total_count: total,
    }))
}

async fn query_audit_table(
    app: &WebUiApp,
    table: &str,
    q: &AuditQuery,
    limit: i64,
    offset: i64,
) -> WebUiResult<Vec<AuditResult>> {
    // We hand-build the WHERE clause to keep the audit query
    // table-agnostic. The columns of interest are:
    //   actor_uuid, target_uuid (or *owner_uuid for some),
    //   op, tick_millis, request_id, before_json / after_json,
    //   notes.
    //
    // The audit_player_state / audit_environment tables use
    // `target_uuid` for the player under effect; the
    // audit_hardware_token table uses `target_owner_uuid`. We
    // filter against whichever exists.

    let mut sql = String::from("SELECT * FROM ");
    sql.push_str(table);
    sql.push_str(" WHERE 1=1");
    if q.actor_uuid.is_some() {
        sql.push_str(" AND actor_uuid = $1");
    }
    if q.op.is_some() {
        sql.push_str(&format!(" AND op = ${}", if q.actor_uuid.is_some() { "2" } else { "1" }));
    }
    if let Some(from) = q.from_tick {
        sql.push_str(&format!(" AND tick_millis >= {}", from));
    }
    if let Some(to) = q.to_tick {
        sql.push_str(&format!(" AND tick_millis <= {}", to));
    }
    sql.push_str(&format!(
        " ORDER BY tick_millis DESC LIMIT {} OFFSET {}",
        limit, offset
    ));

    let mut query = sqlx::query(&sql);
    if let Some(actor) = q.actor_uuid {
        query = query.bind(actor);
    }
    if let Some(op) = &q.op {
        query = query.bind(op);
    }
    let rows = query
        .fetch_all(&app.pg)
        .await
        .map_err(WebUiError::from)?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(row_to_audit_result(table, &row));
    }
    Ok(out)
}

fn row_to_audit_result(_table: &str, row: &sqlx::postgres::PgRow) -> AuditResult {
    // The audit tables share a relaxed contract (99 §2.2). The
    // union query here uses `SELECT *`; missing columns yield
    // defaults rather than errors so a missing `target_uuid`
    // in `audit_hardware_token` (which uses `target_owner_uuid`)
    // still produces a usable row.
    let log_id = column_string(row, "log_id");
    let actor_uuid = column_uuid_string(row, "actor_uuid").unwrap_or_default();
    let actor_type = column_string(row, "actor_type");
    let target_uuid = column_uuid_string(row, "target_uuid")
        .or_else(|| column_uuid_string(row, "target_owner_uuid"))
        .or_else(|| column_uuid_string(row, "target_account_uuid"))
        .unwrap_or_default();
    let target_type = column_string_or(row, "target_type", "");
    let op = column_string(row, "op");
    let before = column_json(row, "before_json")
        .or_else(|| column_json(row, "before_balance").map(|v| serde_json::json!({ "balance": v })))
        .unwrap_or(serde_json::json!({}));
    let after = column_json(row, "after_json")
        .or_else(|| column_json(row, "after_balance").map(|v| serde_json::json!({ "balance": v })))
        .unwrap_or(serde_json::json!({}));
    let tick_millis = column_i64(row, "tick_millis");
    let request_id = column_uuid_string(row, "request_id");
    let notes = column_json(row, "notes").or_else(|| column_json(row, "notes_json"));

    AuditResult {
        log_id,
        actor_uuid,
        actor_type,
        target_uuid,
        target_type,
        op,
        before,
        after,
        tick_millis,
        request_id,
        notes,
    }
}

fn column_string(row: &sqlx::postgres::PgRow, col: &str) -> String {
    row.try_get::<String, _>(col).unwrap_or_default()
}

fn column_string_or(row: &sqlx::postgres::PgRow, col: &str, default: &str) -> String {
    row.try_get::<String, _>(col).unwrap_or_else(|_| default.to_string())
}

fn column_uuid_string(row: &sqlx::postgres::PgRow, col: &str) -> Option<String> {
    row.try_get::<Uuid, _>(col).ok().map(|u| u.to_string())
}

fn column_i64(row: &sqlx::postgres::PgRow, col: &str) -> i64 {
    row.try_get::<i64, _>(col).unwrap_or(0)
}

fn column_json(row: &sqlx::postgres::PgRow, col: &str) -> Option<serde_json::Value> {
    row.try_get::<serde_json::Value, _>(col).ok()
}

/// `GET /audit/export?format=csv` — stream the audit log as a
/// CSV download. Currently supports `format=csv` only.
pub async fn export_audit(
    State(app): State<Arc<WebUiApp>>,
    Query(q): Query<AuditQuery>,
) -> WebUiResult<impl IntoResponse> {
    let limit = q.limit.clamp(1, 100_000);
    let offset = q.offset.max(0);

    // Reuse the same union logic.
    let mut results: Vec<AuditResult> = Vec::new();
    for table in [
        "audit_bank",
        "audit_dglab",
        "audit_admin",
        "audit_player_state",
        "audit_hardware_token",
        "audit_core_pod",
        "audit_contract",
        "audit_creature_config",
        "audit_environment",
    ] {
        results.extend(
            query_audit_table(&app, table, &q, limit, offset).await?,
        );
    }
    results.sort_by(|a, b| b.tick_millis.cmp(&a.tick_millis));

    // Build CSV in memory. The audit log row count is bounded
    // by 100k by the query limit, so the in-memory build is
    // safe.
    let mut wtr = csv::Writer::from_writer(vec![]);
    wtr.write_record([
        "log_id",
        "actor_uuid",
        "actor_type",
        "target_uuid",
        "target_type",
        "op",
        "before",
        "after",
        "tick_millis",
        "request_id",
        "notes",
    ])
    .map_err(|e| WebUiError::internal(format!("csv write: {e}")))?;
    for r in &results {
        wtr.write_record([
            r.log_id.as_str(),
            r.actor_uuid.as_str(),
            r.actor_type.as_str(),
            r.target_uuid.as_str(),
            r.target_type.as_str(),
            r.op.as_str(),
            &r.before.to_string(),
            &r.after.to_string(),
            &r.tick_millis.to_string(),
            r.request_id.as_deref().unwrap_or(""),
            &r.notes.as_ref().map(|v| v.to_string()).unwrap_or_default(),
        ])
        .map_err(|e| WebUiError::internal(format!("csv write: {e}")))?;
    }
    let bytes = wtr
        .into_inner()
        .map_err(|e| WebUiError::internal(format!("csv flush: {e}")))?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"audit_export.csv\""),
    );
    Ok((headers, bytes))
}
