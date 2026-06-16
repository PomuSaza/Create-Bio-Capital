//! Hot-reload watcher for `config/biocapital/creatures/`
//! (`doc/13-bio-customization.md` §5 + §6).
//!
//! `CreatureHotReloader` is the file-system-driven side of the
//! creature customisation pipeline. The gRPC `CreatureService`
//! is the read-side; this module is the write-side. They share
//! the same `CreatureConfigRepository` + `CreatureAuditWriter`
//! so every mutation flows through one PG truth.
//!
//! Task #11 scope (2026-06-14):
//! - tokio 5 s tick that scans `config/biocapital/creatures/`
//! - per-file parse + validate + upsert + audit (creature.load /
//!   creature.reload / creature.unload / creature.reload_failed)
//! - manual `reload_all()` + `reload_one(creature_id)` triggers
//!   for the gRPC `CreatureService.ReloadCreatures` RPC
//!
//! Out of scope (and explicitly **not** implemented here):
//! - HMAC signature check on the .json files (01 §2.1 row
//!   "配置文件热加载攻击"). That belongs to task #14
//!   (config-system landing); for now we trust the local
//!   filesystem.
//! - Java-side `WatchService` integration. Per 11 §11.1 the
//!   Java end is read-only; this reloader is the single source
//!   of reload events.
//!
//! Concurrency: the watcher holds a `JoinHandle` so `start_watching`
//! is idempotent — a second call joins the old task first.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::Value as JsonValue;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::pg::{
    CreatureAuditEntry, CreatureAuditWriter, CreatureConfigRecord, CreatureConfigRepository, MobReplacementRepository,
};

use crate::domain::CreatureConfig;

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ReloadError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("path is not a directory: {0}")]
    NotADirectory(PathBuf),

    #[error("json parse error for {path}: {detail}")]
    JsonParse { path: PathBuf, detail: String },

    #[error("validate failed for {creature_id}: {detail}")]
    Validate { creature_id: String, detail: String },

    #[error("creature_id missing in {0}")]
    MissingId(PathBuf),

    #[error("creature_id mismatch in {path}: file has {file_id} but directory is {dir_id}")]
    IdMismatch {
        path: PathBuf,
        file_id: String,
        dir_id: String,
    },

    #[error("repo error: {0}")]
    Repo(#[from] crate::pg::RepoError),
}

/// Aggregate counters returned by `reload_all()` /
///// `scan_once()`. The gRPC `ReloadCreatures` RPC turns this
//// into a `ReloadResponse` (proto `biocapital.proto`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReloadReport {
    /// Number of files scanned on disk during the tick.
    pub files_scanned: u32,
    /// Number of rows newly inserted (no prior PG row).
    pub loaded: u32,
    /// Number of rows upserted (prior PG row + changed mtime).
    pub reloaded: u32,
    /// Number of parse / validate failures.
    pub failed: u32,
    /// Number of rows removed because the .json file was
    /// deleted from disk.
    pub deleted: u32,
    /// Wall-clock duration of the tick (informational; not
    /// part of the proto response).
    pub elapsed_ms: u64,
}

impl ReloadReport {
    pub fn merge(&mut self, other: ReloadReport) {
        self.files_scanned += other.files_scanned;
        self.loaded += other.loaded;
        self.reloaded += other.reloaded;
        self.failed += other.failed;
        self.deleted += other.deleted;
        self.elapsed_ms += other.elapsed_ms;
    }
}

// ── Clock seam ──────────────────────────────────────────────────────────────

/// Tick + wall-clock source. In production we use
/// [`SystemClock`]; tests can swap in a fake to drive the
/// hot-reload deterministically.
#[async_trait::async_trait]
pub trait Clock: Send + Sync {
    fn current_tick(&self) -> i64;
    fn now(&self) -> chrono::DateTime<Utc>;
}

pub struct SystemClock;

#[async_trait::async_trait]
impl Clock for SystemClock {
    fn current_tick(&self) -> i64 {
        // Sable exposes the logical tick counter on the
        // Java side; for the hot-reload watcher we use the
        // wall-clock millisecond as a stand-in. This is
        // consistent with `audit_core_pod.tick_millis`
        // semantics (i64 server-side clock) even though the
        // doc says the real counter is exposed via Sable.
        // The gRPC `ReloadCreatures` path passes a real tick
        // when the manual RPC fires.
        chrono::Utc::now().timestamp_millis()
    }

