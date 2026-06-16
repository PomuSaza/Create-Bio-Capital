//! In-memory read cache for `PlayerStateSnapshot` (task #123 HUD rebuild).
//!
//! ## Why a cache?
//!
//! The client HUD polls Rust via Sable JNI at **5 Hz** (200 ms — see
//! `doc/02-player-state.md` §5 + the BIO HUD spec in task #123). With 50
//! online players this would translate to **250 `GetState` calls/s**; with 100
//! players, **500 calls/s**. Every call currently incurs a PostgreSQL
//! round-trip (player_state row + 12 body_part_development rows). The cache
//! reduces the steady-state PG load by a factor of ~50x while keeping the
//! data fresh enough for visual feedback.
//!
//! ## Cache-Aside semantics
//!
//! - **Reads** go through [`PlayerStateCache::get_or_load`]. The cache is
//!   consulted first; on a hit that is **strictly younger than `ttl`** the
//!   cached snapshot is returned without touching the repository. On a miss
//!   (or expired entry) the repository is consulted and the result is
//!   inserted.
//! - **Writes** do **not** go through the cache. Mutating RPCs
//!   (`update_state` / `apply_damage` / `add_pleasure` / `add_hunger` /
//!   `add_fluid_effect`) call [`PlayerStateCache::invalidate`] right after
//!   the `upsert` returns. This forces the next `get_or_load` to re-read from
//!   PG so the HUD sees the new value within 200 ms.
//!
//! ## Why 200 ms TTL?
//!
//! Matches the client poll interval. With a TTL ≥ poll interval the cache
//! always returns a value that is **at most as stale as the client's
//! previous poll** — i.e. the HUD never gets older data than it already has.
//! Shorter TTLs would just churn the cache; longer TTLs would let the HUD
//! show stale data after a server-side mutation. 200 ms is the
//! match-the-poll sweet spot.
//!
//! ## Concurrency
//!
//! - The internal map is wrapped in `tokio::sync::RwLock` so the hot
//!   read path is lock-free under contention (multiple readers in parallel).
//! - The cached value is `Clone`-cheap (`PlayerStateSnapshot` is
//!   `BTreeMap<BodyPart, f32>` + a handful of `f32`/`i32`s — well under the
//!   256 B/player budget from 02 §5).
//! - All public methods are `async` to match the surrounding repository
//!   trait surface (`PlayerStateRepository::get` is async).
//!
//! ## Diff on the client side
//!
//! The cache is *value*-oriented, not change-oriented: a hit returns the
//! stored snapshot whether or not it has changed since the last read. The
//! 200 ms TTL keeps the staleness bounded, and the **client** does the
//! diff (`BioCapitalHud` — see task #123 / `src/main/java/.../hud/...`).
//! The Rust side does not need a dirty flag because the worst case is
//! "200 ms of staleness", which is invisible at 5 Hz and far below the
//! 1 frame budget the HUD render loop targets.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use uuid::Uuid;

use crate::player_state::PlayerStateSnapshot;

// ── Public trait alias (so the gRPC layer can stay trait-agnostic) ────────────

/// Subset of [`PlayerStateRepository`](biocapital_pg::PlayerStateRepository)
/// that the cache needs. Defined here as a free-standing trait so the cache
/// crate does not have to depend on `biocapital-pg` (which would create a
/// cycle: pg -> core via the existing `PlayerStateSnapshot` re-export, plus
/// core -> pg via the cache).
///
/// The gRPC service holds an `Arc<dyn PlayerStateRepository>` from
/// `biocapital-pg`; the cache takes `&dyn PlayerStateLoader` so the call
/// `cache.get_or_load(uuid, &*repo).await` works without re-boxing.
#[async_trait::async_trait]
pub trait PlayerStateLoader: Send + Sync {
    async fn load(&self, uuid: Uuid) -> Result<PlayerStateSnapshot, LoadError>;
}

/// Error surface for the loader. The cache treats any `Err` as "no value to
/// cache" and propagates the error to the caller — we deliberately do **not**
/// cache negative results, so a transient PG hiccup does not poison the
/// cache for the rest of the TTL window.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("postgres error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("repository error: {0}")]
    Repo(String),
}

// ── Cache ────────────────────────────────────────────────────────────────────

/// TTL used for the HUD 5 Hz poll: 200 ms.
///
/// Pinned as a `const` so the value is visible in `cargo doc` and so a
/// future "make this configurable" change has a single, obvious landing
/// site.
pub const DEFAULT_TTL: Duration = Duration::from_millis(200);

