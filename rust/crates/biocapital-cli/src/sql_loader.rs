//! D1 决策 — 自定义 SQL 迁移 loader
//!
//! 权威: `doc/01-cross-cutting-concerns.md` §1.1.1 + `doc/14-rust-services.md` §2.1
//!
//! ## 与 sqlx::migrate! 的区别
//!
//! - **不**使用 sqlx 自带的版本跟踪（`_sqlx_migrations` 表）
//! - **不**要求日期/版本前缀（用户原话："为什么会有带日期的这个表单的这个数据库？这肯定不正常"）
//! - 按文件名**字母序**执行
//! - 每个 `.sql` 文件应自行保证**幂等性**（使用 `IF NOT EXISTS` / `CREATE OR REPLACE` 等）
//!
//! ## 用法
//!
//! ```no_run
//! use biocapital_cli::sql_loader::run_sql_files;
//! use std::path::Path;
//! # async fn run(pool: sqlx::PgPool) -> anyhow::Result<()> {
//! run_sql_files(&pool, Path::new("./config/biocapital/sql/")).await?;
//! # Ok(()) }
//! ```

use std::path::Path;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use sqlx::PgPool;
use tracing::{info, warn};

/// 在 `dir` 目录中按字母序执行所有 `*.sql` 文件（D1 决策）。
///
/// # 行为
/// - 目录不存在 → 报错（不静默创建）
/// - 无 `.sql` 文件 → WARN 日志 + 跳过（视为"无可应用迁移"）
/// - 文件名排序按**完整字符串**字母序；用户可加前缀（如 `001_xxx.sql`）控制顺序
/// - 每个文件整体作为一条 batch 执行（`sqlx::raw_sql`）；每个文件应自行事务化（`BEGIN/COMMIT`）
/// - 单文件失败 → 整体失败，**不**回滚已成功的文件（用户手动处理）
///
/// # 幂等性
///
/// 调用方负责确保每个文件**幂等**。本函数不做版本跟踪——重启 / 重跑会再次执行所有文件。
/// PG 端的 `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS` 是标准做法。
pub async fn run_sql_files(pool: &PgPool, dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Err(anyhow!(
            "migration dir not found: {} (D1: must be runtime dir, not source tree)",
            dir.display()
        ));
    }
    if !dir.is_dir() {
        return Err(anyhow!("migration path is not a dir: {}", dir.display()));
    }

    // Collect *.sql files (skip hidden files like .gitkeep)
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => return false,
            };
            // Skip hidden / dotfiles
            if name.starts_with('.') {
                return false;
            }
            // Only .sql extension
            path.extension().and_then(|x| x.to_str()) == Some("sql")
        })
        .collect();
    // 按完整路径字母序
    entries.sort_by_key(|e| e.path());

    if entries.is_empty() {
        warn!(
            "no .sql files found in {} (skipping; runtime dir may be empty)",
            dir.display()
        );
        return Ok(());
    }

    info!(
        "running {} SQL file(s) from {}",
        entries.len(),
        dir.display()
    );
    for entry in entries {
        let path = entry.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        let sql = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;

        let started = Instant::now();
        sqlx::raw_sql(&sql)
            .execute(pool)
            .await
            .with_context(|| format!("executing {}", name))?;
        info!(
            "  ✓ {} ({:.2}s)",
            name,
            started.elapsed().as_secs_f64()
        );
    }

    info!("SQL migrations done");
    Ok(())
}

// ── 单元测试 ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_filters_only_sql_files() {
        let dir = std::env::temp_dir().join(format!(
            "biocapital-sql-loader-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Create test files
        for name in &[
            "001_a.sql",
            "002_b.sql",
            "003_c.txt",   // should be filtered out
            ".gitkeep",    // should be filtered out (hidden)
            "README",      // no extension
        ] {
            std::fs::write(dir.join(name), "-- empty").unwrap();
        }

        // Manually do the same filter logic
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let path = e.path();
                let name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    None => return false,
                };
                if name.starts_with('.') {
                    return false;
                }
                path.extension().and_then(|x| x.to_str()) == Some("sql")
            })
            .collect();
        entries.sort_by_key(|e| e.path());

        let names: Vec<String> = entries
            .iter()
            .map(|e| {
                e.path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(names, vec!["001_a.sql", "002_b.sql"]);

        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn alphabetical_order_respects_prefix() {
        let dir = std::env::temp_dir().join(format!(
            "biocapital-sql-order-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for name in &["c.sql", "a.sql", "b.sql"] {
            std::fs::write(dir.join(name), "--").unwrap();
        }

        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().and_then(|x| x.to_str()) == Some("sql")
            })
            .collect();
        entries.sort_by_key(|e| e.path());

        let names: Vec<String> = entries
            .iter()
            .map(|e| e.path().file_name().unwrap().to_string_lossy().to_string())
            .collect();
        // 字母序：a, b, c
        assert_eq!(names, vec!["a.sql", "b.sql", "c.sql"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}