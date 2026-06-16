//! Bearer Token authentication for the Web UI HTTP API.
//!
//! Two token classes are recognised:
//!
//! 1. **Admin tokens** — listed in `biocapital-server.toml [WebUI] admin_tokens`.
//!    Each entry is a 32-byte hex string. Admin tokens grant full
//!    access (audit / admin routes).
//!
//! 2. **Viewer tokens** — issued in-game by the admin command
//!    `/biocapital admin grant_viewer <player>`. The format is
//!    `"v1." + <32-byte hex body>` so the verifier can route on
//!    the prefix. Viewer tokens are scoped to a single player
//!    UUID (the `subject` claim).
//!
//! Token verification uses constant-time comparison via [`subtle`]
//! to defeat timing attacks. Tokens are **never** logged (the
//! middleware logs only the token's SHA-256 fingerprint).
//!
//! The middleware lives in [`middleware`]. The auth extractors
//! ([`AdminContext`] / [`ViewerContext`]) are what handler
//! functions consume.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::error::WebUiError;
use crate::state::{TokenSet, WebUiApp};

// ── Constants ───────────────────────────────────────────────────────────────

/// HTTP header carrying the bearer token.
pub const AUTH_HEADER: &str = "Authorization";

/// `Authorization: Bearer <token>`.
pub const BEARER_PREFIX: &str = "Bearer ";

/// Prefix that distinguishes viewer tokens from admin tokens.
pub const VIEWER_TOKEN_PREFIX: &str = "v1.";

/// Number of hex chars in an admin / viewer token body.
pub const TOKEN_HEX_LEN: usize = 64;

// ── Authenticated principal ─────────────────────────────────────────────────

/// Authenticated identity extracted from the bearer token.
///
/// `admin` tokens are unscoped (the holder is "the admin"); viewer
/// tokens carry a player UUID (the `subject`).
#[derive(Debug, Clone)]
pub enum AuthPrincipal {
    /// Admin token. No subject — full authority.
    Admin,
    /// Viewer token scoped to a specific player.
    Viewer { subject: Uuid },
}

impl AuthPrincipal {
    /// True if the principal is an admin.
    pub fn is_admin(&self) -> bool {
        matches!(self, AuthPrincipal::Admin)
    }

    /// The player UUID this principal can act as, or `None` for admins.
    pub fn subject(&self) -> Option<Uuid> {
        match self {
            AuthPrincipal::Admin => None,
            AuthPrincipal::Viewer { subject } => Some(*subject),
        }
    }
}

/// Axum extractor. Handlers that require admin auth take
/// `AdminContext` as a parameter; the middleware inserts it.
#[derive(Debug, Clone, Copy)]
pub struct AdminContext {
    pub request_id: Uuid,
}

impl AdminContext {
    pub fn new() -> Self {
        Self {
            request_id: Uuid::new_v4(),
        }
    }
}

impl Default for AdminContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Axum extractor. Handlers scoped to the calling player take
/// `ViewerContext`; the middleware inserts it.
#[derive(Debug, Clone, Copy)]
pub struct ViewerContext {
    pub subject: Uuid,
    pub request_id: Uuid,
}

impl ViewerContext {
    pub fn new(subject: Uuid) -> Self {
        Self {
            subject,
            request_id: Uuid::new_v4(),
        }
    }
}

// ── Token verification ──────────────────────────────────────────────────────

/// Verify a raw bearer token against the configured token set.
/// Returns the matching [`AuthPrincipal`] on success.
///
/// Format rules:
/// - admin: bare 64-hex-char string
/// - viewer: `"v1."` + 64-hex-char body (the body hex-decodes to
///   32 bytes; the *last 16 bytes* of the decoded body encode the
///   subject UUID in big-endian order)
pub fn verify_token(raw: &str, tokens: &TokenSet) -> Option<AuthPrincipal> {
    let raw = raw.trim();

    // Viewer token path.
    if let Some(body) = raw.strip_prefix(VIEWER_TOKEN_PREFIX) {
        if body.len() != TOKEN_HEX_LEN {
            return None;
        }
        let bytes = hex::decode(body).ok()?;
        if bytes.len() != 32 {
            return None;
        }
        for known in &tokens.viewer_tokens {
            if known.as_slice().ct_eq(bytes.as_slice()).into() {
                let mut uuid_bytes = [0u8; 16];
                uuid_bytes.copy_from_slice(&bytes[16..32]);
                return Some(AuthPrincipal::Viewer {
                    subject: Uuid::from_bytes(uuid_bytes),
                });
            }
        }
        return None;
    }

    // Admin token path.
    if raw.len() != TOKEN_HEX_LEN {
        return None;
    }
    for known in &tokens.admin_tokens {
        // Compare as raw bytes (constant-time). The token list
        // stores 64-char hex; the raw bearer is also 64-char hex,
        // so byte-equal on the string is equivalent to byte-equal
        // on the underlying secret.
        if known.as_bytes().ct_eq(raw.as_bytes()).into() {
            return Some(AuthPrincipal::Admin);
        }
    }
    None
}

/// Extract a bearer token from an `Authorization` header. Returns
/// `None` when the header is missing or malformed.
pub fn extract_bearer(headers: &http::HeaderMap) -> Option<&str> {
    let h = headers.get(AUTH_HEADER)?.to_str().ok()?;
    h.strip_prefix(BEARER_PREFIX)
}

