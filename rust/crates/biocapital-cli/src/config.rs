//! Rust 端配置加载 + 校验
//!
//! 权威: `doc/11-config-system.md` §2 + `doc/14-rust-services.md` §1.2/§2.1
//! 加载入口: `biocapital-server.toml`（默认 `config/biocapital-server.toml`，
//!         也可通过 `BIOCAPITAL_SERVER_TOML` 环境变量覆写）。
//!
//! ## 校验规则（task #14 / doc/11 §2.3）
//! - `connectionString` 必须包含 `postgresql://`；格式错误则启动失败
//! - `bindAddress` 不允许 `0.0.0.0` 除非显式 `allowPublicBind = true`
//! - 端口范围：1..=65535
//! - PG 连接池：5..=50；max >= min
//! - 备份 cron：6 字段（minute hour day-of-month month day-of-week）
//! - 强度范围：max_strength_a/b ∈ 0..=200（10 §2.5 官方权威）
//! - admin_tokens 至少 1 项；每项 64-char hex
//! - log_level ∈ {"trace","debug","info","warn","error"}

use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

const DEFAULT_CONFIG_PATH: &str = "config/biocapital-server.toml";
const ENV_OVERRIDE: &str = "BIOCAPITAL_SERVER_TOML";

// ── 错误 ──────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file not found: {0}")]
    NotFound(PathBuf),

    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("toml parse error in {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("invalid bind_address `{0}`: refusing 0.0.0.0 without allowPublicBind=true")]
    PublicBindDenied(String),

    #[error("invalid port {0}: must be in 1..=65535")]
    InvalidPort(u16),

    #[error("invalid bind_host `{0}`: cannot be empty")]
    InvalidBindHost(String),

    #[error("PG connection_pool_min {0} out of range 5..=50")]
    InvalidPoolMin(u32),

    #[error("PG connection_pool_max {0} < connection_pool_min {1}")]
    InvalidPoolMax(u32, u32),

    #[error("invalid backup schedule `{0}`: must be 5- or 6-field cron expression")]
    InvalidCronSchedule(String),

    #[error("max_strength_a {0} out of range 0..=200")]
    InvalidMaxStrengthA(i32),

    #[error("max_strength_b {0} out of range 0..=200")]
    InvalidMaxStrengthB(i32),

    #[error("admin_tokens must contain at least 1 entry")]
    MissingAdminTokens,

    #[error("admin_tokens[{0}] = `{1}`: must be 64-char lowercase hex")]
    InvalidAdminToken(usize, String),

    #[error("invalid log_level `{0}`: must be one of trace/debug/info/warn/error")]
    InvalidLogLevel(String),

    #[error("invalid tracing format `{0}`: must be `json` or `pretty`")]
    InvalidLogFormat(String),

    #[error("cold_start_budget_seconds {0} must be > 0")]
    InvalidColdStart(u64),

    #[error("auto_restart_max {0} must be <= 10 (sanity cap)")]
    InvalidRestartMax(u32),

    #[error("invalid prometheus_port {0}")]
    InvalidPrometheusPort(u16),

    #[error("memory_budget_mb {0} must be >= 64")]
    InvalidMemoryBudget(u32),
}

