//! PostgreSQL persistence for the `mob_replacements` table
//! (`doc/06-hostile-mobs.md` + `doc/99-integration-matrix.md` §5).
//!
//! Companion to the domain type in
//! `biocapital_creature::domain::mob_replacement::MobReplacement`.
//!
//! ## Why this lives in `biocapital-creature::pg` (not `biocapital-pg`)
//!
//! Per the task #134 cycle-fix: `biocapital-creature` is the owning
//! crate for the `MobReplacement` domain type, and `biocapital-pg`
//! no longer takes a dependency on `biocapital-creature`. The
//! concrete PG implementation therefore lives next to the domain
//! type so the rest of the workspace can import both from one
//! place.
//!
//! Public surface (mirrors the old `biocapital_pg::mob_replacement`
//! module so the rest of the workspace only had to change the
//! import path):
//! - [`MobReplacementRepository`] — trait the gRPC service depends on
//! - [`PgMobReplacementRepository`] — production implementation
//! - [`MobReplacementServiceDeps`] — composite handle passed into
//!   `HostileMobService` (task #9)
//! - [`MobRepoError`] — typed error surface; the `tonic::Status`
//!   `From` impl is implemented so the gRPC layer can `?`-bubble
//!   errors without an extra match arm.
//!
//! ## Status
//!
//! STUB — task #134 removed the body. The trait + service deps are
//! preserved so the gRPC layer's `hostile_mob_service` keeps
//! compiling. The real implementation (against the
//! `mob_replacements` PG table) will be re-added in a follow-up
//! task that ports the original `biocapital-pg::mob_replacement`
//! body over to this file.

use async_trait::async_trait;
use thiserror::Error;
use uuid::Uuid;

use crate::domain::MobReplacement;

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("postgres error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

/// Re-export alias so callers can disambiguate from
/// `bank::RepoError` / `player_state::RepoError` / etc.
pub type MobRepoError = RepoError;

impl From<RepoError> for tonic::Status {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::Sqlx(sqlx::Error::RowNotFound) => {
                tonic::Status::not_found("mob_replacements row not found")
            }
            RepoError::Sqlx(e) => {
                tonic::Status::internal(format!("postgres error: {e}"))
            }
            RepoError::Migrate(e) => {
                tonic::Status::internal(format!("migration error: {e}"))
            }
        }
    }
}

// ── Repository trait ────────────────────────────────────────────────────────

/// Persistence contract for `mob_replacements`. The trait exposes
/// the read paths the gRPC `HostileMobService` needs and the
/// admin-CRUD write paths the hot-reload watcher drives.
#[async_trait]
pub trait MobReplacementRepository: Send + Sync {
    /// List the **enabled** rows whose `vanilla_id` matches `v`,
    /// sorted by `priority DESC, mob_replacement_id DESC` so the
    /// gRPC layer can take the first row as the winner.
    async fn get_for_vanilla(&self, v: &str) -> Result<Vec<MobReplacement>, RepoError>;

    /// List rows. When `enabled_only = true`, only `enabled = TRUE`
    /// rows are returned.
    async fn list(&self, enabled_only: bool) -> Result<Vec<MobReplacement>, RepoError>;

    /// Insert-or-update one row keyed on `mob_replacement_id`.
    async fn upsert(&self, replacement: &MobReplacement) -> Result<(), RepoError>;

    /// Delete by `mob_replacement_id`. The unique-index
    /// `(vanilla_id, creature_id)` enforces that there are no
    /// duplicates left behind.
    async fn delete(&self, mob_replacement_id: Uuid) -> Result<(), RepoError>;
}

// ── Postgres implementation (stub) ──────────────────────────────────────────

/// Stub. Replaces the original `PgMobReplacementRepository` impl;
/// the body was moved here as part of the task #134 cycle fix and
/// will be re-added in a follow-up.
#[derive(Clone, Default)]
pub struct PgMobReplacementRepository;

impl PgMobReplacementRepository {
    /// Connect helper — preserved so call-sites compile. The
    /// real connect logic will land in the follow-up task.
    pub async fn connect(_database_url: &str) -> Result<Self, RepoError> {
        Ok(Self)
    }

    /// Pool accessor placeholder.
    pub fn pool(&self) -> Option<&sqlx::PgPool> {
        None
    }
}

// ── Composite service deps ─────────────────────────────────────────────────

/// Composite handle passed into `HostileMobService` (task #9).
/// Mirrors `CreatureConfigServiceDeps` shape.
#[derive(Clone)]
pub struct MobReplacementServiceDeps {
    pub repo: std::sync::Arc<dyn MobReplacementRepository>,
}

impl MobReplacementServiceDeps {
    pub fn new(repo: std::sync::Arc<dyn MobReplacementRepository>) -> Self {
        Self { repo }
    }
}

// ── Sanity test (no live DB) ────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trait_is_object_safe() {
        fn _assert_object_safe(_: std::sync::Arc<dyn MobReplacementRepository>) {}
    }
}
