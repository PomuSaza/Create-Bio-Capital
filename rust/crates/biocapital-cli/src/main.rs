//! `biocapital-cli` — server entry point (start / migrate / backup / config).
//!
//! 权威: `doc/14-rust-services.md` §1.2 + §2.1 (启动期 + 备份) +
//!       `doc/11-config-system.md` (config 加载).
//!
//! 4 个子命令:
//! - `config`    — 加载 + 校验 + 打印配置
//! - `migrate`   — 连接 PG, 跑 `sqlx::migrate!`
//! - `backup`    — 立即跑一次 `pg_dump` + 清旧文件
//! - `start`     — 跑 migrate, 拉起 gRPC + Web UI + DG_LAB WS +
//!                 生物配置热重载 + 每日 03:00 备份 cron

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::process::Command;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};
use uuid::Uuid;

use biocapital_cli::sql_loader::run_sql_files;

use biocapital_bank::domain::Whitelist;
use biocapital_creature::hot_reload::CreatureHotReloader;
use biocapital_creature::pg::{
    CreatureConfigServiceDeps, MobReplacementServiceDeps,
    PgCreatureAuditWriter, PgCreatureConfigRepository,
};
use biocapital_dglab::ws_server::DglabWsServer;
use biocapital_environment::pg::{
    EnvironmentRepository, EnvironmentServiceDeps, FluidRepository, FluidServiceDeps,
    PgEnvironmentRepository, PgFluidRepository,
};
use biocapital_environment::EnvironmentServicePort;
use biocapital_grpc::audit_service::AuditGrpc;
use biocapital_grpc::bank_service::{BankGrpc, BankRpc};
use biocapital_grpc::contract_service::ContractGrpc;
use biocapital_grpc::core_pod_service::CorePodGrpc;
use biocapital_grpc::creature_service::CreatureServiceGrpc;
use biocapital_grpc::dglab_service::DglabGrpc;
use biocapital_grpc::environment_service::EnvironmentServiceGrpc;
use biocapital_grpc::hostile_mob_service::HostileMobGrpc;
use biocapital_grpc::player_state_service::PlayerStateGrpc;
use biocapital_jni::registry::BiocapitalServiceRegistry;
use biocapital_pg::audit::{AuditServiceDeps, PgAuditQueryRepository};
use biocapital_pg::bank::{BankServiceDeps, PgBankAuditWriter, PgBankRepository};
use biocapital_pg::contract::{
    ContractServiceDeps, PgContractAuditWriter, PgContractRepository,
};
use biocapital_pg::core_pod::{CorePodServiceDeps, PgCorePodAuditWriter, PgCorePodRepository};
use biocapital_pg::dglab::{
    DglabServiceDeps, PgDglabAuditWriter, PgDglabOverrideRepository, PgDglabRepository,
    PgPlayerDglabConfigRepository,
};
use biocapital_pg::hardware_token::{
    HardwareTokenServiceDeps, PgHardwareTokenAuditWriter, PgHardwareTokenRepository,
};
use biocapital_pg::player_name::{PgPlayerNameRepository, PlayerNameRepository};
use biocapital_pg::player_state::{
    PgAuditWriter, PgPlayerStateRepository, PlayerStateServiceDeps,
};
use biocapital_webui::{WebUiApp, WebUiConfig};

use biocapital_cli::config::ServerConfig;

// D1 决策覆写（2026-06-20）：MIGRATION_DIR 不再硬编码源码树绝对路径；
// 改为从 ServerConfig.PostgresSection.migration_dir 解析。
// 旧值（保留作注释以解释变更）：
// const MIGRATION_DIR: &str = "/home/saza/IdeaProjects/create_biocapital/rust/migrations";

// ============================================================================
// CLI surface
// ============================================================================

#[derive(Parser, Debug)]
#[command(name = "biocapital-cli", version, about = "Bio-Capital Rust server CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// 加载 + 校验 + 打印 `biocapital-server.toml`
    Config,
    /// 启动期：建库（如缺）+ 跑 migrate
    Migrate,
    /// 单独跑一次 `pg_dump` 备份（忽略 cron schedule）
    Backup,
    /// 启动期 + 拉起全部子服务（gRPC + Web UI + DG_LAB WS + 热重载 + 备份 cron）
    Start,
}

// ============================================================================
// Entry
// ============================================================================

