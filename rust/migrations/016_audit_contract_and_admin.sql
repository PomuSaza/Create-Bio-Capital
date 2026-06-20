-- 20260619000004_audit_contract_and_admin.sql
-- Created: 2026-06-19 (task #67 — E2E B6 /audit/query fix)
--
-- Two audit tables that were referenced by `webui::handlers::audit`
-- and `webui::handlers::admin::write_audit_row` but never had a
-- real migration. The previous fallback policy in
-- `biocapital-pg/src/audit.rs:33,39` treated them as placeholders
-- and returned empty rows. That surfaced at E2E B6 as a 500
-- (`relation "audit_contract" does not exist`) when the HTTP
-- handler ran `SELECT * FROM audit_contract` against a fresh DB.
--
-- Companion code:
--   - rust/crates/biocapital-webui/src/handlers/audit.rs
--       (uses `query_audit_table("audit_contract", ...)` /
--        `query_audit_table("audit_admin", ...)`)
--   - rust/crates/biocapital-webui/src/handlers/admin.rs:317
--       (INSERT INTO audit_admin (log_id, actor_uuid, actor_type,
--        target_uuid, target_type, op, before_json, after_json,
--        tick_millis, request_id, notes))
--   - rust/crates/biocapital-pg/src/contract.rs:175
--       (`ContractAuditEntry` — fields mirrored 1:1 below;
--        the writer in this file piggy-backs on `audit_bank`
--        for payout rows; this table covers the
--        propose/accept/reject/terminate/redeem lifecycle
--        events that are NOT bank transfers)
--
-- Schema choices (matching the 99 §2.2 audit family):
--   * PARTITION BY RANGE (tick_millis) — same pattern as
--     20260619000001_audit_monthly_partition.sql.
--   * `audit_admin` is the canonical shape the existing
--     `write_audit_row` code already INSERTs against, so this
--     migration just materialises it (no code change needed).
--   * `audit_contract` adds a `contract_id` target column
--     (the `ContractAuditEntry` Rust struct's primary
--     business key) and a CHECK on `op` to keep the
--     lifecycle verb set tight.
--
-- Idempotency: every CREATE uses IF NOT EXISTS. The new tables
-- are created ALREADY PARTITIONED so the
-- `audit_is_partitioned()` guard in
-- 20260619000001_audit_monthly_partition.sql recognises them
-- on a re-run and skips conversion (idempotent re-run safe).

BEGIN;

-- ── 1. audit_contract (lifecycle audit for slave contracts) ──
--
-- One row per lifecycle transition: propose / accept / reject /
-- terminate / redeem. The `ContractAuditWriter` in
-- `biocapital-pg::contract` currently routes payout rows to
-- `audit_bank` for cross-table correlation; this table covers
-- the 4 lifecycle events that don't go through the bank
-- (propose / accept / reject / terminate / redeem).
--
-- Column choices follow `ContractAuditEntry` (contract.rs:175):
--   log_id            UUID PK (synthetic, BIGSERIAL also fine
--                     but UUIDv7 is what the rest of the audit
--                     family uses — see `audit_bank.log_id`).
--   actor_uuid        UUID  (master or admin who initiated)
--   actor_type        VARCHAR(16)  (mirrors audit_bank.CHECK)
--   contract_id       UUID  (PK of `contracts` — the lifecycle
--                     subject; nullable so a future "system
--                     expiry" sweep can record rows where the
--                     actor is the Rust sweeper task)
--   op                VARCHAR(32)  (lifecycle verb)
--   status_before     VARCHAR(16)  (PROPOSED | ACTIVE | …)
--   status_after      VARCHAR(16)  (post-transition status)
--   before_json       JSONB  (full before row, nullable on
--                     first INSERT — i.e. propose)
--   after_json        JSONB  (full after row)
--   tick_millis       BIGINT  (server tick at the transition)
--   request_id        UUID  (idempotency key, nullable for
--                     system-driven rows)
--   notes             JSONB  (free-form, e.g. {"reason": "..."})
--   at                TIMESTAMPTZ  (audit emit time — partition
--                     key; matches the audit_player_state
--                     pattern)
--
-- The PRIMARY KEY includes `at` so PostgreSQL accepts a unique
-- constraint on a partitioned table (PG requires the partition
-- key to appear in any unique index — see
-- 20260619000001_audit_monthly_partition.sql header).

CREATE TABLE IF NOT EXISTS audit_contract (
    log_id          UUID         NOT NULL DEFAULT gen_random_uuid(),
    actor_uuid      UUID         NOT NULL,
    actor_type      VARCHAR(16)  NOT NULL
                    CHECK (actor_type IN ('PLAYER', 'ADMIN_CMD', 'RUST_SERVICE', 'SYSTEM')),
    contract_id     UUID         NOT NULL,
    op              VARCHAR(32)  NOT NULL
                    CHECK (op IN ('propose', 'accept', 'reject', 'terminate', 'redeem')),
    status_before   VARCHAR(16)
                    CHECK (status_before IS NULL OR status_before IN
                           ('PROPOSED', 'ACTIVE', 'TERMINATED', 'REDEEMED', 'REJECTED')),
    status_after    VARCHAR(16)  NOT NULL
                    CHECK (status_after IN
                           ('PROPOSED', 'ACTIVE', 'TERMINATED', 'REDEEMED', 'REJECTED')),
    before_json     JSONB,
    after_json      JSONB        NOT NULL,
    tick_millis     BIGINT       NOT NULL,
    request_id      UUID,
    notes           JSONB,
    at              TIMESTAMPTZ  NOT NULL DEFAULT now(),
    PRIMARY KEY (log_id, at)
) PARTITION BY RANGE (at);

-- Default partition (catch-all; promoted to monthly by
-- `audit_ensure_monthly_partition()` after 20260619000001 lands).
CREATE TABLE IF NOT EXISTS audit_contract_default
    PARTITION OF audit_contract DEFAULT;

-- Lookup-by-actor (admin "what did this player do?" view).
CREATE INDEX IF NOT EXISTS idx_audit_contract_actor_time
    ON audit_contract (actor_uuid, at DESC);
-- Lookup-by-contract (admin "history of this contract" view).
CREATE INDEX IF NOT EXISTS idx_audit_contract_target_time
    ON audit_contract (contract_id, at DESC);
-- Lookup-by-op (admin "all rejects in the last hour" view).
CREATE INDEX IF NOT EXISTS idx_audit_contract_op_time
    ON audit_contract (op, at DESC);
-- Idempotency dedupe on the rare caller-supplied request_id.
CREATE INDEX IF NOT EXISTS idx_audit_contract_request_id
    ON audit_contract (request_id)
    WHERE request_id IS NOT NULL;
-- Time-range scans (the 99 §2.2 pagination path).
CREATE INDEX IF NOT EXISTS idx_audit_contract_tick_millis
    ON audit_contract (tick_millis DESC);

COMMENT ON TABLE  audit_contract IS
    'Append-only audit for ContractService lifecycle events (09 §3.1 + 99 §2.2). Payouts go to audit_bank for cross-table correlation; this table covers propose/accept/reject/terminate/redeem.';
COMMENT ON COLUMN audit_contract.log_id        IS 'Synthetic UUIDv7 PK; includes `at` because PG requires partition key in unique constraints on partitioned tables.';
COMMENT ON COLUMN audit_contract.actor_uuid    IS 'Player or admin that initiated the transition (master / slave / admin / system).';
COMMENT ON COLUMN audit_contract.actor_type    IS 'PLAYER | ADMIN_CMD | RUST_SERVICE | SYSTEM.';
COMMENT ON COLUMN audit_contract.contract_id   IS 'FK into contracts.contract_id (lifecycle subject).';
COMMENT ON COLUMN audit_contract.op            IS 'propose | accept | reject | terminate | redeem.';
COMMENT ON COLUMN audit_contract.status_before IS 'Contract status pre-transition; NULL on first INSERT (propose).';
COMMENT ON COLUMN audit_contract.status_after  IS 'Contract status post-transition.';
COMMENT ON COLUMN audit_contract.before_json   IS 'Full contract row before the transition; NULL on propose.';
COMMENT ON COLUMN audit_contract.after_json    IS 'Full contract row after the transition.';
COMMENT ON COLUMN audit_contract.tick_millis   IS 'Server tick (ms) at the transition.';
COMMENT ON COLUMN audit_contract.request_id    IS 'Caller-supplied idempotency key (nullable for system-driven rows).';
COMMENT ON COLUMN audit_contract.notes         IS 'Free-form context (e.g. termination reason).';
COMMENT ON COLUMN audit_contract.at            IS 'Audit emit timestamp; partition key (matches 20260619000001_audit_monthly_partition.sql).';

-- ── 2. audit_admin (admin / system command audit) ──
--
-- One row per privileged command issued through the Web UI
-- (grant_viewer / whitelist_reload / config_reload) or the
-- CLI backup cron (system.backup / system.pg_dump). Column
-- shape is exactly what `webui::handlers::admin::write_audit_row`
-- at line 317 already INSERTs against — this migration just
-- materialises the table so the INSERT no longer falls back
-- to audit_bank.

CREATE TABLE IF NOT EXISTS audit_admin (
    log_id        UUID         NOT NULL DEFAULT gen_random_uuid(),
    actor_uuid    UUID         NOT NULL,
    actor_type    VARCHAR(16)  NOT NULL
                  CHECK (actor_type IN ('PLAYER', 'ADMIN_CMD', 'RUST_SERVICE', 'SYSTEM')),
    target_uuid   UUID,
    target_type   VARCHAR(16),
    op            VARCHAR(64)  NOT NULL
                  CHECK (op IN (
                      'admin.grant_viewer',
                      'admin.whitelist_reload',
                      'admin.config_reload',
                      'system.backup',
                      'system.pg_dump',
                      'system.config_reload',
                      'system.partition_rollover'
                  )),
    before_json   JSONB,
    after_json    JSONB,
    tick_millis   BIGINT       NOT NULL,
    request_id    UUID,
    notes         JSONB,
    at            TIMESTAMPTZ  NOT NULL DEFAULT now(),
    PRIMARY KEY (log_id, at)
) PARTITION BY RANGE (at);

CREATE TABLE IF NOT EXISTS audit_admin_default
    PARTITION OF audit_admin DEFAULT;

-- Actor-centric view (admin "everything this operator did").
CREATE INDEX IF NOT EXISTS idx_audit_admin_actor_time
    ON audit_admin (actor_uuid, at DESC);
-- Op-centric view (admin "all whitelist_reload events").
CREATE INDEX IF NOT EXISTS idx_audit_admin_op_time
    ON audit_admin (op, at DESC);
-- Target-centric view (e.g. "what was done to this player's
-- token?" — useful for viewer-token rotation forensics).
CREATE INDEX IF NOT EXISTS idx_audit_admin_target_time
    ON audit_admin (target_uuid, at DESC)
    WHERE target_uuid IS NOT NULL;
-- Idempotency dedupe (NULL-tolerant partial index keeps it
-- small — most admin ops don't carry a request_id).
CREATE INDEX IF NOT EXISTS idx_audit_admin_request_id
    ON audit_admin (request_id)
    WHERE request_id IS NOT NULL;
-- Time-range scans for the 99 §2.2 pagination path.
CREATE INDEX IF NOT EXISTS idx_audit_admin_tick_millis
    ON audit_admin (tick_millis DESC);

COMMENT ON TABLE  audit_admin IS
    'Append-only audit for privileged Web UI / CLI commands (15 §0 + 99 §2.2 + 12 §6). Replaces the audit_bank fallback in biocapital-webui::handlers::admin::write_audit_row.';
COMMENT ON COLUMN audit_admin.log_id      IS 'Synthetic UUIDv7 PK; includes `at` because PG requires partition key in unique constraints on partitioned tables.';
COMMENT ON COLUMN audit_admin.actor_uuid  IS 'Operator UUID (admin player or system).';
COMMENT ON COLUMN audit_admin.actor_type  IS 'PLAYER | ADMIN_CMD | RUST_SERVICE | SYSTEM.';
COMMENT ON COLUMN audit_admin.target_uuid IS 'Subject of the operation (e.g. the player who received a viewer token); NULL for system-wide ops like config_reload.';
COMMENT ON COLUMN audit_admin.target_type IS 'Free-form tag (PLAYER, WHITELIST, CONFIG, BACKUP_FILE, …).';
COMMENT ON COLUMN audit_admin.op          IS 'admin.grant_viewer | admin.whitelist_reload | admin.config_reload | system.backup | system.pg_dump | system.config_reload | system.partition_rollover.';
COMMENT ON COLUMN audit_admin.before_json IS 'Snapshot of the affected state pre-mutation (nullable when no meaningful "before").';
COMMENT ON COLUMN audit_admin.after_json  IS 'Snapshot of the affected state post-mutation.';
COMMENT ON COLUMN audit_admin.tick_millis IS 'Server tick (ms) at the mutation.';
COMMENT ON COLUMN audit_admin.request_id  IS 'Idempotency key (nullable for ops without one).';
COMMENT ON COLUMN audit_admin.notes       IS 'Free-form context (e.g. ttl_seconds, expires_at, files_scanned).';
COMMENT ON COLUMN audit_admin.at          IS 'Audit emit timestamp; partition key.';

COMMIT;