/// SHA-256 fingerprint of a token, for log lines. Never log the
/// raw token.
pub fn token_fingerprint(raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..8]) // 16 hex chars — plenty for log grep
}

// ── Middleware ──────────────────────────────────────────────────────────────

/// Axum middleware that requires *some* valid token. The
/// resolved [`AuthPrincipal`] is stored in request extensions;
/// the typed extractors ([`AdminContext`] / [`ViewerContext`])
/// read it back out.
pub async fn require_any_token(
    State(app): State<Arc<WebUiApp>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let request_id = Uuid::new_v4();
    let raw = match extract_bearer(req.headers()) {
        Some(s) => s.to_string(),
        None => {
            tracing::info!(
                target: "webui::auth",
                request_id = %request_id,
                path = %req.uri().path(),
                "missing Authorization header"
            );
            return unauthorized_response(request_id, "missing Authorization header");
        }
    };
    let principal = {
        let tokens = app.config.tokens.read().await;
        match verify_token(&raw, &tokens) {
            Some(p) => p,
            None => {
                tracing::warn!(
                    target: "webui::auth",
                    request_id = %request_id,
                    path = %req.uri().path(),
                    fingerprint = %token_fingerprint(&raw),
                    "token verification failed"
                );
                return unauthorized_response(request_id, "invalid token");
            }
        }
    };
    tracing::debug!(
        target: "webui::auth",
        request_id = %request_id,
        path = %req.uri().path(),
        principal = ?principal,
        "authenticated"
    );
    req.extensions_mut().insert(principal);
    next.run(req).await
}

/// Middleware variant that *also* enforces admin scope. Used
/// for `/admin/*` and `/audit/*` routes.
pub async fn require_admin_token(
    State(app): State<Arc<WebUiApp>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let request_id = Uuid::new_v4();
    let raw = match extract_bearer(req.headers()) {
        Some(s) => s.to_string(),
        None => {
            return unauthorized_response(request_id, "missing Authorization header");
        }
    };
    let principal = {
        let tokens = app.config.tokens.read().await;
        match verify_token(&raw, &tokens) {
            Some(p) => p,
            None => return unauthorized_response(request_id, "invalid token"),
        }
    };
    if !principal.is_admin() {
        tracing::warn!(
            target: "webui::auth",
            request_id = %request_id,
            path = %req.uri().path(),
            "non-admin attempted admin-only route"
        );
        return forbidden_response(request_id, "admin token required");
    }
    req.extensions_mut().insert(principal);
    next.run(req).await
}

// ── Helper: build error responses with the canonical envelope ───────────────

fn unauthorized_response(request_id: Uuid, message: &str) -> Response {
    let err = WebUiError::Unauthorized {
        message: message.to_string(),
        request_id,
    };
    err.into_response()
}

fn forbidden_response(request_id: Uuid, message: &str) -> Response {
    let err = WebUiError::Forbidden {
        message: message.to_string(),
        request_id,
    };
    err.into_response()
}

// ── Extractor helpers ───────────────────────────────────────────────────────

/// Pull an [`AuthPrincipal`] out of the request extensions. Used
/// by the [`AdminContext`] / [`ViewerContext`] extractor
/// implementations.
pub fn principal_from_req<B>(req: &Request<B>) -> Option<AuthPrincipal> {
    req.extensions().get::<AuthPrincipal>().cloned()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tokens() -> TokenSet {
        let mut t = TokenSet::default();
        t.admin_tokens
            .push("a".repeat(64));
        t.viewer_tokens
            .push(b"viewer-32-bytes-xxxxxxxxxxxxxx".to_vec());
        t
    }

    #[test]
    fn admin_token_matches() {
        let t = make_tokens();
        let p = verify_token(&"a".repeat(64), &t).unwrap();
        assert!(p.is_admin());
        assert!(p.subject().is_none());
    }

    #[test]
    fn admin_token_mismatch() {
        let t = make_tokens();
        let p = verify_token(&"b".repeat(64), &t);
        assert!(p.is_none());
    }

    #[test]
    fn viewer_token_matches_and_decodes_subject() {
        let t = make_tokens();
        // Build a viewer token whose last 16 bytes encode a known UUID.
        let subject = Uuid::new_v4();
        let mut body = [0u8; 32];
        body[16..32].copy_from_slice(subject.as_bytes());
        // Replace the viewer entry with the new body.
        let mut t2 = TokenSet::default();
        t2.viewer_tokens.push(body.to_vec());
        let raw = format!("v1.{}", hex::encode(body));
        let p = verify_token(&raw, &t2).unwrap();
        match p {
            AuthPrincipal::Viewer { subject: s } => assert_eq!(s, subject),
            _ => panic!("expected viewer"),
        }
    }

    #[test]
    fn viewer_token_wrong_prefix_rejected() {
        let t = make_tokens();
        let p = verify_token(&"x".repeat(64), &t);
        // Admin path: 64 hex chars but unknown body → None.
        assert!(p.is_none());
    }

    #[test]
    fn token_fingerprint_is_stable() {
        let f1 = token_fingerprint("abc");
        let f2 = token_fingerprint("abc");
        assert_eq!(f1, f2);
        let f3 = token_fingerprint("abd");
        assert_ne!(f1, f3);
    }
}