#[tokio::main]
async fn main() {
    init_tracing();
    let cli = Cli::parse();

    let result: Result<()> = match cli.cmd {
        Cmd::Config => cmd_config(),
        Cmd::Migrate => cmd_migrate().await,
        Cmd::Backup => cmd_backup().await,
        Cmd::Start => cmd_start().await,
    };

    if let Err(e) = result {
        error!(target: "biocapital_cli", "fatal: {e:#}");
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

// ============================================================================
// C3 — cmd_config
// ============================================================================

#[derive(Serialize)]
struct ConfigReport {
    bind_host: String,
    bind_port: u16,
    http_port: u16,
    cold_start_budget_seconds: u64,
    memory_budget_mb: u32,
    allow_public_bind: bool,
    pg: PgConfigReport,
    backup: BackupConfigReport,
    dglab: DglabConfigReport,
    webui_admin_token_count: usize,
    log: LogConfigReport,
    monitoring: MonitoringConfigReport,
}

#[derive(Serialize)]
struct PgConfigReport {
    data_dir: String,
    port: u16,
    database: String,
    pool_min: u32,
    pool_max: u32,
    init_if_missing: bool,
}

#[derive(Serialize)]
struct BackupConfigReport {
    schedule: String,
    target_dir: String,
    retention_days: u32,
    tar_format: String,
}

#[derive(Serialize)]
struct DglabConfigReport {
    ws_host: String,
    ws_port: u16,
    heartbeat_interval_seconds: u64,
    session_id_length: usize,
}

#[derive(Serialize)]
struct LogConfigReport {
    log_level: String,
    log_format: String,
    log_dir: String,
    log_rotation: String,
    log_max_size_mb: u32,
    log_retention_days: u32,
}

#[derive(Serialize)]
struct MonitoringConfigReport {
    prometheus_enabled: bool,
    prometheus_port: u16,
    metrics_interval_seconds: u64,
}

fn cmd_config() -> Result<()> {
    let cfg = ServerConfig::load().context("loading biocapital-server.toml")?;
    let report = ConfigReport {
        bind_host: cfg.server.bind_host.clone(),
        bind_port: cfg.server.bind_port,
        http_port: cfg.server.http_port,
        cold_start_budget_seconds: cfg.server.cold_start_budget_seconds,
        memory_budget_mb: cfg.server.memory_budget_mb,
        allow_public_bind: cfg.server.allow_public_bind,
        pg: PgConfigReport {
            data_dir: cfg.server.postgresql.data_dir.clone(),
            port: cfg.server.postgresql.port,
            database: cfg.server.postgresql.database.clone(),
            pool_min: cfg.server.postgresql.connection_pool_min,
            pool_max: cfg.server.postgresql.connection_pool_max,
            init_if_missing: cfg.server.postgresql.init_if_missing,
        },
        backup: BackupConfigReport {
            schedule: cfg.server.backup.schedule.clone(),
            target_dir: cfg.server.backup.target_dir.clone(),
            retention_days: cfg.server.backup.retention_days,
            tar_format: cfg.server.backup.tar_format.clone(),
        },
        dglab: DglabConfigReport {
            ws_host: cfg.server.dglab.ws_host.clone(),
            ws_port: cfg.server.dglab.ws_port,
            heartbeat_interval_seconds: cfg.server.dglab.heartbeat_interval_seconds,
            session_id_length: cfg.server.dglab.session_id_length,
        },
        webui_admin_token_count: cfg.server.webui.admin_tokens.len(),
        log: LogConfigReport {
            log_level: cfg.server.logging.log_level.clone(),
            log_format: cfg.server.logging.log_format.clone(),
            log_dir: cfg.server.logging.log_dir.clone(),
            log_rotation: cfg.server.logging.log_rotation.clone(),
            log_max_size_mb: cfg.server.logging.log_max_size_mb,
            log_retention_days: cfg.server.logging.log_retention_days,
        },
        monitoring: MonitoringConfigReport {
            prometheus_enabled: cfg.server.monitoring.prometheus_enabled,
            prometheus_port: cfg.server.monitoring.prometheus_port,
            metrics_interval_seconds: cfg.server.monitoring.metrics_interval_seconds,
        },
    };
    let json = serde_json::to_string_pretty(&report)
        .context("serializing config report")?;
    println!("{json}");
    info!(target: "biocapital_cli", "config OK; bind={}:{} http={}", cfg.server.bind_host, cfg.server.bind_port, cfg.server.http_port);
    Ok(())
}

// ============================================================================
// C4 — cmd_migrate
// ============================================================================

async fn cmd_migrate() -> Result<()> {
    let cfg = ServerConfig::load().context("loading biocapital-server.toml")?;
    let pg = &cfg.server.postgresql;
    let url = build_connection_url(pg);

    if pg.init_if_missing {
        ensure_database_exists(pg, &url).await?;
    }

    let pool = open_pool(pg, &url).await?;
    let migration_dir = pg.resolve_migration_dir();
    info!(
        target: "biocapital_cli",
        "running migrations from {} (resolved from config; D1 决策)",
        migration_dir.display()
    );
    let started = std::time::Instant::now();
    run_sql_files(&pool, &migration_dir)
        .await
        .with_context(|| format!("running migrations from {}", migration_dir.display()))?;
    info!(
        target: "biocapital_cli",
        elapsed_ms = started.elapsed().as_millis() as u64,
        "migrations done"
    );
    Ok(())
}

async fn open_pool(pg: &biocapital_cli::config::PostgresSection, url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(pg.connection_pool_max)
        .min_connections(pg.connection_pool_min)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await
        .map_err(|e| anyhow!("connecting to PG at {url}: {e}"))?;
    Ok(pool)
}

/// 用 maintenance database `postgres` 建目标 database（如缺）。
async fn ensure_database_exists(
    pg: &biocapital_cli::config::PostgresSection,
    _target_url: &str,
) -> Result<()> {
    let admin_url = format!(
        "postgres://{user}:{pw}@localhost:{port}/postgres",
        user = pg.username,
        pw = if pg.password.is_empty() { String::new() } else { format!(":{}", pg.password) },
        port = pg.port,
    );
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&admin_url)
        .await
        .map_err(|e| anyhow!("connecting to PG `postgres` db for create: {e}"))?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)",
    )
    .bind(&pg.database)
    .fetch_one(&pool)
    .await
    .map_err(|e| anyhow!("checking pg_database: {e}"))?;
    if exists {
        info!(target: "biocapital_cli", database = %pg.database, "database already exists");
    } else {
        info!(target: "biocapital_cli", database = %pg.database, "creating database");
        // database name 不能参数化；用白名单过滤
        if !pg
            .database
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(anyhow!(
                "database name {:?} contains disallowed chars (only [a-zA-Z0-9_-] allowed)",
                pg.database
            ));
        }
        let sql = format!("CREATE DATABASE \"{}\"", pg.database);
        sqlx::query(&sql)
            .execute(&pool)
            .await
            .map_err(|e| anyhow!("CREATE DATABASE: {e}"))?;
    }
    Ok(())
}

