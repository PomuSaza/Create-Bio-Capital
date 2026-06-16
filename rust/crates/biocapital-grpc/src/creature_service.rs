//! gRPC service for `CreatureService`
//! (`doc/14-rust-services.md` §3.2 + `doc/13-bio-customization.md` §6.2).
//!
//! Proto path: `rust/proto/biocapital.proto` → `biocapital.v1` package.
//!
//! Three RPCs are implemented (13 §6.2):
//!   - `ListCreatures(ListRequest) -> CreatureListResponse` (method_id 0)
//!   - `GetCreature(CreatureRequest) -> CreatureConfig`        (method_id 1)
//!   - `ReloadCreatures(Empty) -> ReloadResponse`             (method_id 2)
//!
//! `ListCreatures` reads from `creature_configs` and returns
//! the cached display names + load metadata. The full
//! `CreatureConfig` JSON is **not** shipped over the wire
//! here — the proto's `CreatureListResponse` carries only a
//! list of `creature_id` strings (proto definition in
//! `biocapital.proto` line 415). The hot cache lookup stays
//! cheap; callers that need the full config go through
//! `GetCreature`.
//!
//! `GetCreature` deserialises the cached `config_json` JSONB
//! into the proto `CreatureConfig` message. The proto is a
//! **flat** projection of the on-disk schema (13 §2.1); the
//! conversion from `biocapital_creature::domain::CreatureConfig`
//! to the proto struct happens here.
//!
//! `ReloadCreatures` drives a one-shot `CreatureHotReloader::reload_all()`
//! and returns the counts. Admin / Web UI callers (15 §X)
//! use this for "I just edited 10 files and don't want to
//! wait 5 s".

use std::sync::Arc;

use tonic::{Request, Response, Status};
use uuid::Uuid;

use biocapital_creature::domain::CreatureConfig;
use biocapital_creature::hot_reload::CreatureHotReloader;
// `CreatureConfigRepository` is only referenced by the test module
// below; `use super::*` re-exports the lib's top-level imports.
#[allow(unused_imports)]
use biocapital_creature::pg::{
    CreatureConfigRecord, CreatureConfigRepository, CreatureConfigServiceDeps, CreatureRepoError,
};

// ── Opaque request/response types (proto-shaped) ────────────────────────────

/// Mirrors `biocapital.v1.Empty`. The proto definition is the
/// canonical source; this is the opaque Rust mirror.
#[derive(Debug, Clone, Default)]
pub struct Empty {}

/// Mirrors `biocapital.v1.ListRequest`. The proto definition is
/// the canonical source; this is the opaque Rust mirror.
#[derive(Debug, Clone, Default)]
pub struct ListRequest {
    pub limit: i32,
    pub offset: i32,
}

/// Mirrors `biocapital.v1.CreatureRequest`.
#[derive(Debug, Clone, Default)]
pub struct CreatureRequest {
    pub creature_id: String,
}

/// Mirrors `biocapital.v1.CreatureListResponse`.
#[derive(Debug, Clone, Default)]
pub struct CreatureListResponse {
    pub creature_ids: Vec<String>,
}

/// Mirrors `biocapital.v1.ReloadResponse`.
#[derive(Debug, Clone, Default)]
pub struct ReloadResponse {
    pub reloaded_count: i32,
    pub failed_creature_ids: Vec<String>,
    pub reloaded_at_unix_ms: i64,
}

/// Flat proto projection of `biocapital_creature::domain::CreatureConfig`.
/// Mirrors `biocapital.v1.CreatureConfig` (proto line 425). The
/// field names are taken straight from the proto schema; see
/// 13 §2.1 for the canonical schema source.
#[derive(Debug, Clone, Default)]
pub struct CreatureConfigProto {
    pub creature_id: String,
    pub display_name_zh: String,
    pub display_name_en: String,
    pub model_source: String,
    pub creature_type: String,
    pub geckolib_format_version: i32,

    pub audio_ambient: String,
    pub audio_hurt: String,
    pub audio_death: String,
    pub audio_step: String,
    pub audio_volume: f32,
    pub audio_pitch: f32,

