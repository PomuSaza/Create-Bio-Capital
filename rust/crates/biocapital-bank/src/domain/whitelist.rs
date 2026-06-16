//! TG 群白名单内存 cache — `Whitelist`, `WhitelistToml`,
//! `WhitelistError`.
//!
//! Per `doc/18-tg-whitelist.md` §2, the whitelist is the
//! **primary authority** for player authentication:
//!
//! - 白名单玩家**永不**被风控拦截
//! - 白名单文件路径：`config/biocapital-whitelist.toml`
//! - 启动期加载 + 运行时 SIGHUP / file-notify 热重载
//!
//! The `Whitelist` struct is the in-memory representation. The
//! hot-path query `contains(uuid, username)` is a hash-lookup over
//! two `HashSet`s — 18 §8 budget: < 10 ms for 1k players, < 100 ms
//! for 10k. Lookups are case-insensitive on the username side
//! (matches Java's `String.equalsIgnoreCase`).
//!
//! Persistence: the whitelist itself is loaded from a toml file on
//! disk (1.1.4.1 of the project §1.1); there is no PG table for it
//! (deliberate — the whitelist is treated as a *configuration
//! source*, not a transactional entity).

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;
use thiserror::Error;
use uuid::Uuid;

// ── TOML schema (doc/18 §2.1) ───────────────────────────────────────────────

/// The on-disk toml schema. Mirrors the example in 18 §2.1:
///
/// ```toml
/// [players]
/// uuids = ["069a79f4-...-fca90e38aaf5", ...]
/// usernames = ["jeb_", "dinnerbone", ...]
/// ```
#[derive(Debug, Default, Clone, Deserialize)]
pub struct WhitelistToml {
    #[serde(default)]
    pub players: WhitelistTomlPlayers,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct WhitelistTomlPlayers {
    #[serde(default)]
    pub uuids: Vec<Uuid>,
    #[serde(default)]
    pub usernames: Vec<String>,
}

// ── Whitelist ───────────────────────────────────────────────────────────────

/// The in-memory whitelist. Two hash sets: one for UUIDs, one for
/// usernames (lowercased on insert). The `contains` query is O(1)
/// for UUID and O(1) for username (after the to-lowercase
/// transform).
#[derive(Debug, Default, Clone)]
pub struct Whitelist {
    pub uuids: HashSet<Uuid>,
    pub usernames: HashSet<String>,
}

impl Whitelist {
    /// Empty whitelist — used by the gRPC service as the cold-start
    /// placeholder before the toml file has been parsed.
    pub fn empty() -> Self {
        Self {
            uuids: HashSet::new(),
            usernames: HashSet::new(),
        }
    }

    /// True iff `uuid` is in the UUID set, **or** `username` (case
    /// insensitively) is in the username set. This is the 18 §2.3
    /// `is_whitelisted` query.
    pub fn contains(&self, uuid: Uuid, username: &str) -> bool {
        if self.uuids.contains(&uuid) {
            return true;
        }
        if username.is_empty() {
            return false;
        }
        // Lowercase once; usernames are stored lowercased on
        // load so the comparison is byte-equality.
        let lowered = username.to_lowercase();
        self.usernames.contains(&lowered)
    }

    /// Total count across both sets. Used by the gRPC
    /// `WhitelistReloadEvent` payload (99 §3.1 联动事件).
    pub fn len(&self) -> usize {
        self.uuids.len() + self.usernames.len()
    }

    /// True if the whitelist is empty.
    pub fn is_empty(&self) -> bool {
        self.uuids.is_empty() && self.usernames.is_empty()
    }