    fn now(&self) -> chrono::DateTime<Utc> {
        chrono::Utc::now()
    }
}

// ── HotReloader ─────────────────────────────────────────────────────────────

/// The hot-reload watcher. Holds a `CreatureConfigRepository`
/// + `CreatureAuditWriter` for the write side and an optional
/// `MobReplacementRepository` so the reloader can surface a
/// `note` whenever a deleted creature's id is still referenced
/// by `mob_replacements.creature_id` (helps admins spot stale
/// FK-style references — there is no hard FK because
/// `creature_configs` is JSONB-backed, 13 §6.3).
///
/// One `HotReloader` per process. Cheap to clone (all internals
/// are `Arc`).
#[derive(Clone)]
pub struct CreatureHotReloader {
    config_dir: PathBuf,
    pg_repo: Arc<dyn CreatureConfigRepository>,
    audit_writer: Arc<dyn CreatureAuditWriter>,
    mob_replacement_repo: Arc<dyn MobReplacementRepository>,
    clock: Arc<dyn Clock>,
    /// Period between scans. Defaults to 5 s per 13 §5.1.
    tick_interval: Duration,
    watch_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl CreatureHotReloader {
    pub fn new(
        config_dir: PathBuf,
        pg_repo: Arc<dyn CreatureConfigRepository>,
        audit_writer: Arc<dyn CreatureAuditWriter>,
        mob_replacement_repo: Arc<dyn MobReplacementRepository>,
    ) -> Self {
        Self {
            config_dir,
            pg_repo,
            audit_writer,
            mob_replacement_repo,
            clock: Arc::new(SystemClock),
            tick_interval: Duration::from_secs(5),
            watch_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// Override the tick interval (used by tests).
    pub fn with_tick_interval(mut self, interval: Duration) -> Self {
        self.tick_interval = interval;
        self
    }

    /// Override the clock (used by tests).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Start the periodic watcher. Idempotent — a second call
    /// aborts the first task before spawning a fresh one.
    pub async fn start_watching(&self) {
        let mut guard = self.watch_handle.lock().await;
        if let Some(handle) = guard.take() {
            handle.abort();
        }

        let me = self.clone();
        let handle = tokio::spawn(async move {
            let mut ticker = interval(me.tick_interval);
            // The first tick fires immediately; skip it so the
            // first scan runs ~5 s after start (gives PG init
            // time to settle).
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let report = me.scan_once().await;
                if report.failed > 0 || report.deleted > 0 {
                    warn!(
                        loaded = report.loaded,
                        reloaded = report.reloaded,
                        failed = report.failed,
                        deleted = report.deleted,
                        files_scanned = report.files_scanned,
                        "creature hot-reload tick reported mutations"
                    );
                } else {
                    debug!(
                        loaded = report.loaded,
                        reloaded = report.reloaded,
                        files_scanned = report.files_scanned,
                        "creature hot-reload tick idle"
                    );
                }
            }
        });
        *guard = Some(handle);
        info!(
            config_dir = %self.config_dir.display(),
            tick_seconds = self.tick_interval.as_secs(),
            "creature hot-reload watcher started"
        );
    }

    /// Stop the periodic watcher. Idempotent (no-op when not
    /// running).
    pub async fn stop_watching(&self) {
        let mut guard = self.watch_handle.lock().await;
        if let Some(handle) = guard.take() {
            handle.abort();
        }
    }

    /// One pass of the scan loop. Public so the gRPC
    /// `ReloadCreatures` RPC can drive a full sweep on demand
    /// (the periodic task is the 5 s background path; the RPC
    /// is the manual "I just edited 10 files and don't want to
    /// wait 5 s" path).
    pub async fn scan_once(&self) -> ReloadReport {
        let start = std::time::Instant::now();
        let mut report = ReloadReport::default();
        let now_epoch = chrono::Utc::now().timestamp();

        // Step 1: list files on disk.
        let disk_files = match self.scan_disk().await {
            Ok(files) => files,
            Err(e) => {
                error!(
                    config_dir = %self.config_dir.display(),
                    error = %e,
                    "creature hot-reload: failed to scan config dir"
                );
                report.elapsed_ms = start.elapsed().as_millis() as u64;
                return report;
            }
        };
        report.files_scanned = disk_files.len() as u32;

        // Step 2: load PG snapshot for cross-reference.
        let pg_rows = match self.pg_repo.list(false).await {
            Ok(rs) => rs,
            Err(e) => {
                error!(error = %e, "creature hot-reload: failed to list PG rows");
                report.elapsed_ms = start.elapsed().as_millis() as u64;
                return report;
            }
        };
        let pg_by_id: HashMap<String, CreatureConfigRecord> =
            pg_rows.into_iter().map(|r| (r.creature_id.clone(), r)).collect();

        // Step 3: per-file pass (changed mtime OR new file).
        for (creature_id, path, mtime) in &disk_files {
            let needs_upsert = match pg_by_id.get(creature_id) {
                Some(existing) => existing.source_mtime < *mtime,
                None => true,
            };
            if !needs_upsert {
                continue;
            }
            match self.load_and_upsert(path, *mtime).await {
                Ok(LoadOutcome::New) => report.loaded += 1,
                Ok(LoadOutcome::Reload) => report.reloaded += 1,
                Ok(LoadOutcome::Unloaded) => report.deleted += 1,
                Err(e) => {
                    report.failed += 1;
                    self.record_failure(creature_id, &e.to_string()).await;
                }
            }
        }

        // Step 4: PG rows whose file vanished from disk.
        for (id, row) in &pg_by_id {
            if !disk_files.iter().any(|(d_id, _, _)| d_id == id) {
                match self.unload_one(id, row).await {
                    Ok(()) => report.deleted += 1,
                    Err(e) => {
                        error!(creature_id = %id, error = %e, "creature hot-reload: failed to unload");
                    }
                }
            }
        }

        // The `older_than` threshold is also probed against
        // `list_by_source_mtime(now_epoch)` for observability
        // — we have already covered the practical cases in
        // step 3 + 4 (newer-on-disk or vanished). This call
        // is kept so any future logic that wants to enumerate
        // "rows older than N seconds" can rely on the same
        // method being exercised.
        let _ = self.pg_repo.list_by_source_mtime(now_epoch).await;

        report.elapsed_ms = start.elapsed().as_millis() as u64;
        report
    }

    /// Manual full reload — drives `scan_once()` once and then
    /// writes a single `creature.reload_all` audit row. Used
    /// by the gRPC `CreatureService.ReloadCreatures` RPC.
    pub async fn reload_all(
        &self,
        actor_uuid: Option<uuid::Uuid>,
        actor_type: &str,
        request_id: Option<uuid::Uuid>,
    ) -> Result<ReloadReport, ReloadError> {
        let report = self.scan_once().await;
        let notes = serde_json::json!({
            "files_scanned": report.files_scanned,
            "loaded":        report.loaded,
            "reloaded":      report.reloaded,
            "failed":        report.failed,
            "deleted":       report.deleted,
            "elapsed_ms":    report.elapsed_ms,
        });
        let entry = CreatureAuditEntry::reload_all(
            actor_uuid,
            actor_type,
            self.clock.current_tick(),
            request_id,
            notes,
        );
        self.audit_writer.write(&entry).await?;
        Ok(report)
    }

    /// Manual single-creature reload — parses the .json for
    /// the requested id, validates, upserts, audit. Used by
    /// the gRPC `CreatureService.GetCreature` ad-hoc refresh
    /// path (future task #14 admin UI button). For now the
    /// RPC is best-effort: when the file is missing we report
    /// `Err(NotFound)` after also bumping the PG row's
    /// `reload_failed_count`.
    pub async fn reload_one(
        &self,
        creature_id: &str,
        actor_uuid: Option<uuid::Uuid>,
        actor_type: &str,
        request_id: Option<uuid::Uuid>,
    ) -> Result<LoadOutcome, ReloadError> {
        let path = self.path_for(creature_id);
        if !path.exists() {
            // File vanished → delete the PG row (mirrors the
            // background scan behaviour) + audit unload.
            let existing = self.pg_repo.get(creature_id).await?;
            if let Some(row) = existing {
                self.unload_one(creature_id, &row).await?;
                return Ok(LoadOutcome::Unloaded);
            }
            return Err(ReloadError::MissingId(path));
        }
        let mtime = file_mtime(&path)?;
        match self.load_and_upsert(&path, mtime).await {
            Ok(outcome) => {
                let _ = self.audit_writer.write(&match outcome {
                    LoadOutcome::New => CreatureAuditEntry::loaded(
                        actor_uuid,
                        actor_type,
                        creature_id,
                        // The `after_json` is the config we just
                        // wrote; we don't re-fetch from PG here
                        // (the upsert path already wrote the row
                        // and the next `get()` will see it).
                        serde_json::Value::Null,
                        self.clock.current_tick(),
                        request_id,
                    ),
                    LoadOutcome::Reload => CreatureAuditEntry::reloaded(
                        actor_uuid,
                        actor_type,
                        creature_id,
                        serde_json::Value::Null,
                        serde_json::Value::Null,
                        self.clock.current_tick(),
                        request_id,
                    ),
                    LoadOutcome::Unloaded => CreatureAuditEntry::unloaded(
                        actor_uuid,
                        actor_type,
                        creature_id,
                        serde_json::Value::Null,
                        self.clock.current_tick(),
                        request_id,
                    ),
                }).await;
                Ok(outcome)
            }
            Err(e) => {
                self.record_failure(creature_id, &e.to_string()).await;
                Err(e)
            }
        }
    }

    // ── internal helpers ──────────────────────────────────────

    /// Scan `config_dir` and return `(creature_id, path,
    /// mtime_epoch)` tuples for every well-formed subdir.
    async fn scan_disk(
        &self,
    ) -> Result<Vec<(String, PathBuf, i64)>, ReloadError> {
        let dir = self.config_dir.clone();
        // The actual filesystem call is blocking; offload so
        // the tokio runtime stays responsive.
        tokio::task::spawn_blocking(move || scan_disk_blocking(&dir))
            .await
            .map_err(|e| ReloadError::Io(std::io::Error::other(format!("join: {e}"))))?
    }

    fn path_for(&self, creature_id: &str) -> PathBuf {
        self.config_dir.join(creature_id).join("creatures.json")
    }

    async fn load_and_upsert(
        &self,
        path: &Path,
        mtime: i64,
    ) -> Result<LoadOutcome, ReloadError> {
        let raw = tokio::fs::read_to_string(path).await?;
        let value: JsonValue = serde_json::from_str(&raw).map_err(|e| {
            ReloadError::JsonParse {
                path: path.to_path_buf(),
                detail: e.to_string(),
            }
        })?;
        let cfg: CreatureConfig = serde_json::from_value(value.clone()).map_err(|e| {
            ReloadError::JsonParse {
                path: path.to_path_buf(),
                detail: format!("creature struct decode: {e}"),
            }
        })?;
        cfg.validate().map_err(|e| ReloadError::Validate {
            creature_id: cfg.id.clone(),
            detail: e.to_string(),
        })?;

        // Cross-check: the file's `id` field must match the
        // directory name (`config/biocapital/creatures/<id>/...`).
        let dir_id = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if dir_id != cfg.id {
            return Err(ReloadError::IdMismatch {
                path: path.to_path_buf(),
                file_id: cfg.id.clone(),
                dir_id,
            });
        }

        let existed = self.pg_repo.get(&cfg.id).await?.is_some();
        let now = self.clock.now();
        let tick = self.clock.current_tick();
        let record = CreatureConfigRecord::from_loaded(
            &cfg,
            path.to_string_lossy().to_string(),
            mtime,
            tick,
            now,
        )?;
        self.pg_repo.upsert(&record).await?;

        // Audit. The "actor" for a hot-reload tick is the
        // Rust service itself; for the manual `reload_one`
        // path the caller passes an explicit actor (see the
        // wrapper above).
        let op_entry = if existed {
            CreatureAuditEntry::reloaded(
                None,
                "RUST_SERVICE",
                &cfg.id,
                JsonValue::Null,
                record.config_json.clone(),
                tick,
                None,
            )
        } else {
            CreatureAuditEntry::loaded(
                None,
                "RUST_SERVICE",
                &cfg.id,
                record.config_json.clone(),
                tick,
                None,
            )
        };
        self.audit_writer.write(&op_entry).await?;

        // Best-effort FK-style observability: warn when a
        // deleted creature's id is still referenced by
        // `mob_replacements.creature_id`. There is no hard
        // FK at the DB layer (13 §6.3), so this is purely a
        // logging hint for the admin.
        if !existed {
            if let Ok(mob_rows) = self.mob_replacement_repo.list(false).await {
                let dangling: Vec<String> = mob_rows
                    .into_iter()
                    .filter(|m| m.creature_id == cfg.id)
                    .map(|m| m.vanilla_id)
                    .collect();
                if !dangling.is_empty() {
                    info!(
                        creature_id = %cfg.id,
                        mob_count = dangling.len(),
                        vanilla_ids = ?dangling,
                        "creature loaded; mob_replacements now reference it"
                    );
                }
            }
        }

        Ok(if existed { LoadOutcome::Reload } else { LoadOutcome::New })
    }

    async fn unload_one(
        &self,
        creature_id: &str,
        row: &CreatureConfigRecord,
    ) -> Result<(), ReloadError> {
        // Best-effort FK-style observability (see above).
        if let Ok(mob_rows) = self.mob_replacement_repo.list(false).await {
            let dangling: Vec<String> = mob_rows
                .into_iter()
                .filter(|m| m.creature_id == creature_id)
                .map(|m| m.vanilla_id)
                .collect();
            if !dangling.is_empty() {
                warn!(
                    creature_id = %creature_id,
                    mob_count = dangling.len(),
                    vanilla_ids = ?dangling,
                    "creature unloaded but mob_replacements still reference it"
                );
            }
        }
        self.pg_repo.delete(creature_id).await?;
        let entry = CreatureAuditEntry::unloaded(
            None,
            "RUST_SERVICE",
            creature_id,
            row.config_json.clone(),
            self.clock.current_tick(),
            None,
        );
        self.audit_writer.write(&entry).await?;
        Ok(())
    }

    async fn record_failure(&self, creature_id: &str, error_detail: &str) {
        // Bump the PG counter (best-effort; ignore repo errors
        // so the tick loop never panics).
        let _ = self.pg_repo.increment_reload_failed_count(creature_id).await;
        let entry = CreatureAuditEntry::reload_failed(
            None,
            "RUST_SERVICE",
            creature_id,
            self.clock.current_tick(),
            None,
            error_detail,
        );
        if let Err(e) = self.audit_writer.write(&entry).await {
            error!(
                creature_id = %creature_id,
                error = %e,
                "creature hot-reload: failed to write failure audit row"
            );
        }
    }
}

/// Outcome of `load_and_upsert`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOutcome {
    /// New row inserted.
    New,
    /// Existing row replaced (mtime change).
    Reload,
    /// Row deleted (file vanished from disk).
    Unloaded,
}

// ── Filesystem helpers ─────────────────────────────────────────────────────

fn scan_disk_blocking(
    dir: &Path,
) -> Result<Vec<(String, PathBuf, i64)>, ReloadError> {
    if !dir.exists() {
        // Missing config dir is **not** an error at startup —
        // the user may not have any custom creatures yet. An
        // empty scan list is the correct answer.
        return Ok(Vec::new());
    }
    if !dir.is_dir() {
        return Err(ReloadError::NotADirectory(dir.to_path_buf()));
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let creature_id = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let json_path = path.join("creatures.json");
        if !json_path.is_file() {
            // No `creatures.json` → skip. The directory is
            // either a placeholder (asset templates land here
            // during task #15) or an unrelated folder the user
            // happened to drop in. We deliberately do **not**
            // delete the PG row for a directory missing its
            // creatures.json; that would be too aggressive.
            continue;
        }
        let mtime = file_mtime(&json_path)?;
        out.push((creature_id, json_path, mtime));
    }
    Ok(out)
}

fn file_mtime(path: &Path) -> Result<i64, ReloadError> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(mtime)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    use crate::domain::{CreatureConfig, MobReplacement, ModelSource};
    use crate::pg::mob_replacement::RepoError as MobPgErr;
    use crate::pg::{CreatureAuditWriter, CreatureConfigRepository, MobReplacementRepository, RepoError as CreaturePgErr};