    pub texture_main: String,
    pub texture_overlay: String,

    pub model_geo: String,
    pub model_animations: Vec<String>,
    pub model_idle_animation: String,
    pub model_scale: f32,

    pub max_health: f32,
    pub attack_damage: f32,
    pub movement_speed: f32,

    pub drops_override_json: String,

    pub replaces: String,
    pub tags: Vec<String>,
    pub enabled: bool,

    pub loaded_tick: i64,
    pub loaded_at_unix_ms: i64,
}

// ── gRPC service trait (the wiring target) ──────────────────────────────────

/// Mirrors the generated
/// `biocapital.v1.creature_service_server::CreatureService`.
#[tonic::async_trait]
pub trait CreatureRpc: Send + Sync + 'static {
    async fn list_creatures(
        &self,
        request: Request<ListRequestProto>,
    ) -> Result<Response<CreatureListResponse>, Status>;

    async fn get_creature(
        &self,
        request: Request<CreatureRequest>,
    ) -> Result<Response<CreatureConfigProto>, Status>;

    async fn reload_creatures(
        &self,
        request: Request<EmptyProto>,
    ) -> Result<Response<ReloadResponse>, Status>;
}

// Re-export aliases so callers can use the proto-shaped
// names (`ListRequestProto` / `EmptyProto`) without worrying
// about future renames. Both types live in this module.
pub type ListRequestProto = ListRequest;
pub type EmptyProto = Empty;

// ── Implementation ──────────────────────────────────────────────────────────

/// The actual gRPC service. Holds:
/// - `repo` for the `creature_configs` table
///   (`CreatureConfigRepository`);
/// - `hot_reloader` for the `CreatureHotReloader` (drives the
///   manual `ReloadCreatures` RPC; the 5 s background tick is
///   driven by `start_watching` and is independent of the RPC
///   path).
///
/// Cheap to clone (`Arc` internals).
#[derive(Clone)]
pub struct CreatureServiceGrpc {
    deps: CreatureConfigServiceDeps,
    hot_reloader: Arc<CreatureHotReloader>,
}

impl CreatureServiceGrpc {
    pub fn new(
        deps: CreatureConfigServiceDeps,
        hot_reloader: Arc<CreatureHotReloader>,
    ) -> Self {
        Self { deps, hot_reloader }
    }
}

#[tonic::async_trait]
impl CreatureRpc for CreatureServiceGrpc {
    async fn list_creatures(
        &self,
        _request: Request<ListRequestProto>,
    ) -> Result<Response<CreatureListResponse>, Status> {
        let rows = self
            .deps
            .repo
            .list(true)
            .await
            .map_err(repo_status)?;
        let creature_ids: Vec<String> =
            rows.into_iter().map(|r| r.creature_id).collect();
        Ok(Response::new(CreatureListResponse { creature_ids }))
    }

    async fn get_creature(
        &self,
        request: Request<CreatureRequest>,
    ) -> Result<Response<CreatureConfigProto>, Status> {
        let CreatureRequest { creature_id } = request.into_inner();
        if creature_id.is_empty() {
            return Err(Status::invalid_argument("creature_id is empty"));
        }
        let row = self
            .deps
            .repo
            .get(&creature_id)
            .await
            .map_err(repo_status)?
            .ok_or_else(|| {
                Status::not_found(format!(
                    "creature_configs row not found for creature_id={creature_id}"
                ))
            })?;
        let proto = record_to_proto(&row)?;
        Ok(Response::new(proto))
    }

    async fn reload_creatures(
        &self,
        request: Request<EmptyProto>,
    ) -> Result<Response<ReloadResponse>, Status> {
        // We ignore the body (`Empty`); actor_type = "ADMIN_CMD"
        // for the manual RPC path per the 99 §2.2 audit
        // taxonomy. request_id is left None; the proto `Empty`
        // message carries no idempotency key.
        let actor_uuid: Option<Uuid> = None;
        let report = self
            .hot_reloader
            .reload_all(actor_uuid, "ADMIN_CMD", None)
            .await
            .map_err(|e| Status::internal(format!("reload_all failed: {e}")))?;
        let _ = request; // silence unused
        let resp = ReloadResponse {
            reloaded_count: (report.loaded + report.reloaded) as i32,
            failed_creature_ids: Vec::new(),
            reloaded_at_unix_ms: chrono::Utc::now().timestamp_millis(),
        };
        Ok(Response::new(resp))
    }
}