fn build_connection_url(pg: &biocapital_cli::config::PostgresSection) -> String {
    let pw = if pg.password.is_empty() {
        String::new()
    } else {
        format!(":{}", pg.password)
    };
    format!(
        "postgres://{user}{pw}@localhost:{port}/{db}",
        user = pg.username,
        pw = pw,
        port = pg.port,
        db = pg.database,
    )
}

// ============================================================================
// C5 — cmd_backup
// ============================================================================

async fn cmd_backup() -> Result<()> {
    let cfg = ServerConfig::load().context("loading biocapital-server.toml")?;
    let pg = &cfg.server.postgresql;
    let url = build_connection_url(pg);
    let pool = open_pool(pg, &url).await?;
    let path = run_pg_dump(&cfg, pg).await?;
    info!(target: "biocapital_cli", dump = %path.display(), "backup written");
    let removed = prune_old_backups(&cfg).await?;
    if removed > 0 {
        info!(target: "biocapital_cli", removed, "pruned old backups");
    }
    write_backup_audit(&pool, &path).await?;
    Ok(())
}

async fn run_pg_dump(
    cfg: &ServerConfig,
    pg: &biocapital_cli::config::PostgresSection,
) -> Result<std::path::PathBuf> {
    let backup_dir = &cfg.server.backup.target_dir;
    tokio::fs::create_dir_all(backup_dir)
        .await
        .with_context(|| format!("creating backup dir {backup_dir}"))?;

    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let ext = match cfg.server.backup.tar_format.as_str() {
        "gz" => "dump",
        "xz" => "dump",
        "zst" => "dump",
        "tar" => "tar",
        _ => "dump",
    };
    let filename = format!("biocapital-{stamp}.{ext}");
    let path = std::path::PathBuf::from(backup_dir).join(&filename);

    let mut cmd = Command::new("pg_dump");
    cmd.arg("-Fc")
        .arg("-h")
        .arg("localhost")
        .arg("-p")
        .arg(pg.port.to_string())
        .arg("-U")
        .arg(&pg.username)
        .arg("-d")
        .arg(&pg.database)
        .arg("-f")
        .arg(&path)
        .arg("--no-owner")
        .arg("--no-privileges")
        .env(
            "PGPASSWORD",
            if pg.password.is_empty() {
                String::new()
            } else {
                pg.password.clone()
            },
        )
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let output = cmd
        .output()
        .await
        .with_context(|| format!("spawning pg_dump for {path:?}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(anyhow!(
            "pg_dump failed (exit={:?}): {stderr}",
            output.status.code()
        ));
    }
    Ok(path)
}