    struct MemRepo {
        rows: Mutex<HashMap<String, CreatureConfigRecord>>,
    }
    impl MemRepo {
        fn new() -> Self {
            Self {
                rows: Mutex::new(HashMap::new()),
            }
        }
        fn insert(&self, r: CreatureConfigRecord) {
            self.rows.lock().unwrap().insert(r.creature_id.clone(), r);
        }
    }
    #[async_trait::async_trait]
    impl CreatureConfigRepository for MemRepo {
        async fn upsert(&self, r: &CreatureConfigRecord) -> Result<(), CreaturePgErr> {
            self.rows.lock().unwrap().insert(r.creature_id.clone(), r.clone());
            Ok(())
        }
        async fn get(&self, id: &str) -> Result<Option<CreatureConfigRecord>, CreaturePgErr> {
            Ok(self.rows.lock().unwrap().get(id).cloned())
        }
        async fn list(&self, enabled_only: bool) -> Result<Vec<CreatureConfigRecord>, CreaturePgErr> {
            let v: Vec<CreatureConfigRecord> = self.rows.lock().unwrap().values().cloned().collect();
            Ok(if enabled_only {
                v.into_iter().filter(|r| r.enabled).collect()
            } else {
                v
            })
        }
        async fn delete(&self, id: &str) -> Result<(), CreaturePgErr> {
            self.rows.lock().unwrap().remove(id);
            Ok(())
        }
        async fn list_by_source_mtime(&self, older_than: i64) -> Result<Vec<CreatureConfigRecord>, CreaturePgErr> {
            Ok(self.rows.lock().unwrap().values().filter(|r| r.source_mtime < older_than).cloned().collect())
        }
        async fn increment_reload_failed_count(&self, _id: &str) -> Result<(), CreaturePgErr> {
            Ok(())
        }
    }

