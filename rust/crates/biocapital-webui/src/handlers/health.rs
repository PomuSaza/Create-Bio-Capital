//! `GET /health` — liveness + readiness probe.
//!
//! Returns `{ "status": "ok" }` when the server is up and the
//! PG pool is reachable. The endpoint is **unauthenticated**
//! so an external load-balancer can probe it without a token.

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::Serialize;
use sqlx::Row;

use crate::state::WebUiApp;

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub pg: String,
    pub tick_millis: i64,
}

pub async fn get_health(State(app): State<Arc<WebUiApp>>) -> Json<HealthResponse> {
    let pg_ok = sqlx::query("SELECT 1 AS ok")
        .fetch_one(&app.pg)
        .await
        .map(|row| row.try_get::<i32, _>("ok").unwrap_or(0) == 1)
        .unwrap_or(false);
    Json(HealthResponse {
        status: if pg_ok { "ok".to_string() } else { "degraded".to_string() },
        pg: if pg_ok { "up".to_string() } else { "down".to_string() },
        tick_millis: chrono::Utc::now().timestamp_millis(),
    })
}