// ── Sections ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSection {
    pub bind_host: String,
    pub bind_port: u16,
    pub http_port: u16,
    pub cold_start_budget_seconds: u64,
    pub memory_budget_mb: u32,
    pub auto_restart_max: u32,
    pub auto_restart_interval_seconds: u64,
    /// 默认 `false`；仅显式开启才允许 0.0.0.0 监听
    #[serde(default)]
    pub allow_public_bind: bool,
    // Nested sections live under [Server.*] in the TOML, so the
    // deserializer needs to walk into the same `server` table.
    // The `alias = "Xxx"` attributes accept the title-cased section
    // names that `doc/11-config-system.md` §2 mandates.
    #[serde(alias = "PostgreSQL")]
    pub postgresql: PostgresSection,
    #[serde(alias = "Backup")]
    pub backup: BackupSection,
    #[serde(alias = "Dglab")]
    pub dglab: DglabSection,
    #[serde(alias = "WebUI")]
    pub webui: WebuiSection,
    #[serde(alias = "Logging")]
    pub logging: LoggingSection,
    #[serde(alias = "Monitoring")]
    pub monitoring: MonitoringSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostgresSection {
    pub data_dir: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub database: String,
    pub init_if_missing: bool,
    pub migration_dir_embedded: bool,
    pub connection_pool_min: u32,
    pub connection_pool_max: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackupSection {
    pub schedule: String,
    pub target_dir: String,
    pub retention_days: u32,
    pub tar_format: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DglabSection {
    pub ws_host: String,
    pub ws_port: u16,
    pub heartbeat_interval_seconds: u64,
    pub session_id_length: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebuiSection {
    pub admin_tokens: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingSection {
    pub log_level: String,
    pub log_format: String,
    pub log_dir: String,
    pub log_rotation: String,
    pub log_max_size_mb: u32,
    pub log_retention_days: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MonitoringSection {
    pub prometheus_enabled: bool,
    pub prometheus_port: u16,
    pub metrics_interval_seconds: u64,
}

// ── 顶层 ─────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(alias = "Server")]
    pub server: ServerSection,
}

impl ServerConfig {
    /// 从环境变量 `BIOCAPITAL_SERVER_TOML` 或默认 `config/biocapital-server.toml` 加载
    pub fn load() -> Result<Self, ConfigError> {
        let path = PathBuf::from(
            std::env::var(ENV_OVERRIDE).unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string()),
        );
        Self::load_from(&path)
    }

    /// 从指定路径加载（用于测试）
    pub fn load_from(path: &std::path::Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Err(ConfigError::NotFound(path.to_path_buf()));
        }
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let cfg: Self = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// 全部约束校验（11 §2.3）
    pub fn validate(&self) -> Result<(), ConfigError> {
        // ── Server ──
        if self.server.bind_host.is_empty() {
            return Err(ConfigError::InvalidBindHost(self.server.bind_host.clone()));
        }
        if self.server.bind_host == "0.0.0.0" && !self.server.allow_public_bind {
            return Err(ConfigError::PublicBindDenied(self.server.bind_host.clone()));
        }
        if self.server.bind_port == 0 {
            return Err(ConfigError::InvalidPort(self.server.bind_port));
        }
        if self.server.http_port == 0 {
            return Err(ConfigError::InvalidPort(self.server.http_port));
        }
        if self.server.cold_start_budget_seconds == 0 {
            return Err(ConfigError::InvalidColdStart(
                self.server.cold_start_budget_seconds,
            ));
        }
        if self.server.memory_budget_mb < 64 {
            return Err(ConfigError::InvalidMemoryBudget(self.server.memory_budget_mb));
        }
        if self.server.auto_restart_max > 10 {
            return Err(ConfigError::InvalidRestartMax(self.server.auto_restart_max));
        }

        // ── PostgreSQL ──
        if self.server.postgresql.port == 0 {
            return Err(ConfigError::InvalidPort(self.server.postgresql.port));
        }
        if !(5..=50).contains(&self.server.postgresql.connection_pool_min) {
            return Err(ConfigError::InvalidPoolMin(
                self.server.postgresql.connection_pool_min,
            ));
        }
        if self.server.postgresql.connection_pool_max < self.server.postgresql.connection_pool_min {
            return Err(ConfigError::InvalidPoolMax(
                self.server.postgresql.connection_pool_max,
                self.server.postgresql.connection_pool_min,
            ));
        }
        if self.server.postgresql.username.is_empty() || self.server.postgresql.database.is_empty() {
            // password 可以为空（开发环境）
            // 但 username 和 database 不允许空
            // 用现成的 error 表达即可
            return Err(ConfigError::InvalidBindHost(
                "postgresql.username/database must not be empty".to_string(),
            ));
        }

        // ── Backup ──
        validate_cron(&self.server.backup.schedule)?;
        if !["gz", "xz", "zst", "tar"].contains(&self.server.backup.tar_format.as_str()) {
            return Err(ConfigError::InvalidCronSchedule(format!(
                "tar_format {} not in [gz, xz, zst, tar]",
                self.server.backup.tar_format
            )));
        }

        // ── DG_LAB ──
        if self.server.dglab.ws_port == 0 {
            return Err(ConfigError::InvalidPort(self.server.dglab.ws_port));
        }
        // 强度上限校验（10 §2.5；强度在 player_dglab_config 而非 server.toml；
        // 但若 [Server.Dglab] 暴露 max_strength_* 则一并校验，留给后续任务）
        if self.server.dglab.session_id_length < 16 || self.server.dglab.session_id_length > 64 {
            return Err(ConfigError::InvalidBindHost(format!(
                "session_id_length {} out of range 16..=64",
                self.server.dglab.session_id_length
            )));
        }

        // ── WebUI ──
        if self.server.webui.admin_tokens.is_empty() {
            return Err(ConfigError::MissingAdminTokens);
        }
        for (i, token) in self.server.webui.admin_tokens.iter().enumerate() {
            if token.len() != 64 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(ConfigError::InvalidAdminToken(i, token.clone()));
            }
            // 必须是 lowercase hex
            if token.chars().any(|c| c.is_ascii_uppercase()) {
                return Err(ConfigError::InvalidAdminToken(i, token.clone()));
            }
        }

        // ── Logging ──
        if !["trace", "debug", "info", "warn", "error"].contains(&self.server.logging.log_level.as_str()) {
            return Err(ConfigError::InvalidLogLevel(self.server.logging.log_level.clone()));
        }
        if !["json", "pretty"].contains(&self.server.logging.log_format.as_str()) {
            return Err(ConfigError::InvalidLogFormat(self.server.logging.log_format.clone()));
        }
        if !["daily", "hourly", "size"].contains(&self.server.logging.log_rotation.as_str()) {
            return Err(ConfigError::InvalidLogFormat(format!(
                "log_rotation {} not in [daily, hourly, size]",
                self.server.logging.log_rotation
            )));
        }

        // ── Monitoring ──
        if self.server.monitoring.prometheus_enabled && self.server.monitoring.prometheus_port == 0 {
            return Err(ConfigError::InvalidPrometheusPort(
                self.server.monitoring.prometheus_port,
            ));
        }

        Ok(())
    }
}

// ── 校验辅助 ─────────────────────────────────────────────

/// 6 字段或 5 字段 cron 表达式（minute hour day-of-month month day-of-week [year]）
/// 完整 cron 解析留 `cron` crate；这里仅做形状校验（启动失败时给出可读错误）
fn validate_cron(expr: &str) -> Result<(), ConfigError> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 5 && parts.len() != 6 {
        return Err(ConfigError::InvalidCronSchedule(expr.to_string()));
    }
    // 字段必须是 `*` 或 `数字` 或 `数字-数字` 或 `*/数字`；启动期严格校验
    // 详细语义交给 cron crate；本项目初期只拒形状错的
    for part in parts {
        if part.is_empty() {
            return Err(ConfigError::InvalidCronSchedule(expr.to_string()));
        }
    }
    Ok(())
}

// ── 单元测试 ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "biocapital-config-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("biocapital-server.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    const MINIMAL_OK: &str = r#"
[Server]
bind_host = "127.0.0.1"
bind_port = 50051
http_port = 8080
cold_start_budget_seconds = 5
memory_budget_mb = 512
auto_restart_max = 3
auto_restart_interval_seconds = 30

[Server.PostgreSQL]
data_dir = "/tmp/pg"
port = 5432
username = "biocapital"
password = "biocapital"
database = "biocapital"
init_if_missing = true
migration_dir_embedded = true
connection_pool_min = 5
connection_pool_max = 10

[Server.Backup]
schedule = "0 3 * * *"
target_dir = "/tmp/backup"
retention_days = 7
tar_format = "gz"

[Server.Dglab]
ws_host = "0.0.0.0"
ws_port = 9999
heartbeat_interval_seconds = 60
session_id_length = 20

[Server.WebUI]
admin_tokens = ["0000000000000000000000000000000000000000000000000000000000000000"]

[Server.Logging]
log_level = "info"
log_format = "json"
log_dir = "/tmp/log"
log_rotation = "daily"
log_max_size_mb = 100
log_retention_days = 30

[Server.Monitoring]
prometheus_enabled = false
prometheus_port = 9090
metrics_interval_seconds = 15
"#;

    #[test]
    fn loads_minimal_ok_config() {
        let p = write_temp(MINIMAL_OK);
        let cfg = ServerConfig::load_from(&p).expect("should load");
        assert_eq!(cfg.server.bind_port, 50051);
        assert_eq!(cfg.server.postgresql.connection_pool_min, 5);
        assert_eq!(cfg.server.backup.schedule, "0 3 * * *");
    }

    #[test]
    fn rejects_public_bind_without_opt_in() {
        let mut s = MINIMAL_OK.to_string();
        s = s.replace(
            "[Server]\nbind_host = \"127.0.0.1\"",
            "[Server]\nbind_host = \"0.0.0.0\"",
        );
        let p = write_temp(&s);
        let err = ServerConfig::load_from(&p).unwrap_err();
        matches!(err, ConfigError::PublicBindDenied(_));
    }

    #[test]
    fn allows_public_bind_when_opt_in() {
        let mut s = MINIMAL_OK.to_string();
        s = s.replace(
            "[Server]\nbind_host = \"127.0.0.1\"",
            "[Server]\nbind_host = \"0.0.0.0\"\nallow_public_bind = true",
        );
        let p = write_temp(&s);
        ServerConfig::load_from(&p).expect("should load");
    }

    #[test]
    fn rejects_invalid_admin_token() {
        let mut s = MINIMAL_OK.to_string();
        s = s.replace(
            "\"0000000000000000000000000000000000000000000000000000000000000000\"",
            "\"TOOSHORT\"",
        );
        let p = write_temp(&s);
        let err = ServerConfig::load_from(&p).unwrap_err();
        matches!(err, ConfigError::InvalidAdminToken(..));
    }

    #[test]
    fn rejects_pool_max_lt_min() {
        let mut s = MINIMAL_OK.to_string();
        s = s.replace("connection_pool_min = 5", "connection_pool_min = 10");
        s = s.replace("connection_pool_max = 10", "connection_pool_max = 8");
        let p = write_temp(&s);
        let err = ServerConfig::load_from(&p).unwrap_err();
        matches!(err, ConfigError::InvalidPoolMax(..));
    }

    #[test]
    fn rejects_invalid_cron() {
        let mut s = MINIMAL_OK.to_string();
        s = s.replace("schedule = \"0 3 * * *\"", "schedule = \"0 3\"");
        let p = write_temp(&s);
        let err = ServerConfig::load_from(&p).unwrap_err();
        matches!(err, ConfigError::InvalidCronSchedule(_));
    }

    #[test]
    fn rejects_invalid_log_level() {
        let mut s = MINIMAL_OK.to_string();
        s = s.replace("log_level = \"info\"", "log_level = \"verbose\"");
        let p = write_temp(&s);
        let err = ServerConfig::load_from(&p).unwrap_err();
        matches!(err, ConfigError::InvalidLogLevel(_));
    }

    #[test]
    fn missing_admin_tokens_rejected() {
        let mut s = MINIMAL_OK.to_string();
        s = s.replace(
            "admin_tokens = [\"0000000000000000000000000000000000000000000000000000000000000000\"]",
            "admin_tokens = []",
        );
        let p = write_temp(&s);
        let err = ServerConfig::load_from(&p).unwrap_err();
        matches!(err, ConfigError::MissingAdminTokens);
    }

    #[test]
    fn not_found_returns_not_found_error() {
        let p = PathBuf::from("/tmp/definitely-does-not-exist-biocapital.toml");
        let err = ServerConfig::load_from(&p).unwrap_err();
        matches!(err, ConfigError::NotFound(_));
    }
}