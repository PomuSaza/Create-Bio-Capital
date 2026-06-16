//! CLI entry — start / migrate / backup / config-load
//!
//! 权威: `doc/14-rust-services.md` §1.2/§2.1
//!
//! 模块清单（task #14 范围内）：
//! - `config` — Rust 端 `biocapital-server.toml` 加载 + 校验（11 §2.3）
//! - 启动期 init、备份、迁移：留待后续子任务

pub mod config;

pub use config::{ConfigError, ServerConfig};