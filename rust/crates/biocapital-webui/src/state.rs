//! State container for the Web UI HTTP API.
//!
//! [`WebUiApp`] is what the axum router holds in its
//! [`axum::extract::State`]. It owns the PG pool, the
//! [`ServiceBundle`] of per-domain services, the
//! [`EventBus`] for SSE, and the [`WebUiConfig`].
//!
//! The struct is intentionally `Clone`-cheap: the PG pool and
//! the service Arcs are already shared, and the event bus uses
//! `tokio::sync::broadcast` (cheap to clone).

use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

use biocapital_creature::pg::CreatureConfigRepository;
use biocapital_pg::{
    BankRepository, ContractRepository, DglabRepository, HardwareTokenRepository,
    PlayerStateRepository,
};

// ── Token set ───────────────────────────────────────────────────────────────

/// In-memory view of the configured bearer tokens. Loaded from
/// `biocapital-server.toml [WebUI] admin_tokens` and from the
/// in-game grant-viewer command (which inserts into
/// `tokens.viewer_tokens`).
///
/// Admin tokens are stored as 64-char hex strings. Viewer tokens
/// are stored as 32 raw bytes — the last 16 bytes encode the
/// player UUID (subject).
#[derive(Debug, Default, Clone)]
pub struct TokenSet {
    /// 64-char hex admin tokens.
    pub admin_tokens: Vec<String>,
    /// 32-byte viewer tokens (body bytes; the `"v1."` prefix is
    /// added at issue time).
    pub viewer_tokens: Vec<Vec<u8>>,
}

impl TokenSet {
    /// Replace the admin token list with the given hex strings.
    /// Each entry is expected to be 64 hex chars; malformed
    /// entries are silently skipped with a `tracing::warn!`.
    pub fn set_admin_tokens(&mut self, hex_tokens: impl IntoIterator<Item = String>) {
        self.admin_tokens.clear();
        for raw in hex_tokens {
            if raw.len() == 64 && hex::decode(&raw).is_ok() {
                self.admin_tokens.push(raw);
            } else {
                tracing::warn!(
                    target: "webui::auth",
                    token_prefix = %&raw[..raw.len().min(8)],
                    "ignoring malformed admin token (need 64 hex chars)"
                );
            }
        }
    }

    /// Insert a freshly-issued viewer token. The bytes are the
    /// raw 32-byte body (last 16 = subject UUID).
    pub fn insert_viewer_token(&mut self, body: Vec<u8>) {
        if body.len() != 32 {
            tracing::warn!(
                target: "webui::auth",
                len = body.len(),
                "refusing to insert malformed viewer token"
            );
            return;
        }
        self.viewer_tokens.retain(|b| b.as_slice() != body.as_slice());
        self.viewer_tokens.push(body);
    }

    /// Remove a viewer token (logout / revoke).
    pub fn remove_viewer_token(&mut self, body: &[u8]) {
        self.viewer_tokens.retain(|b| b.as_slice() != body);
    }
}

// ── Config ──────────────────────────────────────────────────────────────────

/// Static configuration for the Web UI server. Loaded once at
/// startup from `biocapital-server.toml` and (for the token
/// list) updated on `/admin/whitelist/reload` (which also
/// re-reads the whitelist toml and rotates the in-memory token
/// set).
#[derive(Debug, Clone)]
pub struct WebUiConfig {
    /// HTTP port (default 8080 per `doc/14-rust-services.md §2.1`).
    pub http_port: u16,
    /// Whether the server is publicly exposed. Disables CORS
    /// lockdown if false (dev mode only).
    pub public: bool,
    /// Token set. Wrapped in `Arc<RwLock>` so admin endpoints
    /// can rotate it without rebuilding the router.
    pub tokens: Arc<RwLock<TokenSet>>,
}

impl Default for WebUiConfig {
    fn default() -> Self {
        Self {
            http_port: 8080,
            public: false,
            tokens: Arc::new(RwLock::new(TokenSet::default())),
        }
    }
}

// ── Event bus ───────────────────────────────────────────────────────────────

/// A single event pushed to SSE subscribers. Variants mirror the
/// events listed in `doc/99-integration-matrix.md §3.1` that the
/// Web UI cares about (bank transactions, DG_LAB strength,
/// player state).
#[derive(Debug, Clone)]
pub enum WebUiEvent {
    BankTransaction(BankTransactionEvent),
    DglabStrengthChange(DglabStrengthChangeEvent),
    PlayerStateChange(PlayerStateChangeEvent),
    WhitelistReload(WhitelistReloadEvent),
    CreatureConfigReload(CreatureConfigReloadEvent),
}