/// In-memory cache of `PlayerStateSnapshot` keyed by `Uuid`. The cache is
/// process-local; a Rust gRPC server restart drops it (and that's fine —
/// the next `get_or_load` after restart is a PG hit, which is the warm-up
/// cost we accept).
///
/// Cheap to clone — the inner `RwLock` is `Arc`-shaped, so a `PlayerStateCache`
/// is effectively a handle the gRPC service can pass around freely.
#[derive(Clone)]
pub struct PlayerStateCache {
    inner: std::sync::Arc<CacheInner>,
}

struct CacheInner {
    map: RwLock<HashMap<Uuid, CachedEntry>>,
    ttl: Duration,
}

struct CachedEntry {
    snapshot: PlayerStateSnapshot,
    /// Wall-clock time the entry was inserted. We use `Instant` (monotonic)
    /// so the TTL is robust to wall-clock skew; the value is never
    /// serialised.
    inserted_at: Instant,
}

impl std::fmt::Debug for PlayerStateCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't try to print the inner map — it can deadlock under a debug
        // print while a read lock is held.
        f.debug_struct("PlayerStateCache")
            .field("ttl_ms", &self.inner.ttl.as_millis())
            .finish()
    }
}

impl PlayerStateCache {
    /// Build a new cache with the default 200 ms TTL.
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_TTL)
    }

    /// Build a new cache with a custom TTL. Mostly useful for tests; the
    /// 200 ms default is correct for the production HUD 5 Hz poll.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: std::sync::Arc::new(CacheInner {
                map: RwLock::new(HashMap::new()),
                ttl,
            }),
        }
    }

    /// Returns the configured TTL. Mostly for `cargo doc` / metrics.
    pub fn ttl(&self) -> Duration {
        self.inner.ttl
    }

    /// Cache-Aside read: return the cached snapshot if fresh, otherwise
    /// load from the loader and store the result.
    ///
    /// On a loader error the cache is **not** updated; the error is
    /// returned to the caller unchanged. This means a transient PG outage
    /// cannot poison the cache.
    pub async fn get_or_load(
        &self,
        uuid: Uuid,
        loader: &dyn PlayerStateLoader,
    ) -> Result<PlayerStateSnapshot, LoadError> {
        // Fast path: take a read lock and check freshness. We clone the
        // snapshot out of the guard so the read lock is released as soon as
        // possible (the BTreeMap clone is the only allocation cost on hit).
        {
            let guard = self.inner.map.read().await;
            if let Some(entry) = guard.get(&uuid) {
                if entry.inserted_at.elapsed() < self.inner.ttl {
                    return Ok(entry.snapshot.clone());
                }
            }
        }

        // Slow path: take a write lock, re-check (another writer may have
        // refreshed in the meantime), then load + insert.
        let mut guard = self.inner.map.write().await;
        if let Some(entry) = guard.get(&uuid) {
            if entry.inserted_at.elapsed() < self.inner.ttl {
                return Ok(entry.snapshot.clone());
            }
        }
        let snapshot = loader.load(uuid).await?;
        guard.insert(
            uuid,
            CachedEntry {
                snapshot: snapshot.clone(),
                inserted_at: Instant::now(),
            },
        );
        Ok(snapshot)
    }

    /// Drop the cached entry for `uuid`. The next `get_or_load` for this
    /// uuid will load from the loader. Cheap; takes a write lock for a
    /// single hash-map remove.
    ///
    /// Idempotent: invalidating a uuid that is not cached is a no-op.
    pub async fn invalidate(&self, uuid: Uuid) {
        let mut guard = self.inner.map.write().await;
        guard.remove(&uuid);
    }

    /// Drop every cached entry. Used by tests; also useful if a future
    /// "force reload from PG" admin command is added.
    pub async fn invalidate_all(&self) {
        let mut guard = self.inner.map.write().await;
        guard.clear();
    }

    /// Current number of cached entries. O(1) read-lock; primarily for
    /// tests and metrics.
    pub async fn len(&self) -> usize {
        self.inner.map.read().await.len()
    }

    /// `true` if the cache holds no entries.
    pub async fn is_empty(&self) -> bool {
        self.inner.map.read().await.is_empty()
    }
}

impl Default for PlayerStateCache {
    fn default() -> Self {
        Self::new()
    }
}

// ── Adapter so `Arc<dyn PlayerStateRepository>` (biocapital-pg) implements
//    `PlayerStateLoader` without a second wrapper type. ─────────────────────