    /// Load from a toml file. The toml must be valid UTF-8; the
    /// `[players]` section may be empty (empty whitelist = no
    /// players can ever pass). UUIDs that fail to parse are surfaced
    /// as `InvalidUuid` errors so a typo in the config file doesn't
    /// silently shrink the whitelist.
    pub fn load_from_path(path: &Path) -> Result<Self, WhitelistError> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            WhitelistError::ReadFile {
                path: path.display().to_string(),
                source: e,
            }
        })?;
        let toml: WhitelistToml = toml::from_str(&text)?;
        Self::from_toml(toml)
    }

    /// Construct from a parsed toml. Lowercases every username on
    /// insert so the runtime `contains` query is byte-equality.
    pub fn from_toml(toml: WhitelistToml) -> Result<Self, WhitelistError> {
        let mut uuids = HashSet::new();
        for u in toml.players.uuids {
            uuids.insert(u); // Uuid::deserialize validates format
        }
        let mut usernames = HashSet::new();
        for u in toml.players.usernames {
            usernames.insert(u.to_lowercase());
        }
        Ok(Self { uuids, usernames })
    }
}

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum WhitelistError {
    #[error("failed to read whitelist file {path}: {source}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse whitelist toml: {0}")]
    Toml(#[from] toml::de::Error),
}

// ── Sanity tests (no DB) ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn wl() -> Whitelist {
        let toml: WhitelistToml = toml::from_str(
            r#"
            [players]
            uuids = [
              "069a79f4-44e9-4726-a5be-fca90e38aaf5",
              "f7c77d99-9f15-4a66-87e9-2b9b3a3e8c7d",
            ]
            usernames = [
              "jeb_",
              "Dinnerbone",
            ]
            "#,
        )
        .unwrap();
        Whitelist::from_toml(toml).unwrap()
    }

    #[test]
    fn contains_matches_uuid_or_username() {
        let w = wl();
        let notch = Uuid::parse_str("069a79f4-44e9-4726-a5be-fca90e38aaf5").unwrap();
        assert!(w.contains(notch, "anything"));
        assert!(w.contains(Uuid::nil(), "jeb_"));
        // Case-insensitive username match.
        assert!(w.contains(Uuid::nil(), "DINNERBONE"));
        assert!(w.contains(Uuid::nil(), "dinnerbone"));
        // Negative
        assert!(!w.contains(Uuid::nil(), "herobrine"));
    }

    #[test]
    fn empty_username_does_not_match() {
        let w = wl();
        // Username "" should never match (defensive: the gRPC
        // layer's PlayerLoggedInEvent always provides a real name
        // but we guard against a missing value).
        assert!(!w.contains(Uuid::nil(), ""));
    }

    #[test]
    fn empty_whitelist_contains_nothing() {
        let w = Whitelist::empty();
        assert!(!w.contains(Uuid::new_v4(), "anyone"));
        assert!(w.is_empty());
        assert_eq!(w.len(), 0);
    }

    #[test]
    fn len_counts_both_sets() {
        let w = wl();
        // 2 UUIDs + 2 usernames (lowercased on insert; "Dinnerbone" → "dinnerbone")
        assert_eq!(w.len(), 4);
        assert!(!w.is_empty());
    }

    #[test]
    fn missing_players_section_is_ok() {
        // Backward-compat: a file with no [players] section parses
        // to an empty whitelist.
        let toml: WhitelistToml = toml::from_str("").unwrap();
        let w = Whitelist::from_toml(toml).unwrap();
        assert!(w.is_empty());
    }

    #[test]
    fn duplicate_uuid_or_username_is_deduped() {
        let toml: WhitelistToml = toml::from_str(
            r#"
            [players]
            uuids = [
              "069a79f4-44e9-4726-a5be-fca90e38aaf5",
              "069a79f4-44e9-4726-a5be-fca90e38aaf5",
            ]
            usernames = ["jeb_", "JEB_"]
            "#,
        )
        .unwrap();
        let w = Whitelist::from_toml(toml).unwrap();
        assert_eq!(w.uuids.len(), 1);
        // Case-insensitive dedupe: "jeb_" + "JEB_" both lowercase
        // to "jeb_".
        assert_eq!(w.usernames.len(), 1);
    }
}
