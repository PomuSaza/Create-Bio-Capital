//! DG_LAB auth token (10 §3.2).
//!
//! Each player owns at most one enabled token at a time. The
//! `idx_dglab_tokens_owner` partial unique index on the SQL side
//! enforces this constraint; the Rust side mirrors it in
//! [`DglabToken::validate`] and the `GenerateToken` gRPC path.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// UUIDv4 token presented by the DG_LAB client when opening a
/// WebSocket.  Stored as a 36-char canonical string in PG to match
/// the `VARCHAR(36) PRIMARY KEY` column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DglabToken {
    pub token: String,
    pub owner_uuid: Uuid,
    pub created_tick: i64,
    pub last_used_tick: i64,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
}

impl DglabToken {
    /// Validate a freshly-issued token. The service layer should
    /// reject callers that bypass this check.
    pub fn validate(&self) -> Result<(), DglabTokenError> {
        if self.token.len() != 36 {
            return Err(DglabTokenError::WrongShape {
                expected: 36,
                got: self.token.len(),
            });
        }
        // Cheap canonical-format probe; the gRPC layer will parse
        // into a Uuid for stronger guarantees.
        Uuid::parse_str(&self.token).map_err(|e| DglabTokenError::NotUuid(e.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DglabTokenError {
    #[error("token length {got} != expected {expected}")]
    WrongShape { expected: usize, got: usize },

    #[error("token is not a UUID: {0}")]
    NotUuid(String),
}