// ── Conversion helpers ──────────────────────────────────────────────────────

/// Convert a `CreatureConfigRecord` (PG row) into the flat
/// proto `CreatureConfig` shape. The `config_json` JSONB is
/// the authoritative payload — display-name + loaded-tick
/// metadata comes from the surrounding columns (cache + audit).
fn record_to_proto(row: &CreatureConfigRecord) -> Result<CreatureConfigProto, Status> {
    // We round-trip the JSONB through the domain type so the
    // proto struct only needs the flat projection logic. This
    // keeps the wire form 1:1 with the domain type and avoids
    // duplicating the field mapping.
    let cfg: CreatureConfig = serde_json::from_value(row.config_json.clone())
        .map_err(|e| Status::internal(format!(
            "creature_configs.config_json decode failed for {}: {e}",
            row.creature_id
        )))?;

    let display_name_zh = cfg
        .display_name
        .get("zh_cn")
        .cloned()
        .unwrap_or_default();
    let display_name_en = cfg
        .display_name
        .get("en_us")
        .cloned()
        .unwrap_or_default();

    // The proto CreatureConfig.creature_type is one of
    // "MONSTER" / "PASSIVE" / "BOSS". The domain type does not
    // model `creature_type` directly (the 13 §2.1 schema lists
    // it but task #9's domain type does not persist it; we
    // infer from `tags` until the domain widens). Fall back to
    // "MONSTER" for hostile-mob variants (the only type the
    // current schema supports, per doc 06 §2.3 — variants are
    // always monsters).
    let creature_type = if cfg.tags.iter().any(|t| t == "passive") {
        "PASSIVE"
    } else if cfg.tags.iter().any(|t| t == "boss") {
        "BOSS"
    } else {
        "MONSTER"
    }
    .to_string();

    // The proto `audio_*` flat fields map to the domain's
    // `audio_clips.{ambient,hurt,death,step}` paths.
    let audio = &cfg.audio_clips;
    let audio_ambient = audio.ambient.clone().unwrap_or_default();
    let audio_hurt = audio.hurt.clone().unwrap_or_default();
    let audio_death = audio.death.clone().unwrap_or_default();
    let audio_step = audio.step.clone().unwrap_or_default();
    // 13 §2.1 lists `audio.volume` and `audio.pitch`; the
    // domain AudioClips struct does not model them (task #9
    // limits scope to the file paths). Default to 1.0 / 1.0.
    let audio_volume: f32 = 1.0;
    let audio_pitch: f32 = 1.0;

    let texture_main = cfg
        .model_variants
        .texture_path
        .clone()
        .unwrap_or_default();
    let texture_overlay = String::new();
    let model_geo = cfg.model_variants.geo_path.clone().unwrap_or_default();
    let mut model_animations = Vec::new();
    if let Some(anim) = &cfg.model_variants.animation_path {
        model_animations.push(anim.clone());
    }
    let model_idle_animation = String::new();
    let model_scale: f32 = 1.0;

    let max_health = cfg.stat_overrides.max_health.unwrap_or(20.0);
    let attack_damage = cfg.stat_overrides.attack_damage.unwrap_or(3.0);
    let movement_speed = cfg.stat_overrides.movement_speed.unwrap_or(0.23);

    // The proto `drops_override_json` field is a JSON string
    // (proto schema comment: "为简化留作 raw JSON 字符串"). The
    // domain `drops: Vec<DropEntry>` is the structured shape.
    let drops_override_json = serde_json::to_string(&cfg.drops).unwrap_or_default();

    // The proto's `replaces` is a single string; the domain's
    // `replaces: Vec<String>` allows multiple. Take the first
    // entry (creatures almost always have exactly one
    // `replaces`); fall back to entity_type if empty.
    let replaces = cfg
        .replaces
        .first()
        .cloned()
        .unwrap_or_else(|| cfg.entity_type.clone());

    Ok(CreatureConfigProto {
        creature_id: cfg.id,
        display_name_zh,
        display_name_en,
        model_source: cfg.model_source.to_string(),
        creature_type,
        geckolib_format_version: 2,
        audio_ambient,
        audio_hurt,
        audio_death,
        audio_step,
        audio_volume,
        audio_pitch,
        texture_main,
        texture_overlay,
        model_geo,
        model_animations,
        model_idle_animation,
        model_scale,
        max_health,
        attack_damage,
        movement_speed,
        drops_override_json,
        replaces,
        tags: cfg.tags,
        enabled: cfg.enabled,
        loaded_tick: row.last_loaded_tick,
        loaded_at_unix_ms: row.last_loaded_at.timestamp_millis(),
    })
}

