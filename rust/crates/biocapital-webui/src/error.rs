//! Error type and JSON envelope for the Web UI HTTP API.
//!
//! The error envelope shape is fixed by `doc/15-web-ui.md §5.1`:
//!
//! ```json
//! { "error": "code", "message": "human readable", "request_id": "uuid" }
//! ```
//!
//! HTTP status mapping (task spec):
//! - 200 OK
//! - 400 Bad Request (parameter validation)
//! - 401 Unauthorized (missing / invalid token)
//! - 403 Forbidden (token present but lacks permission)
//! - 404 Not Found
//! - 409 Conflict (e.g. insufficient balance)
//! - 500 Internal Server Error

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

pub type WebUiResult<T> = std::result::Result<T, WebUiError>;

/// Stable error codes emitted in the JSON envelope's `error`
/// field. Mirrors the contract in `doc/15-web-ui.md §5.1` (snake
/// case). Adding a new variant is a breaking change for the Web
/// UI frontend — bump `WEBUI_ERROR_SCHEMA_VERSION` if you do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::NotFound => "not_found",
            ErrorCode::Conflict => "conflict",
            ErrorCode::Internal => "internal_error",
        }
    }
}

/// Bumped whenever [`ErrorCode`] gains a variant. The frontend
/// should refuse to talk to a server whose version is older than
/// what it understands.
pub const WEBUI_ERROR_SCHEMA_VERSION: u32 = 1;

/// The single error type every handler returns. Implements
/// [`IntoResponse`] so handlers can `?`-bubble without manual
/// JSON assembly.
#[derive(Debug, Error)]
pub enum WebUiError {
    #[error("bad request: {message}")]
    BadRequest { message: String, request_id: Uuid },

    #[error("unauthorized: {message}")]
    Unauthorized { message: String, request_id: Uuid },

    #[error("forbidden: {message}")]
    Forbidden { message: String, request_id: Uuid },

    #[error("not found: {message}")]
    NotFound { message: String, request_id: Uuid },

    #[error("conflict: {message}")]
    Conflict { message: String, request_id: Uuid },

    #[error("internal error: {message}")]
    Internal { message: String, request_id: Uuid },
}

impl WebUiError {
    /// Build a `BadRequest` error with a fresh request_id.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest {
            message: message.into(),
            request_id: Uuid::new_v4(),
        }
    }

    /// Build an `Unauthorized` error with a fresh request_id.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::Unauthorized {
            message: message.into(),
            request_id: Uuid::new_v4(),
        }
    }

    /// Build a `Forbidden` error with a fresh request_id.
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden {
            message: message.into(),
            request_id: Uuid::new_v4(),
        }
    }

    /// Build a `NotFound` error with a fresh request_id.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound {
            message: message.into(),
            request_id: Uuid::new_v4(),
        }
    }

    /// Build a `Conflict` error with a fresh request_id.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
            message: message.into(),
            request_id: Uuid::new_v4(),
        }
    }

    /// Build an `Internal` error with a fresh request_id.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
            request_id: Uuid::new_v4(),
        }
    }

    /// Map error → HTTP status code.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest { .. } => StatusCode::BAD_REQUEST,
            Self::Unauthorized { .. } => StatusCode::UNAUTHORIZED,
            Self::Forbidden { .. } => StatusCode::FORBIDDEN,
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Map error → stable `error` code string.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::BadRequest { .. } => ErrorCode::BadRequest,
            Self::Unauthorized { .. } => ErrorCode::Unauthorized,
            Self::Forbidden { .. } => ErrorCode::Forbidden,
            Self::NotFound { .. } => ErrorCode::NotFound,
            Self::Conflict { .. } => ErrorCode::Conflict,
            Self::Internal { .. } => ErrorCode::Internal,
        }
    }

    /// Map error → human-readable message (the `message` field).
    pub fn message(&self) -> &str {
        match self {
            Self::BadRequest { message, .. }
            | Self::Unauthorized { message, .. }
            | Self::Forbidden { message, .. }
            | Self::NotFound { message, .. }
            | Self::Conflict { message, .. }
            | Self::Internal { message, .. } => message,
        }
    }

    /// The request_id attached to this error.
    pub fn request_id(&self) -> Uuid {
        match self {
            Self::BadRequest { request_id, .. }
            | Self::Unauthorized { request_id, .. }
            | Self::Forbidden { request_id, .. }
            | Self::NotFound { request_id, .. }
            | Self::Conflict { request_id, .. }
            | Self::Internal { request_id, .. } => *request_id,
        }
    }
}