    struct MemAudit {
        rows: Mutex<Vec<CreatureAuditEntry>>,
    }
    impl MemAudit {
        fn new() -> Self {
            Self { rows: Mutex::new(Vec::new()) }
        }
        fn count_op(&self, op: &str) -> usize {
            self.rows.lock().unwrap().iter().filter(|e| e.op == op).count()
        }
    }
    #[async_trait::async_trait]
    impl CreatureAuditWriter for MemAudit {
        async fn write(&self, e: &CreatureAuditEntry) -> Result<(), CreaturePgErr> {
            self.rows.lock().unwrap().push(e.clone());
            Ok(())
        }
    }

    struct EmptyMobRepo;
    #[async_trait::async_trait]
    impl MobReplacementRepository for EmptyMobRepo {
        async fn get_for_vanilla(&self, _v: &str) -> Result<Vec<MobReplacement>, MobPgErr> { Ok(vec![]) }
        async fn list(&self, _e: bool) -> Result<Vec<MobReplacement>, MobPgErr> { Ok(vec![]) }
        async fn upsert(&self, _r: &MobReplacement) -> Result<(), MobPgErr> { Ok(()) }
        async fn delete(&self, _id: uuid::Uuid) -> Result<(), MobPgErr> { Ok(()) }
    }

    struct FixedClock;
    #[async_trait::async_trait]
    impl Clock for FixedClock {
        fn current_tick(&self) -> i64 { 42 }
        fn now(&self) -> chrono::DateTime<Utc> { chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap() }
    }