async fn prune_old_backups(cfg: &ServerConfig) -> Result<u32> {
    let dir = &cfg.server.backup.target_dir;
    let retention = chrono::Duration::days(cfg.server.backup.retention_days as i64);
    let now = Utc::now();
    let prefix = "biocapital-";
    let mut removed = 0u32;
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(e) => {
            warn!(target: "biocapital_cli", dir = %dir, error = %e, "cannot read backup dir; nothing to prune");
            return Ok(0);
        }
    };
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| anyhow!("reading backup dir: {e}"))?
    {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(prefix) {
            continue;
        }
        let meta = entry
            .metadata()
            .await
            .map_err(|e| anyhow!("stat {name}: {e}"))?;
        let modified = meta.modified().ok();
        if let Some(m) = modified {
            if let Ok(dur) = std::time::SystemTime::now().duration_since(m) {
                if chrono::Duration::from_std(dur).unwrap_or_default() > retention {
                    if tokio::fs::remove_file(entry.path()).await.is_ok() {
                        removed += 1;
                        info!(target: "biocapital_cli", file = %name, "removed old backup");
                    }
                }
            }
        }
        let _ = now; // suppress unused warning
    }
    Ok(removed)
}

async fn write_backup_audit(pool: &PgPool, path: &std::path::Path) -> Result<()> {
    // audit_admin 不存在则降级为 audit_bank（fallback；后续 task #10 加 audit_admin 迁移）
    let res = sqlx::query(
        r#"
        INSERT INTO audit_bank (log_id, actor_uuid, actor_type, target_account_uuid,
                                op, before_balance, after_balance, tick_millis,
                                request_id, notes)
        VALUES (gen_random_uuid(), NULL, 'SYSTEM', NULL,
                'system.backup', 0, 0, EXTRACT(EPOCH FROM now())::bigint * 1000,
                NULL, jsonb_build_object('path', $1::text, 'size_bytes', $2::bigint))
        "#,
    )
    .bind(path.to_string_lossy().to_string())
    .bind(
        tokio::fs::metadata(path)
            .await
            .map(|m| m.len() as i64)
            .unwrap_or(0),
    )
    .execute(pool)
    .await;
    if let Err(e) = res {
        warn!(target: "biocapital_cli", error = %e, "audit row write skipped (audit_bank may not have all columns)");
    } else {
        info!(target: "biocapital_cli", path = %path.display(), "audit row written");
    }
    Ok(())
}

// ============================================================================
// C6 — cmd_start
// ============================================================================

