-- 20260614000010_creature_configs.sql
-- Created: 2026-06-14 (task #11, doc/13-bio-customization.md + doc/99-integration-matrix.md §5.1)
--
-- Creature-customization durable schema. PostgreSQL is the
-- source of truth for the per-creature metadata parsed out of
-- `config/biocapital/creatures/<creature_id>/creatures.json`
-- (13 §2.1) and reloaded on every file mtime change
-- (`CreatureService.ReloadCreatures`, 13 §5 / 13 §6.2).
--
-- Companion code:
--   - rust/crates/biocapital-creature/src/domain/creature.rs       (CreatureConfig)
--   - rust/crates/biocapital-creature/src/hot_reload.rs            (CreatureHotReloader)
--   - rust/crates/biocapital-pg/src/creature.rs                   (CreatureConfigRepository)
--   - rust/crates/biocapital-grpc/src/creature_service.rs         (CreatureService: ListCreatures / GetCreature / ReloadCreatures)
--   - rust/proto/biocapital.proto                                 (CreatureService / CreatureConfig / CreatureListResponse / ReloadResponse)
--
-- Idempotency: every CREATE uses IF NOT EXISTS so a partial
-- run can resume. 99 §2.2 audit invariants are preserved —
-- the gRPC `ReloadCreatures` path writes a per-row audit line
-- to `audit_creature_config` with `op` ∈
-- {creature.load, creature.reload, creature.unload,
--  creature.reload_failed, creature.reload_all}.

BEGIN;

-- ── 1. creature_configs (variant metadata cache) ───────────────────────────
--
-- One row per creature id (`config/biocapital/creatures/<id>/...`).
-- `config_json` is the full `CreatureConfig` JSON blob — the
-- Rust domain type stays the authoritative parser (13 §6.1);
-- PG is a hot cache so the gRPC `GetCreature` read path does
-- not touch disk.
--
-- `display_name_zh` / `display_name_en` are denormalised
-- cache columns so the hot `ListCreatures` path does not have
-- to parse the JSONB on every call. They mirror
-- `config_json -> 'display_name' -> 'zh_cn' / 'en_us'`.
--
-- `source_path` / `source_mtime` drive the hot-reload watcher
-- (13 §5): the reloader scans `config/biocapital/creatures/`
-- every 5 s, compares each file's mtime against the row's
-- `source_mtime`, and re-upserts on change.
--
-- `reload_failed_count` increments on every parse / validate
-- failure so the admin / Web UI can flag stale rows without
-- having to grep logs.
--
-- `enabled` toggles visibility in `ListCreatures`; the gRPC
-- `mob_replacements` (06 §3.3) lookup ignores `enabled = false`
-- rows entirely.

CREATE TABLE IF NOT EXISTS creature_configs (
    creature_id           VARCHAR(64)  PRIMARY KEY,
    config_json           JSONB        NOT NULL,
    display_name_zh       TEXT,
    display_name_en       TEXT,
    enabled               BOOLEAN      NOT NULL DEFAULT TRUE,
    last_loaded_at        TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    last_loaded_tick      BIGINT       NOT NULL,
    source_path           TEXT         NOT NULL,
    source_mtime          BIGINT       NOT NULL,
    reload_failed_count   INT          NOT NULL DEFAULT 0,
    notes                 JSONB
);

-- Hot read path: `ListCreatures(enabled_only=true)`. Partial
-- index keeps it tight even when many rows are disabled.
CREATE INDEX IF NOT EXISTS idx_creature_configs_enabled
    ON creature_configs(creature_id) WHERE enabled = TRUE;

-- Hot-reload watch path: `list_by_source_mtime(older_than)`.
-- Ordered by mtime ASC so the reloader walks files oldest-
-- first (cheap: the reloader does not need a sort).
CREATE INDEX IF NOT EXISTS idx_creature_configs_source_mtime
    ON creature_configs(source_mtime);

COMMENT ON TABLE  creature_configs IS
    'Authoritative per-creature metadata parsed from config/biocapital/creatures/<id>/creatures.json (13 §1.2 + §2 + §6.3). Hot reloaded by CreatureService.ReloadCreatures / CreatureHotReloader (5 s tick, 13 §5).';
COMMENT ON COLUMN creature_configs.creature_id IS
    'Unique creature id (snake_case). Mirrors CreatureConfig.id and mob_replacements.creature_id.';
COMMENT ON COLUMN creature_configs.config_json IS
    'Full CreatureConfig JSON projection. The Rust domain type stays the authoritative parser; this column is a hot cache.';
COMMENT ON COLUMN creature_configs.display_name_zh IS
    'Denormalised cache: display_name.zh_cn (13 §2.1). Avoids JSONB parse on every ListCreatures call.';
COMMENT ON COLUMN creature_configs.display_name_en IS
    'Denormalised cache: display_name.en_us (13 §2.1).';
COMMENT ON COLUMN creature_configs.enabled IS
    'When FALSE the row is hidden from ListCreatures and mob_replacements lookups return no-match (13 §2.2 + 06 §3.3).';
COMMENT ON COLUMN creature_configs.last_loaded_at IS
    'Wall-clock timestamp of the most recent successful upsert.';
COMMENT ON COLUMN creature_configs.last_loaded_tick IS
    'Server tick at the most recent successful upsert (BIGINT, Sable logical tick counter, NOT millis).';
COMMENT ON COLUMN creature_configs.source_path IS
    'Absolute path of the creatures.json file the row was parsed from (13 §1.2).';
COMMENT ON COLUMN creature_configs.source_mtime IS
    'File mtime (epoch seconds) of source_path at last successful load. Drives the 5 s hot-reload tick (13 §5.1).';
COMMENT ON COLUMN creature_configs.reload_failed_count IS
    'Cumulative parse / validate failures for this row since last successful load. Surfaced in Web UI to flag stale configs.';
COMMENT ON COLUMN creature_configs.notes IS
    'Free-form notes (e.g. validation warning messages).';

-- ── 2. audit_creature_config (creature-load audit) ──────────────────────────
--
-- Append-only log of every hot-reload mutation. Per 99 §2.2
-- the gRPC layer writes one row per:
--   * successful upsert (op = creature.load on first insert /
--     creature.reload on subsequent upsert)
--   * delete (op = creature.unload when the .json file is
--     removed from disk)
--   * failure (op = creature.reload_failed when parse /
--     validate throws)
--   * full re-scan (op = creature.reload_all, one row at the
--     end of a ReloadCreatures RPC)
--
-- `before_json` / `after_json` are NULL on failures and on
-- `creature.reload_all` (those carry only a notes count).
-- `request_id` is the gRPC caller's idempotency key (proto
-- `Empty` carries none, so this is `NULL` for hot-reload
-- ticks and UUIDv4 for manual admin paths).