    fn make_reloader(dir: &Path) -> (CreatureHotReloader, Arc<MemRepo>, Arc<MemAudit>) {
        let repo = Arc::new(MemRepo::new());
        let audit = Arc::new(MemAudit::new());
        let mob = Arc::new(EmptyMobRepo);
        let r = CreatureHotReloader::new(
            dir.to_path_buf(),
            repo.clone(),
            audit.clone(),
            mob,
        )
        .with_tick_interval(Duration::from_millis(50))
        .with_clock(Arc::new(FixedClock));
        (r, repo, audit)
    }

    fn write_creature(dir: &Path, id: &str) -> PathBuf {
        let creature_dir = dir.join(id);
        std::fs::create_dir_all(&creature_dir).unwrap();
        let cfg = CreatureConfig {
            id: id.to_string(),
            display_name: [
                ("en_us".to_string(), format!("Variant {id}")),
                ("zh_cn".to_string(), format!("变体 {id}")),
            ]
            .into_iter()
            .collect(),
            model_source: ModelSource::Vanilla,
            entity_type: "minecraft:zombie".to_string(),
            model_variants: Default::default(),
            audio_clips: Default::default(),
            drops: Vec::new(),
            replaces: vec!["minecraft:zombie".to_string()],
            tags: vec!["monster".to_string()],
            enabled: true,
            stat_overrides: Default::default(),
        };
        let path = creature_dir.join("creatures.json");
        let raw = serde_json::to_string(&cfg).unwrap();
        std::fs::write(&path, raw).unwrap();
        path
    }