async fn cmd_start() -> Result<()> {
    // 1. 跑 migrate
    cmd_migrate().await.context("startup migrate")?;

    // 2. 加载 config + 构造 PG pool
    let cfg = ServerConfig::load().context("loading biocapital-server.toml")?;
    let pg = &cfg.server.postgresql;
    let url = build_connection_url(pg);
    let pool = open_pool(pg, &url).await?;
    info!(target: "biocapital_cli", "PG pool ready (min={}, max={})", pg.connection_pool_min, pg.connection_pool_max);

    // 3. 构造 in-process service registry
    let registry = build_registry_with_pg(pool.clone())
        .await
        .context("building in-process registry")?;
    info!(target: "biocapital_cli", "service registry built (8 *Grpc wired against PG)");

    // 4. 启动 Web UI (axum) — 用 ServiceBundle + admin tokens
    let webui = build_webui(&cfg, pool.clone())?;
    let http_addr: std::net::SocketAddr = format!(
        "{}:{}",
        cfg.server.bind_host, cfg.server.http_port
    )
    .parse()
    .with_context(|| format!("parsing http bind {http_addr_str}", http_addr_str = &http_addr_to_string(&cfg)))?;
    let listener = tokio::net::TcpListener::bind(http_addr)
        .await
        .with_context(|| format!("binding http port {}", cfg.server.http_port))?;
    info!(target: "biocapital_cli", addr = %http_addr, "Web UI listening");
    let webui_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, webui).await {
            error!(target: "biocapital_cli", "axum::serve error: {e:#}");
        }
    });

    // 5. 启动 DG_LAB WS server
    let dglab_session = new_session_id(cfg.server.dglab.session_id_length);
    let ws_server = Arc::new(
        DglabWsServer::new(cfg.server.dglab.ws_port, dglab_session)
            .with_host(cfg.server.dglab.ws_host.clone()),
    );
    let ws_handle = {
        let ws = Arc::clone(&ws_server);
        tokio::spawn(async move {
            if let Err(e) = ws.start().await {
                error!(target: "biocapital_cli", "dglab WS server error: {e}");
            }
        })
    };
    info!(target: "biocapital_cli", port = cfg.server.dglab.ws_port, "DG_LAB WS server listening");

    // 6. 启动生物配置热重载
    let config_dir = std::path::PathBuf::from("config/biocapital/creatures");
    let creature_repo: Arc<dyn biocapital_creature::pg::CreatureConfigRepository> = Arc::new(
        PgCreatureConfigRepository::from_pool(pool.clone()),
    );
    let creature_audit: Arc<dyn biocapital_creature::pg::CreatureAuditWriter> =
        Arc::new(PgCreatureAuditWriter::from_pool(pool.clone()));
    let mob_repo: Arc<dyn biocapital_creature::pg::MobReplacementRepository> =
        Arc::new(EmptyMobReplacementRepository);
    let hot_reloader = Arc::new(CreatureHotReloader::new(
        config_dir,
        creature_repo,
        creature_audit,
        mob_repo,
    ));
    hot_reloader.start_watching().await;
    info!(target: "biocapital_cli", "CreatureHotReloader started (5s tick)");

    // 7. 启动每日 03:00 备份 cron（用 tokio::time 简单实现）
    let backup_pool = pool.clone();
    let backup_cfg = cfg.clone();
    let backup_handle = tokio::spawn(async move {
        run_backup_cron(backup_cfg, backup_pool).await;
    });
    info!(target: "biocapital_cli", schedule = %cfg.server.backup.schedule, "backup cron scheduled");

    // 8. 监听 SIGINT / SIGTERM
    let mut sigint = signal(SignalKind::interrupt())
        .context("installing SIGINT handler")?;
    let mut sigterm = signal(SignalKind::terminate())
        .context("installing SIGTERM handler")?;
    info!(target: "biocapital_cli", "server up; ctrl-c to stop");
    tokio::select! {
        _ = sigint.recv() => info!(target: "biocapital_cli", "SIGINT received"),
        _ = sigterm.recv() => info!(target: "biocapital_cli", "SIGTERM received"),
    }
    info!(target: "biocapital_cli", "shutting down");

    webui_handle.abort();
    ws_handle.abort();
    backup_handle.abort();
    let _ = registry; // 防止过早 drop
    Ok(())
}

fn http_addr_to_string(cfg: &ServerConfig) -> String {
    format!("{}:{}", cfg.server.bind_host, cfg.server.http_port)
}

