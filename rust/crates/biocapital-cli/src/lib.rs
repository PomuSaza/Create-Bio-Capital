//! CLI entry — start / migrate / backup / config-load
//!
//! 权威: `doc/14-rust-services.md` §1.2/§2.1
//!
//! 模块清单：
//! - `config` — Rust 端 `biocapital-server.toml` 加载 + 校验（11 §2.3）
//! - `sql_loader` — D1 决策：自定义 SQL 迁移 loader（**不**用 sqlx::migrate!）

pub mod config;
pub mod sql_loader;

pub use config::{
    BackupSection, ConfigError, DglabSection, LoggingSection, MonitoringSection, PostgresSection,
    ServerConfig, ServerSection, WebuiSection,
};
pub use sql_loader::run_sql_files;