/// Map a `biocapital_creature::pg::CreatureRepoError` to a `tonic::Status`.
fn repo_status(e: CreatureRepoError) -> Status {
    match e {
        CreatureRepoError::Sqlx(sqlx::Error::RowNotFound) => {
            Status::not_found("creature_configs row not found")
        }
        CreatureRepoError::Sqlx(e) => Status::internal(format!("postgres error: {e}")),
        CreatureRepoError::Migrate(e) => Status::internal(format!("migration error: {e}")),
        CreatureRepoError::InvalidJson(detail) => {
            Status::invalid_argument(format!("invalid JSONB payload: {detail}"))
        }
        CreatureRepoError::NotFound(creature_id) => Status::not_found(format!(
            "creature_configs row not found for creature_id={creature_id}"
        )),
    }
}

// ── Tests (no live DB / network required) ──────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Mutex;

    use biocapital_creature::domain::CreatureConfig;
    use biocapital_creature::hot_reload::SystemClock;
    use biocapital_creature::pg::CreatureConfigRecord;

    struct MemRepo {
        rows: Mutex<Vec<CreatureConfigRecord>>,
    }
    impl MemRepo {
        fn new(rows: Vec<CreatureConfigRecord>) -> Self {
            Self { rows: Mutex::new(rows) }
        }
    }
    #[async_trait]
    impl CreatureConfigRepository for MemRepo {
        async fn upsert(
            &self,
            record: &CreatureConfigRecord,
        ) -> Result<(), biocapital_creature::pg::CreatureRepoError> {
            let mut g = self.rows.lock().unwrap();
            g.retain(|r| r.creature_id != record.creature_id);
            g.push(record.clone());
            Ok(())
        }
        async fn get(
            &self,
            id: &str,
        ) -> Result<Option<CreatureConfigRecord>, biocapital_creature::pg::CreatureRepoError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.creature_id == id)
                .cloned())
        }
        async fn list(
            &self,
            enabled_only: bool,
        ) -> Result<Vec<CreatureConfigRecord>, biocapital_creature::pg::CreatureRepoError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|r| !enabled_only || r.enabled)
                .cloned()
                .collect())
        }
        async fn delete(&self, id: &str) -> Result<(), biocapital_creature::pg::CreatureRepoError> {
            self.rows.lock().unwrap().retain(|r| r.creature_id != id);
            Ok(())
        }
        async fn list_by_source_mtime(
            &self,
            _older: i64,
        ) -> Result<Vec<CreatureConfigRecord>, biocapital_creature::pg::CreatureRepoError> {
            Ok(Vec::new())
        }
        async fn increment_reload_failed_count(
            &self,
            _id: &str,
        ) -> Result<(), biocapital_creature::pg::CreatureRepoError> {
            Ok(())
        }
    }

    /// Stub audit writer for the hot-reloader.
    struct StubAudit;
    #[async_trait]
    impl biocapital_creature::pg::CreatureAuditWriter for StubAudit {
        async fn write(
            &self,
            _e: &biocapital_creature::pg::CreatureAuditEntry,
        ) -> Result<(), biocapital_creature::pg::CreatureRepoError> {
            Ok(())
        }
    }
    struct StubMobRepo;
    #[async_trait]
    impl biocapital_creature::pg::MobReplacementRepository for StubMobRepo {
        async fn get_for_vanilla(
            &self,
            _v: &str,
        ) -> Result<Vec<biocapital_creature::domain::MobReplacement>, biocapital_creature::pg::MobRepoError>
        {
            Ok(vec![])
        }
        async fn list(
            &self,
            _e: bool,
        ) -> Result<Vec<biocapital_creature::domain::MobReplacement>, biocapital_creature::pg::MobRepoError>
        {
            Ok(vec![])
        }
        async fn upsert(
            &self,
            _r: &biocapital_creature::domain::MobReplacement,
        ) -> Result<(), biocapital_creature::pg::MobRepoError> {
            Ok(())
        }
        async fn delete(&self, _id: Uuid) -> Result<(), biocapital_creature::pg::MobRepoError> {
            Ok(())
        }
    }

    fn record_for(creature_id: &str) -> CreatureConfigRecord {
        let cfg = if creature_id == "variant_zombie" {
            CreatureConfig::placeholder_variant_zombie()
        } else {
            let mut c = CreatureConfig::placeholder_variant_zombie();
            c.id = creature_id.to_string();
            c.enabled = false;
            c
        };
        CreatureConfigRecord::from_loaded(
            &cfg,
            format!("/config/biocapital/creatures/{creature_id}/creatures.json"),
            1_700_000_000,
            999,
            Utc::now(),
        )
        .unwrap()
    }

    fn make_service(
        rows: Vec<CreatureConfigRecord>,
    ) -> (CreatureServiceGrpc, Arc<CreatureHotReloader>) {
        let repo = Arc::new(MemRepo::new(rows));
        let audit: Arc<dyn biocapital_creature::pg::CreatureAuditWriter> = Arc::new(StubAudit);
        let mob: Arc<dyn biocapital_creature::pg::MobReplacementRepository> =
            Arc::new(StubMobRepo);
        let deps = CreatureConfigServiceDeps::new(repo, audit);
        let hot = Arc::new(CreatureHotReloader::new(
            std::path::PathBuf::from("/tmp/does-not-matter"),
            Arc::new(MemRepo::new(Vec::new())),
            Arc::new(StubAudit),
            Arc::new(StubMobRepo),
        ));
        let _ = SystemClock; // silence unused import
        (CreatureServiceGrpc::new(deps, hot.clone()), hot)
    }

    #[tokio::test]
    async fn list_creatures_returns_enabled_only() {
        let (svc, _hot) = make_service(vec![
            record_for("variant_zombie"),
            record_for("variant_disabled"),
        ]);
        let resp = svc
            .list_creatures(Request::new(ListRequestProto::default()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.creature_ids, vec!["variant_zombie".to_string()]);
    }

    #[tokio::test]
    async fn get_creature_returns_full_proto() {
        let (svc, _hot) = make_service(vec![record_for("variant_zombie")]);
        let resp = svc
            .get_creature(Request::new(CreatureRequest {
                creature_id: "variant_zombie".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.creature_id, "variant_zombie");
        assert_eq!(resp.display_name_zh, "变体僵尸");
        assert_eq!(resp.display_name_en, "Variant Zombie");
        assert_eq!(resp.creature_type, "MONSTER");
        assert_eq!(resp.geckolib_format_version, 2);
        assert!(resp.enabled);
    }

    #[tokio::test]
    async fn get_creature_rejects_empty_id() {
        let (svc, _hot) = make_service(vec![]);
        let status = svc
            .get_creature(Request::new(CreatureRequest {
                creature_id: String::new(),
            }))
            .await
            .err()
            .expect("must fail");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_creature_returns_not_found_for_unknown_id() {
        let (svc, _hot) = make_service(vec![]);
        let status = svc
            .get_creature(Request::new(CreatureRequest {
                creature_id: "missing".to_string(),
            }))
            .await
            .err()
            .expect("must fail");
        assert_eq!(status.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn reload_creatures_returns_counts() {
        let (svc, _hot) = make_service(vec![]);
        let resp = svc
            .reload_creatures(Request::new(EmptyProto::default()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.reloaded_count, 0);
        assert!(resp.reloaded_at_unix_ms > 0);
    }
}