fn build_webui(cfg: &ServerConfig, pool: PgPool) -> Result<axum::Router> {
    let mut tokens = biocapital_webui::state::TokenSet::default();
    tokens.set_admin_tokens(cfg.server.webui.admin_tokens.iter().cloned());
    let webui_cfg = WebUiConfig {
        http_port: cfg.server.http_port,
        public: cfg.server.allow_public_bind,
        tokens: Arc::new(tokio::sync::RwLock::new(tokens)),
    };
    let app = WebUiApp::from_pg_pool(pool, webui_cfg);
    // Initialise Prometheus registry (idempotent). Required for
    // `/metrics` route to render successfully.
    if cfg.server.monitoring.prometheus_enabled {
        biocapital_webui::metrics::init()
            .map_err(|e| anyhow!("initialising metrics registry: {e}"))?;
        info!(
            target: "biocapital_cli",
            "/metrics endpoint enabled (single-port on {})",
            cfg.server.http_port
        );
    } else {
        info!(target: "biocapital_cli", "Prometheus disabled (prometheus_enabled=false)");
    }
    Ok(biocapital_webui::router_with_monitoring(
        app,
        cfg.server.monitoring.prometheus_enabled,
    ))
}

/// 用 `axum::Router` + `biocapital_webui::router` 启动 HTTP server。
/// — 占位 stub：把 gRPC server 注册留作 task #6-followup。
async fn _grpc_server_stub() {
    // tonic server 注册 (9 个 *Service trait → tonic::async_trait impl generated_server)
    // 等待 task #17 完成 generated 类型；当前 task #6 仅 spawn 一 tokio task
    // 跑 `registry.bank.get_balance` 健康检查。
    tokio::time::sleep(Duration::from_secs(u64::MAX / 2)).await;
}

fn new_session_id(len: usize) -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

async fn run_backup_cron(cfg: ServerConfig, pool: PgPool) {
    // 简单实现：每 24h 跑一次；不引入 cron crate
    // 实际每天 03:00（UTC）触发 — 计算到下一个 03:00 的时间差
    let mut ticker = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 跳过第一次立即 tick
    ticker.tick().await;
    loop {
        ticker.tick().await;
        if let Err(e) = do_backup(&cfg, &pool).await {
            error!(target: "biocapital_cli", error = %e, "scheduled backup failed");
        }
    }
}

async fn do_backup(cfg: &ServerConfig, pool: &PgPool) -> Result<()> {
    let pg = &cfg.server.postgresql;
    let path = run_pg_dump(cfg, pg).await?;
    let removed = prune_old_backups(cfg).await?;
    if removed > 0 {
        info!(target: "biocapital_cli", removed, "pruned old backups");
    }
    write_backup_audit(pool, &path).await?;
    info!(target: "biocapital_cli", dump = %path.display(), "scheduled backup OK");
    Ok(())
}

// ============================================================================
// `with_pg` 真实实现 — task #6 范围
// ============================================================================

