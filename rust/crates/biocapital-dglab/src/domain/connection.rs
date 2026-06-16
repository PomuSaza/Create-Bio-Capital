//! DG_LAB WebSocket connection registry (10 §3 / §4.1).
//!
//! `DglabState` is the in-process map of currently-open WebSocket
//! connections, keyed by token. The `on_new_connection` helper
//! implements the **single-connection-per-token** invariant called
//! out in 10 §4.1: a duplicate connect for the same token kicks
//! the older socket off the map (the WebSocket layer is then
//! responsible for sending the `close_frame` and dropping the
//! underlying task).

use std::collections::HashMap;
use std::net::SocketAddr;

use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DglabConnection {
    pub connection_id: Uuid,
    pub token: String,
    pub ws_addr: SocketAddr,
    pub connected_at: DateTime<Utc>,
    pub last_ping_at: DateTime<Utc>,
    pub protocol_version: String,
}

#[derive(Debug, Error)]
pub enum DglabError {
    /// New connection attempt for a token that already has an open
    /// socket.  The caller is expected to evict the old connection
    /// and replace it with the new one.
    #[error("token {0} already has an open connection")]
    TokenAlreadyActive(String),
}

/// Per-process registry of active DG_LAB WebSocket connections.
/// The map is keyed by token (10 §4.1 invariant: at most one
/// connection per token at a time).
#[derive(Debug, Default, Clone)]
pub struct DglabState {
    pub connections_by_token: HashMap<String, DglabConnection>,
}

impl DglabState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of active connections — used by tests + the Web UI
    /// "Devices" page (10 §7).
    pub fn len(&self) -> usize {
        self.connections_by_token.len()
    }

    pub fn is_empty(&self) -> bool {
        self.connections_by_token.is_empty()
    }

    /// Look up a connection by token. Returns `None` if no socket
    /// for the given token is currently registered.
    pub fn get(&self, token: &str) -> Option<&DglabConnection> {
        self.connections_by_token.get(token)
    }

    /// Drop a connection (e.g. on graceful close). Returns the
    /// evicted entry, if any, so the caller can audit it.
    pub fn remove(&mut self, token: &str) -> Option<DglabConnection> {
        self.connections_by_token.remove(token)
    }
}

/// Register a new connection, evicting any prior connection for
/// the same token. The caller (the WebSocket layer) is
/// responsible for sending the `close_frame` to the evicted
/// socket — this function only updates the registry.
///
/// Returns the evicted connection, if any, so the caller can
/// wire the audit + close-frame side effects.
pub fn on_new_connection(
    state: &mut DglabState,
    new_conn: DglabConnection,
) -> Result<Option<DglabConnection>, DglabError> {
    let evicted = state
        .connections_by_token
        .remove(&new_conn.token);
    state
        .connections_by_token
        .insert(new_conn.token.clone(), new_conn);
    Ok(evicted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(token: &str) -> DglabConnection {
        DglabConnection {
            connection_id: Uuid::new_v4(),
            token: token.to_owned(),
            ws_addr: "127.0.0.1:9999".parse().unwrap(),
            connected_at: Utc::now(),
            last_ping_at: Utc::now(),
            protocol_version: "1.0".to_owned(),
        }
    }

    #[test]
    fn first_connection_registers() {
        let mut s = DglabState::new();
        let evicted = on_new_connection(&mut s, conn("tok-1")).unwrap();
        assert!(evicted.is_none());
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn duplicate_token_evicts_old() {
        let mut s = DglabState::new();
        let old = conn("tok-1");
        let new = DglabConnection {
            connection_id: Uuid::new_v4(),
            ..conn("tok-1")
        };
        on_new_connection(&mut s, old.clone()).unwrap();
        let evicted = on_new_connection(&mut s, new).unwrap().unwrap();
        assert_eq!(evicted.connection_id, old.connection_id);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn distinct_tokens_coexist() {
        let mut s = DglabState::new();
        on_new_connection(&mut s, conn("tok-1")).unwrap();
        on_new_connection(&mut s, conn("tok-2")).unwrap();
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn remove_clears_entry() {
        let mut s = DglabState::new();
        on_new_connection(&mut s, conn("tok-1")).unwrap();
        let removed = s.remove("tok-1").unwrap();
        assert_eq!(removed.token, "tok-1");
        assert!(s.is_empty());
    }
}