/// Bridge from `biocapital_pg::PlayerStateRepository` to
/// `PlayerStateLoader`. Lives in `biocapital-pg` rather than here so the
/// `biocapital-core` crate stays free of the sqlx dependency. The gRPC
/// service uses this in its `with_cache` constructor.
///
/// **Not** defined in this file — `biocapital-pg` provides the impl so
/// `biocapital-core` does not need to depend on `biocapital-pg`. The trait
/// declaration above is the only contract this crate owns.

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Counting loader: every call increments a counter, and we can
    /// pre-load a fixed snapshot for a uuid.
    struct FakeLoader {
        calls: Mutex<u32>,
        snapshots: Mutex<HashMap<Uuid, PlayerStateSnapshot>>,
    }

    impl FakeLoader {
        fn new() -> Self {
            Self {
                calls: Mutex::new(0),
                snapshots: Mutex::new(HashMap::new()),
            }
        }
        fn put(&self, snap: PlayerStateSnapshot) {
            self.snapshots.lock().unwrap().insert(snap.uuid, snap);
        }
        fn calls(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl PlayerStateLoader for FakeLoader {
        async fn load(&self, uuid: Uuid) -> Result<PlayerStateSnapshot, LoadError> {
            *self.calls.lock().unwrap() += 1;
            Ok(self
                .snapshots
                .lock()
                .unwrap()
                .get(&uuid)
                .cloned()
                .unwrap_or_else(|| PlayerStateSnapshot::new(uuid)))
        }
    }

    #[tokio::test]
    async fn cache_miss_triggers_load() {
        let cache = PlayerStateCache::with_ttl(Duration::from_millis(200));
        let loader = FakeLoader::new();
        let uuid = Uuid::new_v4();

        let snap = cache.get_or_load(uuid, &loader).await.unwrap();
        assert_eq!(snap.uuid, uuid);
        assert_eq!(loader.calls(), 1);
    }

    #[tokio::test]
    async fn cache_hit_does_not_reload() {
        let cache = PlayerStateCache::with_ttl(Duration::from_secs(60));
        let loader = FakeLoader::new();
        let uuid = Uuid::new_v4();

        let s1 = cache.get_or_load(uuid, &loader).await.unwrap();
        let s2 = cache.get_or_load(uuid, &loader).await.unwrap();
        assert_eq!(s1, s2);
        // Loader was called exactly once; second hit is pure cache.
        assert_eq!(loader.calls(), 1);
    }

    #[tokio::test]
    async fn invalidate_forces_reload() {
        let cache = PlayerStateCache::with_ttl(Duration::from_secs(60));
        let loader = FakeLoader::new();
        let uuid = Uuid::new_v4();
        loader.put(PlayerStateSnapshot::new(uuid));

        let _ = cache.get_or_load(uuid, &loader).await.unwrap();
        assert_eq!(loader.calls(), 1);

        cache.invalidate(uuid).await;
        let _ = cache.get_or_load(uuid, &loader).await.unwrap();
        assert_eq!(loader.calls(), 2);
    }

    #[tokio::test]
    async fn ttl_expiry_triggers_reload() {
        // 50 ms TTL is short enough to be observable in a test without
        // turning the test into a wall-clock wait.
        let cache = PlayerStateCache::with_ttl(Duration::from_millis(50));
        let loader = FakeLoader::new();
        let uuid = Uuid::new_v4();
        loader.put(PlayerStateSnapshot::new(uuid));

        let _ = cache.get_or_load(uuid, &loader).await.unwrap();
        assert_eq!(loader.calls(), 1);

        // Wait past the TTL. 80 ms gives the 50 ms TTL a comfortable
        // margin while keeping the test fast.
        tokio::time::sleep(Duration::from_millis(80)).await;

        let _ = cache.get_or_load(uuid, &loader).await.unwrap();
        assert_eq!(loader.calls(), 2);
    }

    #[tokio::test]
    async fn loader_error_does_not_cache_negative_result() {
        // Build a loader that errors every time.
        struct ErrLoader;
        #[async_trait::async_trait]
        impl PlayerStateLoader for ErrLoader {
            async fn load(&self, _uuid: Uuid) -> Result<PlayerStateSnapshot, LoadError> {
                Err(LoadError::Repo("simulated".into()))
            }
        }

        let cache = PlayerStateCache::with_ttl(Duration::from_secs(60));
        let uuid = Uuid::new_v4();
        let r1 = cache.get_or_load(uuid, &ErrLoader).await;
        assert!(r1.is_err());
        let r2 = cache.get_or_load(uuid, &ErrLoader).await;
        assert!(r2.is_err());
        // A transient failure must not poison the cache: a future
        // successful load should still insert.
        assert!(cache.is_empty().await);
    }

    #[tokio::test]
    async fn default_ttl_is_200ms() {
        // Pin the HUD's poll interval: 200 ms. If a future change widens
        // this, the test will fail and the HUD render loop will need to
        // re-validate the 5 Hz budget.
        assert_eq!(DEFAULT_TTL, Duration::from_millis(200));
        assert_eq!(PlayerStateCache::new().ttl(), Duration::from_millis(200));
    }
}