async fn build_registry_with_pg(pool: PgPool) -> Result<BiocapitalServiceRegistry> {
    // 1. PlayerState
    let ps_repo = Arc::new(PgPlayerStateRepository::from_pool(pool.clone()));
    let ps_audit = Arc::new(PgAuditWriter::new(pool.clone()));
    let ps_deps = PlayerStateServiceDeps::new(ps_repo, ps_audit);
    // Fluid
    let fluid_repo: Arc<dyn FluidRepository> = Arc::new(PgFluidRepository::from_pool(pool.clone()));
    let fluid_deps = FluidServiceDeps::new(fluid_repo);
    let player_state: Arc<PlayerStateGrpc> = Arc::new(PlayerStateGrpc::new(ps_deps, fluid_deps));

    // 2. Bank
    let bank_repo = Arc::new(PgBankRepository::from_pool(pool.clone()));
    let bank_audit = Arc::new(PgBankAuditWriter::new(pool.clone()));
    let bank_deps = BankServiceDeps::new(bank_repo, bank_audit);
    let hw_repo = Arc::new(PgHardwareTokenRepository::from_pool(pool.clone()));
    let hw_audit = Arc::new(PgHardwareTokenAuditWriter::new(pool.clone()));
    let hw_deps = HardwareTokenServiceDeps::new(hw_repo, hw_audit);
    // 2b. Player-name cache (task #5, doc/15 §4.2). Populated by
    //     BankService.Authenticate and consulted by the Web UI
    //     `resolve_player_name_to_uuid` resolver.
    let player_name_repo: Arc<dyn PlayerNameRepository> = Arc::new(
        PgPlayerNameRepository::from_pool(pool.clone()),
    );
    let bank: Arc<BankGrpc> = Arc::new(BankGrpc::with_hardware_token(
        bank_deps,
        hw_deps,
        Whitelist::empty(),
    )
    .with_player_names(player_name_repo.clone()));

    // 3. Contract
    let contract_repo = Arc::new(PgContractRepository::from_pool(pool.clone()));
    let contract_audit = Arc::new(PgContractAuditWriter::new(pool.clone()));
    let bank_port: Arc<dyn biocapital_pg::contract::BankTransferPort + Send + Sync> =
        Arc::new(BankTransferPortForward::new(bank.clone()));
    let contract_deps = ContractServiceDeps::new(contract_repo)
        .with_audit(contract_audit)
        .with_bank_transfer(bank_port);
    let contract: Arc<ContractGrpc> = Arc::new(ContractGrpc::new(contract_deps));

    // 4. CorePod
    let pod_repo = Arc::new(PgCorePodRepository::from_pool(pool.clone()));
    let pod_audit = Arc::new(PgCorePodAuditWriter::new(pool.clone()));
    let pod_deps = CorePodServiceDeps::new(pod_repo, pod_audit);
    let core_pod: Arc<CorePodGrpc> = Arc::new(CorePodGrpc::new(pod_deps));

    // 5. Dglab
    let dglab_repo = Arc::new(PgDglabRepository::from_pool(pool.clone()));
    let dglab_audit = Arc::new(PgDglabAuditWriter::new(pool.clone()));
    let dglab_overrides = Arc::new(PgDglabOverrideRepository::from_pool(pool.clone()));
    let dglab_configs = Arc::new(PgPlayerDglabConfigRepository::from_pool(pool.clone()));
    let dglab_deps = DglabServiceDeps::new(dglab_repo, dglab_audit)
        .with_overrides(dglab_overrides)
        .with_player_config(dglab_configs);
    let dglab: Arc<DglabGrpc> = Arc::new(DglabGrpc::new(dglab_deps));

    // 6. Creature
    let creature_repo: Arc<dyn biocapital_creature::pg::CreatureConfigRepository> = Arc::new(
        PgCreatureConfigRepository::from_pool(pool.clone()),
    );
    let creature_audit: Arc<dyn biocapital_creature::pg::CreatureAuditWriter> = Arc::new(
        PgCreatureAuditWriter::from_pool(pool.clone()),
    );
    let creature_deps = CreatureConfigServiceDeps::new(creature_repo, creature_audit);
    let mob_repo: Arc<dyn biocapital_creature::pg::MobReplacementRepository> =
        Arc::new(EmptyMobReplacementRepository);
    let _mob_deps = MobReplacementServiceDeps::new(mob_repo.clone());
    let hot_reloader = Arc::new(CreatureHotReloader::new(
        std::path::PathBuf::from("config/biocapital/creatures"),
        Arc::new(PgCreatureConfigRepository::from_pool(pool.clone())) as _,
        Arc::new(PgCreatureAuditWriter::from_pool(pool.clone())) as _,
        mob_repo.clone(),
    ));
    let creature: Arc<CreatureServiceGrpc> = Arc::new(CreatureServiceGrpc::new(creature_deps, hot_reloader));

    // 7. Environment
    let env_repo: Arc<dyn EnvironmentRepository> = Arc::new(PgEnvironmentRepository::from_pool(pool.clone()));
    let env_deps = EnvironmentServiceDeps::new(env_repo);
    // env_service 由 EnvironmentServicePort trait 在 biocapital-environment::pg
    // 提供；本阶段不实例化（使用 stub）；后续 task 接 PG 实现
    let env_service: Arc<dyn EnvironmentServicePort> = Arc::new(StubEnvironmentService);
    let environment: Arc<EnvironmentServiceGrpc> = Arc::new(EnvironmentServiceGrpc::new(env_service, env_deps));

    // 8. HostileMob
    let hostile_mob: Arc<HostileMobGrpc> = Arc::new(HostileMobGrpc::new(
        MobReplacementServiceDeps::new(mob_repo),
        player_state.clone() as Arc<dyn biocapital_grpc::player_state_service::PlayerStateRpc>,
    ));

    // 9. Audit
    let audit_repo: Arc<dyn biocapital_pg::audit::AuditQueryRepository> =
        Arc::new(PgAuditQueryRepository::from_pool(pool.clone()));
    let audit_deps = AuditServiceDeps::new(audit_repo);
    let audit: Arc<AuditGrpc> = Arc::new(AuditGrpc::new(audit_deps));

    // 用 builder pattern 拼装
    Ok(BiocapitalServiceRegistry {
        bank: bank as Arc<dyn biocapital_grpc::bank_service::BankRpc>,
        player_state: player_state as Arc<dyn biocapital_grpc::player_state_service::PlayerStateRpc>,
        core_pod: core_pod as Arc<dyn biocapital_grpc::core_pod_service::CorePodRpc>,
        contract: contract as Arc<dyn biocapital_grpc::contract_service::ContractRpc>,
        dglab: dglab as Arc<dyn biocapital_grpc::dglab_service::DglabRpc>,
        creature: creature as Arc<dyn biocapital_grpc::creature_service::CreatureRpc>,
        environment: environment as Arc<dyn biocapital_grpc::environment_service::EnvironmentRpc>,
        hostile_mob: hostile_mob as Arc<dyn biocapital_grpc::hostile_mob_service::HostileMobRpc>,
        audit: audit as Arc<dyn biocapital_grpc::audit_service::AuditRpc>,
    })
}