impl WebUiEvent {
    /// SSE `event:` field.
    pub fn name(&self) -> &'static str {
        match self {
            WebUiEvent::BankTransaction(_) => "bank_transaction",
            WebUiEvent::DglabStrengthChange(_) => "dglab_strength_change",
            WebUiEvent::PlayerStateChange(_) => "player_state_change",
            WebUiEvent::WhitelistReload(_) => "whitelist_reload",
            WebUiEvent::CreatureConfigReload(_) => "creature_config_reload",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BankTransactionEvent {
    pub account_uuid: Uuid,
    pub op: String,
    pub amount: i64,
    pub balance_after: i64,
    pub tick_millis: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DglabStrengthChangeEvent {
    pub owner_uuid: Uuid,
    pub channel_a: i32,
    pub channel_b: i32,
    pub source: String,
    pub tick_millis: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PlayerStateChangeEvent {
    pub player_uuid: Uuid,
    pub field: String,
    pub before: f32,
    pub after: f32,
    pub tick_millis: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WhitelistReloadEvent {
    pub loaded: usize,
    pub tick_millis: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CreatureConfigReloadEvent {
    pub reloaded: usize,
    pub failed: usize,
    pub tick_millis: i64,
}

/// Tokio broadcast bus. Subscribers (the SSE handler) clone the
/// `Sender` and pass it into a [`broadcast::Receiver`] loop.
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<WebUiEvent>,
}

impl EventBus {
    /// New bus with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Subscribe to the bus. Each subscriber gets its own
    /// receiver; a slow subscriber will see lagged events
    /// reflected in [`broadcast::error::RecvError::Lagged`].
    pub fn subscribe(&self) -> broadcast::Receiver<WebUiEvent> {
        self.tx.subscribe()
    }

    /// Publish an event. Returns the number of subscribers
    /// that saw the event.
    pub fn publish(&self, event: WebUiEvent) -> usize {
        self.tx.send(event).unwrap_or(0)
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(256)
    }
}

// ── Service bundle ──────────────────────────────────────────────────────────

/// Trait-erased references to the per-domain gRPC services that
/// the Web UI calls into. Kept as a single struct so the
/// handler modules can take it via `State<Arc<ServiceBundle>>`.
///
/// Each service is an `Arc<dyn Trait>` so the gRPC service
/// implementation (in `biocapital-grpc`) can be reused as-is.
#[derive(Clone)]
pub struct ServiceBundle {
    pub bank: Arc<dyn BankRepository>,
    pub contract: Arc<dyn ContractRepository>,
    pub creature: Arc<dyn CreatureConfigRepository>,
    pub dglab: Arc<dyn DglabRepository>,
    pub hardware_token: Arc<dyn HardwareTokenRepository>,
    pub player_state: Arc<dyn PlayerStateRepository>,
}

impl ServiceBundle {
    /// Bundle together the production PG-backed services.
    /// Used by the `biocapital-cli` startup path.
    pub fn from_pg(
        pool: PgPool,
    ) -> Self {
        use biocapital_creature::pg::PgCreatureConfigRepository;
        use biocapital_pg::{
            PgBankRepository, PgContractRepository, PgDglabRepository,
            PgHardwareTokenRepository, PgPlayerStateRepository,
        };
        Self {
            bank: Arc::new(PgBankRepository::from_pool(pool.clone())),
            contract: Arc::new(PgContractRepository::from_pool(pool.clone())),
            creature: Arc::new(PgCreatureConfigRepository::from_pool(pool.clone())),
            dglab: Arc::new(PgDglabRepository::from_pool(pool.clone())),
            hardware_token: Arc::new(PgHardwareTokenRepository::from_pool(pool.clone())),
            player_state: Arc::new(PgPlayerStateRepository::from_pool(pool.clone())),
        }
    }
}

// ── Top-level state ─────────────────────────────────────────────────────────

/// The struct that axum's `with_state` consumes. Handlers take
/// `State<Arc<WebUiApp>>`.
pub struct WebUiApp {
    /// The PG connection pool (shared with the gRPC layer).
    pub pg: PgPool,
    /// Per-domain services. Same instances as the gRPC layer.
    pub services: ServiceBundle,
    /// Static config (port, token set, …).
    pub config: WebUiConfig,
    /// Event bus for SSE.
    pub events: EventBus,
}

impl WebUiApp {
    /// Build a `WebUiApp` from the production PG pool + a
    /// config. The service bundle is wired to the same pool
    /// the gRPC layer uses.
    pub fn from_pg_pool(pg: PgPool, config: WebUiConfig) -> Arc<Self> {
        let services = ServiceBundle::from_pg(pg.clone());
        Arc::new(Self {
            pg,
            services,
            config,
            events: EventBus::default(),
        })
    }
}
