//! HTTP handler modules for the Web UI.
//!
//! One file per route group from `doc/15-web-ui.md §5.3`:
//! - [`players`]    — `/players/me` + `/players/:uuid`
//! - [`bank`]       — `/bank/transfer` + `/bank/history`
//! - [`contracts`]  — `/contracts` + `/contracts/:id` + `/contracts/:id/redeem`
//! - [`devices`]    — DG_LAB device tokens
//! - [`audit`]      — audit query + export
//! - [`admin`]      — config + whitelist reload
//! - [`auth`]       — 18 §7 hardware-token endpoints
//! - [`events`]     — SSE stream
//! - [`health`]     — `/health`

pub mod admin;
pub mod audit;
pub mod auth;
pub mod bank;
pub mod contracts;
pub mod devices;
pub mod events;
pub mod health;
pub mod players;