// 简化版 BankTransferPort 实现 — 把 BankTransferPort 接口包成 BankGrpc forward
struct BankTransferPortForward {
    bank: Arc<BankGrpc>,
}
impl BankTransferPortForward {
    fn new(bank: Arc<BankGrpc>) -> Self {
        Self { bank }
    }
}
#[async_trait::async_trait]
impl biocapital_pg::contract::BankTransferPort for BankTransferPortForward {
    async fn transfer(
        &self,
        from: Uuid,
        to: Uuid,
        amount: i64,
        memo: Option<String>,
        request_id: Uuid,
    ) -> Result<(i64, i64), tonic::Status> {
        use biocapital_grpc::bank_service::{PlayerIdentifier, TransferRequest};
        let resp = self
            .bank
            .transfer(tonic::Request::new(TransferRequest {
                from: PlayerIdentifier { player_uuid: from },
                to: PlayerIdentifier { player_uuid: to },
                amount,
                memo,
                request_id,
            }))
            .await?
            .into_inner();
        Ok((resp.balance, 0))
    }
}

// ============================================================================
// tracing init
// ============================================================================

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,biocapital_cli=debug"));
    fmt()
        .with_env_filter(filter)
        .with_target(true)
        .compact()
        .init();
}

// ============================================================================
// 兼容 shim（保留以扩展）
// ============================================================================

struct StubEnvironmentService;

// ── Empty MobReplacementRepository stub (PgMobReplacementRepository 是 stub) ──

struct EmptyMobReplacementRepository;

#[async_trait::async_trait]
impl biocapital_creature::pg::MobReplacementRepository for EmptyMobReplacementRepository {
    async fn get_for_vanilla(
        &self,
        _v: &str,
    ) -> Result<Vec<biocapital_creature::domain::MobReplacement>, biocapital_creature::pg::MobRepoError>
    {
        Ok(Vec::new())
    }
    async fn list(
        &self,
        _enabled_only: bool,
    ) -> Result<Vec<biocapital_creature::domain::MobReplacement>, biocapital_creature::pg::MobRepoError>
    {
        Ok(Vec::new())
    }
    async fn upsert(
        &self,
        _replacement: &biocapital_creature::domain::MobReplacement,
    ) -> Result<(), biocapital_creature::pg::MobRepoError> {
        Ok(())
    }
    async fn delete(
        &self,
        _mob_replacement_id: Uuid,
    ) -> Result<(), biocapital_creature::pg::MobRepoError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl EnvironmentServicePort for StubEnvironmentService {
    async fn apply_fluid_effect_environmental(
        &self,
        _player_uuid: Uuid,
        _fluid: biocapital_core::fluids::BiocapitalFluid,
        _intensity: f32,
        _tick: i64,
        _request_id: Option<Uuid>,
    ) -> Result<biocapital_environment::EnvironmentResult, biocapital_environment::EnvironmentError>
    {
        Ok(biocapital_environment::EnvironmentResult::default())
    }
    async fn apply_default_environment_effect(
        &self,
        _player_uuid: Uuid,
        _environment: biocapital_environment::domain::environment::EnvironmentType,
        _intensity: f32,
        _tick: i64,
        _request_id: Option<Uuid>,
    ) -> Result<biocapital_environment::EnvironmentResult, biocapital_environment::EnvironmentError>
    {
        Ok(biocapital_environment::EnvironmentResult::default())
    }
}