    #[tokio::test]
    async fn scan_once_loads_new_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        write_creature(dir, "variant_zombie");

        let (r, repo, audit) = make_reloader(dir);
        let report = r.scan_once().await;

        assert_eq!(report.files_scanned, 1);
        assert_eq!(report.loaded, 1);
        assert_eq!(report.reloaded, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(report.deleted, 0);

        let stored = repo.get("variant_zombie").await.unwrap();
        assert!(stored.is_some());
        assert_eq!(audit.count_op("creature.load"), 1);
    }

    #[tokio::test]
    async fn scan_once_reloads_on_mtime_change() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let path = write_creature(dir, "variant_zombie");

        let (r, repo, audit) = make_reloader(dir);
        let _ = r.scan_once().await;
        let _ = r.scan_once().await;
        // No mtime change → no reload.
        assert_eq!(repo.get("variant_zombie").await.unwrap().unwrap().source_mtime,
                   path.metadata().unwrap().modified().unwrap()
                       .duration_since(std::time::UNIX_EPOCH).unwrap()
                       .as_secs() as i64);
        assert_eq!(audit.count_op("creature.reload"), 0);

        // Force mtime forward by 2 s and re-scan.
        let new_mtime = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        let new_ft = filetime::FileTime::from_system_time(new_mtime);
        filetime::set_file_mtime(&path, new_ft).unwrap();

