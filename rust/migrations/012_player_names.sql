-- 20260617000002_player_names.sql
-- Created: 2026-06-17 (task #5, doc/15 §4.2 / doc/08 §5.1)
--
-- Player-name cache table for Web UI `resolve_player_name_to_uuid`.
-- Populated by the BankService.Authenticate RPC (Java
-- AuthHandler.onPlayerLoggedIn sends (uuid, username) on
-- player join) and consulted by webui/handlers/bank.rs so the
-- `POST /bank/transfer` destination can be resolved by name.
--
-- See:
--   - rust/crates/biocapital-pg/src/player_name.rs    (repository)
--   - rust/crates/biocapital-webui/src/handlers/bank.rs (resolver + 5min LRU)
--   - doc/15-web-ui.md §4                              (data contract)
--
-- Schema notes:
--   * PRIMARY KEY is (player_uuid) — a player has at most one
--     current name; previous names are kept in `audit_player_name`
--     (appended to on every change by the repository).
--   * UNIQUE on LOWER(username) enforces case-insensitive
--     uniqueness (Minecraft usernames are case-sensitive on the
--     wire, but in practice Mojang's database is case-insensitive
--     and rejects two names differing only in case).
--   * `last_seen_at` is bumped on every upsert — used by an
--     optional background sweeper to age out rows that have not
--     logged in for N days (no automatic delete today).

BEGIN;

CREATE TABLE IF NOT EXISTS player_names (
    player_uuid    UUID        PRIMARY KEY,
    username       TEXT        NOT NULL,
    last_seen_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Case-insensitive uniqueness + lookup index. The unique index
-- also acts as the lookup index for `WHERE LOWER(username) = $1`,
-- so we don't need a separate non-unique index for that query.
CREATE UNIQUE INDEX IF NOT EXISTS idx_player_names_username_lower
    ON player_names (LOWER(username));

-- Optional append-only audit of name changes. The repository
-- writes one row per `upsert` whose incoming username differs
-- from the stored one; we deliberately keep it nullable so
-- first-time inserts do not pollute the audit log.
CREATE TABLE IF NOT EXISTS audit_player_name (
    audit_id        BIGSERIAL   PRIMARY KEY,
    player_uuid     UUID        NOT NULL,
    old_username    TEXT,
    new_username    TEXT        NOT NULL,
    source          TEXT        NOT NULL  -- 'auth.login' (the only writer today)
                  CHECK (source IN ('auth.login')),
    tick_millis     BIGINT      NOT NULL,
    at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_audit_player_name_target_time
    ON audit_player_name (player_uuid, tick_millis DESC);

COMMENT ON TABLE  player_names          IS
    'Player-name cache for Web UI name→UUID resolution (15 §4.2). Populated by BankService.Authenticate (Java onPlayerLoggedIn) and consultated by webui/handlers/bank.rs::resolve_player_name_to_uuid. Source of truth for display names; the authoritative UUID→account mapping lives in bank_accounts.owner_uuid.';
COMMENT ON COLUMN player_names.username IS
    'Current Minecraft username (case-preserving; lookup is LOWER())';
COMMENT ON COLUMN player_names.last_seen_at IS
    'Bumped on every upsert (player re-login). Used by the optional background sweeper (not yet implemented).';

COMMENT ON TABLE  audit_player_name     IS
    'Append-only audit of player-name changes (task #5).';
COMMENT ON COLUMN audit_player_name.source IS
    'Provenance. Today only ''auth.login'' (AuthHandler.onPlayerLoggedIn → BankService.Authenticate → player_name.upsert).';

COMMIT;
