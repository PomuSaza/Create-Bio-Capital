//! SSE event stream — `GET /events`.
//!
//! See `doc/15-web-ui.md §8`. The stream is a single endpoint
//! that multiplexes all event kinds, with each event
//! identified by its `event:` field. A subscriber's `?filter=`
//! query parameter narrows the stream to specific event kinds
//! (comma-separated list of `bank_transaction` /
//! `dglab_strength_change` / `player_state_change` /
//! `whitelist_reload` / `creature_config_reload`).
//!
//! Auth: any valid token. The `require_any_token` middleware
//! that wraps this route in [`crate::router`] enforces that.
//! Admins see all events; viewers see only events whose
//! payload includes their own UUID (subject filter on the
//! server side).
//!
//! Implementation note: this module uses `futures::stream` +
//! `tokio::sync::broadcast` directly. We deliberately avoid
//! `tokio_stream::wrappers::BroadcastStream` so the crate
//! doesn't need a new dependency. Lagged events are logged
//! and dropped (clients should re-poll on next reconnect).

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
};
use futures_core::Stream;
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::state::{WebUiApp, WebUiEvent};

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    #[serde(default)]
    pub filter: Option<String>,
}

/// `GET /events?filter=bank_transaction,dglab_strength_change`
///
/// Returns an SSE stream. The keep-alive interval is 30 s
/// (matches `doc/15 §8.3`).
pub async fn sse_events(
    State(app): State<Arc<WebUiApp>>,
    Query(q): Query<EventsQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let allow: Vec<String> = q
        .filter
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut rx = app.events.subscribe();

    let sse = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if !allow.is_empty() && !allow.iter().any(|k| k == event.name()) {
                        continue;
                    }
                    let data = match serde_json::to_string(&event_payload(&event)) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    yield Ok::<_, Infallible>(
                        Event::default().event(event.name()).data(data),
                    );
                }
                Err(RecvError::Lagged(skipped)) => {
                    // Surface a "lagged" event so the client
                    // knows to refresh.
                    yield Ok::<_, Infallible>(
                        Event::default()
                            .event("lagged")
                            .data(format!(r#"{{"skipped":{skipped}}}"#)),
                    );
                    continue;
                }
                Err(RecvError::Closed) => {
                    // Bus is closed (server shutting down). Send
                    // a final "shutdown" event and break the loop.
                    yield Ok::<_, Infallible>(
                        Event::default().event("shutdown").data("{}"),
                    );
                    break;
                }
            }
        }
    };

    Sse::new(sse).keep_alive(KeepAlive::new().interval(Duration::from_secs(30)))
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", content = "payload")]
enum EventPayload<'a> {
    BankTransaction(&'a crate::state::BankTransactionEvent),
    DglabStrengthChange(&'a crate::state::DglabStrengthChangeEvent),
    PlayerStateChange(&'a crate::state::PlayerStateChangeEvent),
    WhitelistReload(&'a crate::state::WhitelistReloadEvent),
    CreatureConfigReload(&'a crate::state::CreatureConfigReloadEvent),
}

fn event_payload(e: &WebUiEvent) -> EventPayload<'_> {
    match e {
        WebUiEvent::BankTransaction(x) => EventPayload::BankTransaction(x),
        WebUiEvent::DglabStrengthChange(x) => EventPayload::DglabStrengthChange(x),
        WebUiEvent::PlayerStateChange(x) => EventPayload::PlayerStateChange(x),
        WebUiEvent::WhitelistReload(x) => EventPayload::WhitelistReload(x),
        WebUiEvent::CreatureConfigReload(x) => EventPayload::CreatureConfigReload(x),
    }
}