impl IntoResponse for WebUiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(json!({
            "error": self.code().as_str(),
            "message": self.message(),
            "request_id": self.request_id().to_string(),
        }));
        (status, body).into_response()
    }
}

/// Wire format for the envelope. Exposed so the SSE handler
/// can reuse the same shape for transport-level errors.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub error: String,
    pub message: String,
    pub request_id: String,
}

impl From<&WebUiError> for ErrorEnvelope {
    fn from(e: &WebUiError) -> Self {
        Self {
            error: e.code().as_str().to_string(),
            message: e.message().to_string(),
            request_id: e.request_id().to_string(),
        }
    }
}

// ── Conversions from common error types ─────────────────────────────────────

impl From<sqlx::Error> for WebUiError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => WebUiError::not_found("resource not found"),
            other => WebUiError::internal(format!("postgres error: {other}")),
        }
    }
}

impl From<serde_json::Error> for WebUiError {
    fn from(e: serde_json::Error) -> Self {
        WebUiError::bad_request(format!("JSON parse error: {e}"))
    }
}

impl From<uuid::Error> for WebUiError {
    fn from(e: uuid::Error) -> Self {
        WebUiError::bad_request(format!("invalid UUID: {e}"))
    }
}

impl From<anyhow::Error> for WebUiError {
    fn from(e: anyhow::Error) -> Self {
        WebUiError::internal(format!("{e:#}"))
    }
}

// ── Per-domain `RepoError` conversions ──────────────────────────────────────
//
// Each `biocapital-pg::<module>` exposes a typed `RepoError` enum
// (and a matching `*RepoError` type alias). The Web UI handlers
// `?`-bubble these via the `From` impls below — the mapping is
// deliberately coarse: any PG-side error is reported as a 500
// with the underlying error string in the `message` field
// (mirrors how the gRPC layer's `repo_status` helpers flatten
// everything to `Status::internal`). 404/409 mapping is
// intentionally **not** done here because the per-domain
// `RepoError` enums vary in how they encode "not found" — the
// handlers do that mapping themselves when they need a 404
// response (e.g. `BankAccountNotFound`).

impl From<biocapital_pg::bank::RepoError> for WebUiError {
    fn from(e: biocapital_pg::bank::RepoError) -> Self {
        WebUiError::internal(format!("bank repository error: {e}"))
    }
}

impl From<biocapital_pg::contract::RepoError> for WebUiError {
    fn from(e: biocapital_pg::contract::RepoError) -> Self {
        WebUiError::internal(format!("contract repository error: {e}"))
    }
}

impl From<biocapital_pg::dglab::RepoError> for WebUiError {
    fn from(e: biocapital_pg::dglab::RepoError) -> Self {
        WebUiError::internal(format!("dglab repository error: {e}"))
    }
}

impl From<biocapital_pg::hardware_token::RepoError> for WebUiError {
    fn from(e: biocapital_pg::hardware_token::RepoError) -> Self {
        WebUiError::internal(format!("hardware-token repository error: {e}"))
    }
}

impl From<biocapital_pg::core_pod::PodRepoError> for WebUiError {
    fn from(e: biocapital_pg::core_pod::PodRepoError) -> Self {
        WebUiError::internal(format!("core-pod repository error: {e}"))
    }
}

impl From<biocapital_pg::PlayerStateRepoError> for WebUiError {
    fn from(e: biocapital_pg::PlayerStateRepoError) -> Self {
        WebUiError::internal(format!("player-state repository error: {e}"))
    }
}