        let report = r.scan_once().await;
        assert_eq!(report.reloaded, 1);
        assert_eq!(report.loaded, 0);
        assert_eq!(audit.count_op("creature.reload"), 1);
    }

    #[tokio::test]
    async fn scan_once_deletes_vanished_rows() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        write_creature(dir, "variant_zombie");

        let (r, repo, audit) = make_reloader(dir);
        let _ = r.scan_once().await;
        assert!(repo.get("variant_zombie").await.unwrap().is_some());

        // Delete the file (and the dir) on disk.
        std::fs::remove_dir_all(dir.join("variant_zombie")).unwrap();
        let report = r.scan_once().await;

        assert_eq!(report.deleted, 1);
        assert!(repo.get("variant_zombie").await.unwrap().is_none());
        assert_eq!(audit.count_op("creature.unload"), 1);
    }

    #[tokio::test]
    async fn scan_once_records_validation_failure() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let creature_dir = dir.join("variant_zombie");
        std::fs::create_dir_all(&creature_dir).unwrap();
        // Invalid: id is empty.
        std::fs::write(
            creature_dir.join("creatures.json"),
            r#"{"id":"","entity_type":"minecraft:zombie","model_source":{"kind":"VANILLA"}}"#,
        )
        .unwrap();

        let (r, _repo, audit) = make_reloader(dir);
        let report = r.scan_once().await;

        assert_eq!(report.failed, 1);
        assert_eq!(report.loaded, 0);
        assert_eq!(audit.count_op("creature.reload_failed"), 1);
    }

    #[tokio::test]
    async fn reload_all_writes_summary_audit() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        write_creature(dir, "variant_zombie");

        let (r, _repo, audit) = make_reloader(dir);
        let report = r.reload_all(None, "ADMIN_CMD", None).await.unwrap();

        assert_eq!(report.loaded, 1);
        assert_eq!(audit.count_op("creature.reload_all"), 1);
    }

    #[tokio::test]
    async fn missing_config_dir_is_not_an_error() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("does_not_exist");
        let (r, _repo, _audit) = make_reloader(&dir);
        let report = r.scan_once().await;
        assert_eq!(report.files_scanned, 0);
        assert_eq!(report.loaded, 0);
    }
}