CREATE TABLE IF NOT EXISTS audit_creature_config (
    log_id               UUID         PRIMARY KEY,
    actor_uuid           UUID,
    actor_type           VARCHAR(16)  NOT NULL
                         CHECK (actor_type IN ('RUST_SERVICE', 'ADMIN_CMD')),
    target_creature_id   VARCHAR(64),
    op                   VARCHAR(32)  NOT NULL
                         CHECK (op IN (
                           'creature.load',
                           'creature.reload',
                           'creature.unload',
                           'creature.reload_failed',
                           'creature.reload_all'
                         )),
    before_json          JSONB,
    after_json           JSONB,
    tick_millis          BIGINT       NOT NULL,
    request_id           UUID,
    notes                JSONB
);

-- Lookup-by-creature, newest-first (admin "what happened to
-- this creature" view).
CREATE INDEX IF NOT EXISTS idx_audit_creature_config_target_time
    ON audit_creature_config(target_creature_id, tick_millis DESC);

-- Lookup-by-op, newest-first (admin "show me all reload
-- failures in the last hour" view).
CREATE INDEX IF NOT EXISTS idx_audit_creature_config_op_time
    ON audit_creature_config(op, tick_millis DESC);

COMMENT ON TABLE  audit_creature_config IS
    'Append-only audit of CreatureService / CreatureHotReloader mutations (13 §6.2 + 99 §2.2).';
COMMENT ON COLUMN audit_creature_config.log_id IS
    'Synthetic UUID PK for the audit row.';
COMMENT ON COLUMN audit_creature_config.actor_uuid IS
    'Operator UUID (NULL for hot-reload watcher ticks where the actor is the Rust service itself).';
COMMENT ON COLUMN audit_creature_config.actor_type IS
    'RUST_SERVICE for hot-reload ticks; ADMIN_CMD for manual ReloadCreatures RPC calls.';
COMMENT ON COLUMN audit_creature_config.target_creature_id IS
    'Affected creature id (NULL on creature.reload_all which is a full sweep).';
COMMENT ON COLUMN audit_creature_config.op IS
    'One of: creature.load | creature.reload | creature.unload | creature.reload_failed | creature.reload_all.';
COMMENT ON COLUMN audit_creature_config.before_json IS
    'CreatureConfig snapshot BEFORE the mutation. NULL on creature.unload + creature.reload_failed + creature.reload_all.';
COMMENT ON COLUMN audit_creature_config.after_json IS
    'CreatureConfig snapshot AFTER the mutation. NULL on creature.reload_failed + creature.reload_all.';
COMMENT ON COLUMN audit_creature_config.tick_millis IS
    'Server tick at the time of the mutation (BIGINT, Sable logical tick counter, NOT millis — see doc/06 §8 + doc/04 §X).';
COMMENT ON COLUMN audit_creature_config.request_id IS
    'Caller-supplied idempotency key. NULL for hot-reload watcher ticks (which have no caller).';
COMMENT ON COLUMN audit_creature_config.notes IS
    'Free-form notes (e.g. {"files_scanned": 7, "reloaded": 3, "failed": 1} on creature.reload_all; {"error": "..."} on creature.reload_failed).';

COMMIT;