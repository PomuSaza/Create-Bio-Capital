-- 20260614000009_hardware_token.sql
-- Created: 2026-06-14 (task #83, doc/18-tg-whitelist.md)
--
-- Hardware-token durable schema. The Rust `BankService` exposes
-- 5 RPCs (RequestHardwareToken / BindHardware / ListHardware /
-- RevokeHardware / Authenticate) that drive this table.
--
-- Companion code:
--   - rust/crates/biocapital-bank/src/domain/hardware_token.rs
--       (HardwareToken + HardwareTokenStatus + 30-day / 3-slot constants)
--   - rust/crates/biocapital-bank/src/domain/whitelist.rs
--       (Whitelist in-memory cache; the whitelist file itself
--        lives at <minecraft_dir>/config/biocapital-whitelist.toml
--        per 01 §1.1; this migration does NOT add a PG table for
--        the whitelist — the toml is the source of truth)
--   - rust/crates/biocapital-pg/src/hardware_token.rs
--       (HardwareTokenRepository trait + Pg impl + audit writer)
--   - rust/crates/biocapital-grpc/src/bank_service.rs
--       (5 new RPCs on BankService: methods 9..=13)
--
-- Idempotency: every CREATE uses IF NOT EXISTS so a partial run can
-- resume. The 99 §2.2 audit invariants are preserved — the
-- audit_hardware_token table is append-only.

BEGIN;

-- ── 1. hardware_tokens (per-player slot table) ────────────────────────────
--
-- One row per (owner, token). The PK is `token_id` (UUIDv4
-- minted by the gRPC layer on `RequestHardwareToken`).
--
-- The 3-slot FIFO cap is enforced by the `trg_enforce_hardware_token_limit`
-- trigger below. The Rust repository never has to count rows
-- for cap enforcement — the trigger is the single source of
-- truth (18 §5.1 + §5.2).
--
-- `status` CHECK mirrors the Rust `HardwareTokenStatus` enum
-- exactly so wire-format stays in lock-step. Widening the CHECK
-- requires widening the Rust enum in the same migration.

CREATE TABLE IF NOT EXISTS hardware_tokens (
    token_id          UUID         PRIMARY KEY,
    owner_uuid        UUID         NOT NULL,
    hardware_id_hash  VARCHAR(64),  -- 52 base32 chars + padding to 64
                                     -- 18 §4.2; NULL = not yet bound
    status            VARCHAR(16)  NOT NULL
                      CHECK (status IN (
                          'ACTIVE', 'BOUND', 'EXPIRED', 'REPLACED', 'REVOKED'
                      )),
    issued_at         TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    expires_at        TIMESTAMPTZ  NOT NULL,
    bound_at          TIMESTAMPTZ,
    replaced_at       TIMESTAMPTZ,
    revoked_at        TIMESTAMPTZ
);

-- Per-owner reverse-chronological scans
-- (`WHERE owner_uuid = $1 AND status NOT IN (...)`).
CREATE INDEX IF NOT EXISTS idx_hardware_tokens_owner
    ON hardware_tokens(owner_uuid, status);

-- 18 §4.3: same `hardware_id_hash` cannot bind to two different
-- `owner_uuid`s. Partial UNIQUE on (owner, hash) where
-- status = 'BOUND' keeps the constraint tight without affecting
-- historical / replaced rows.
CREATE UNIQUE INDEX IF NOT EXISTS idx_hardware_tokens_owner_hash_bound
    ON hardware_tokens(owner_uuid, hardware_id_hash)
    WHERE status = 'BOUND' AND hardware_id_hash IS NOT NULL;

-- The expiry sweep (`expire_overdue`) reads
-- `WHERE expires_at < $1 AND status IN ('ACTIVE', 'BOUND')`
-- to mark overdue rows in one pass.
CREATE INDEX IF NOT EXISTS idx_hardware_tokens_expiring
    ON hardware_tokens(expires_at) WHERE status IN ('ACTIVE', 'BOUND');

COMMENT ON TABLE  hardware_tokens IS
    'Per-player hardware-token slot table (18 §1.2 + §5.1). FIFO 3-slot cap enforced by trg_enforce_hardware_token_limit.';
COMMENT ON COLUMN hardware_tokens.token_id IS
    'UUIDv4 PK. Minted by the gRPC RequestHardwareToken RPC.';
COMMENT ON COLUMN hardware_tokens.owner_uuid IS
    'Player UUID (references player_state.player_uuid logically; no FK to avoid migration ordering).';
COMMENT ON COLUMN hardware_tokens.hardware_id_hash IS
    'SHA-256 + base32 of "<raw_serial>|<machine_uuid>|<os_version>" (18 §4.2). 52 chars base32 = 64 chars column. NULL = issued but never bound.';
COMMENT ON COLUMN hardware_tokens.status IS
    'ACTIVE | BOUND | EXPIRED | REPLACED | REVOKED. Mirrors HardwareTokenStatus enum.';
COMMENT ON COLUMN hardware_tokens.issued_at IS
    'Server-side mint time. Set by DEFAULT NOW() on insert.';
COMMENT ON COLUMN hardware_tokens.expires_at IS
    'issued_at + 30d. The expire_overdue sweep marks anything past this as EXPIRED.';
COMMENT ON COLUMN hardware_tokens.bound_at IS
    'When the player first pasted the token (BindHardware RPC). NULL for never-bound rows.';
COMMENT ON COLUMN hardware_tokens.replaced_at IS
    'When the FIFO trigger evicted this row. NULL for live rows.';
COMMENT ON COLUMN hardware_tokens.revoked_at IS
    'When the player / admin explicitly revoked this row via RevokeHardware. NULL for live rows.';

-- ── 2. FIFO slot-cap trigger (18 §5.2) ───────────────────────────────────
--
-- Before each INSERT, count the owner's non-terminal rows. If
-- the count is already at the 3-slot cap, evict the oldest
-- non-terminal row by flipping its status to REPLACED.
--
-- "Oldest" = earliest `issued_at` (18 §5.1 — "删除最早绑定时间
-- `bound_at` 最小的那条" — the doc text says bound_at but the
-- table design uses `issued_at` for FIFO ordering because
-- `bound_at` is NULL for not-yet-bound rows; 18 §3.2 only ever
-- inserts a fresh token in `status = ACTIVE` first, so the bound
-- sequence aligns with the issued sequence. **Open question —
-- see task #83 反问 §1**).
--
-- This trigger is the single source of truth for the 3-slot
-- cap. The Rust repository never has to count rows; the only
-- call it makes is `INSERT INTO hardware_tokens (...)`.

CREATE OR REPLACE FUNCTION enforce_hardware_token_limit() RETURNS TRIGGER AS $$
DECLARE
    token_count INT;
    oldest_id   UUID;
BEGIN
    SELECT COUNT(*) INTO token_count
      FROM hardware_tokens
     WHERE owner_uuid = NEW.owner_uuid
       AND status NOT IN ('REPLACED', 'REVOKED', 'EXPIRED');

    IF token_count >= 3 THEN
        SELECT token_id INTO oldest_id
          FROM hardware_tokens
         WHERE owner_uuid = NEW.owner_uuid
           AND status NOT IN ('REPLACED', 'REVOKED', 'EXPIRED')
         ORDER BY issued_at ASC
         LIMIT 1;
        UPDATE hardware_tokens
           SET status = 'REPLACED', replaced_at = NOW()
         WHERE token_id = oldest_id;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_enforce_hardware_token_limit ON hardware_tokens;
CREATE TRIGGER trg_enforce_hardware_token_limit
    BEFORE INSERT ON hardware_tokens
    FOR EACH ROW
    EXECUTE FUNCTION enforce_hardware_token_limit();

COMMENT ON FUNCTION enforce_hardware_token_limit() IS
    'PG-layer FIFO slot cap for hardware_tokens (18 §5.2). One row per INSERT; if owner already has 3 non-terminal rows, the oldest is flipped to REPLACED.';
COMMENT ON TRIGGER trg_enforce_hardware_token_limit ON hardware_tokens IS
    'BEFORE INSERT — flips the oldest non-terminal row to REPLACED when the owner is at the 3-slot cap.';

-- ── 3. audit_hardware_token (per-event append-only log) ───────────────────
--
-- One row per token lifecycle event (request / bind / expire /
-- revoke / replace / fifo_evict). 99 §2.2 fields are present:
-- actor_uuid / actor_type / target_token_id / target_owner_uuid
-- / op / hardware_id_hash / reason / tick_millis / request_id.
-- The table has no `before` / `after` JSONB columns — the
-- hardware-token lifecycle has no meaningful before/after state
-- snapshot, so we keep the table narrow.

CREATE TABLE IF NOT EXISTS audit_hardware_token (
    log_id             UUID         PRIMARY KEY,
    actor_uuid         UUID,
    actor_type         VARCHAR(16)  NOT NULL
                       CHECK (actor_type IN (
                           'PLAYER', 'ADMIN_CMD', 'RUST_SERVICE'
                       )),
    target_token_id    UUID,
    target_owner_uuid  UUID,
    op                 VARCHAR(32)  NOT NULL
                       CHECK (op IN (
                           'token.request', 'token.bind',
                           'token.expire', 'token.revoke',
                           'token.replace', 'token.fifo_evict',
                           'auth.whitelist_pass', 'auth.hardware_pass',
                           'auth.deny'
                       )),
    hardware_id_hash   VARCHAR(64),
    reason             TEXT,
    tick_millis        BIGINT       NOT NULL,
    request_id         UUID
);

-- Per-target reverse-chronological scans
-- (`WHERE target_owner_uuid = $1 ORDER BY tick_millis DESC`)
-- power the per-player history view.
CREATE INDEX IF NOT EXISTS idx_audit_hardware_token_target_time
    ON audit_hardware_token(target_owner_uuid, tick_millis DESC);

-- Per-op reverse-chronological scans
-- (`WHERE op = $1 ORDER BY tick_millis DESC`)
-- power the per-op dashboard (e.g. "every fifo_evict in the last day").
CREATE INDEX IF NOT EXISTS idx_audit_hardware_token_op_time
    ON audit_hardware_token(op, tick_millis DESC);

COMMENT ON TABLE  audit_hardware_token IS
    'Append-only audit for the hardware-token lifecycle (18 §9.2 + 99 §2.2 contract). One row per token event plus per-Authenticate op.';
COMMENT ON COLUMN audit_hardware_token.actor_type IS
    'PLAYER | ADMIN_CMD | RUST_SERVICE. Mirrors 99 §2.2 actor_type enum (3 of 4 values used here; HARDWARE_DGLAB is reserved for the dglab service).';
COMMENT ON COLUMN audit_hardware_token.op IS
    'token.request | token.bind | token.expire | token.revoke | token.replace | token.fifo_evict | auth.whitelist_pass | auth.hardware_pass | auth.deny. The auth.* trio covers the Authenticate RPC outcomes from 18 §3.1.';
COMMENT ON COLUMN audit_hardware_token.tick_millis IS
    'Server tick at write time (BIGINT). Mirrors 99 §2.2 tick_millis convention (the Sable logical tick counter).';
COMMENT ON COLUMN audit_hardware_token.request_id IS
    'Idempotency dedupe (proto AuthenticateRequest / BindHardwareRequest; not yet carried in proto, reserved for the gRPC follow-up).';

COMMIT;
