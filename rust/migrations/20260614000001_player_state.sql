-- 20260614000001_player_state.sql
-- Created: 2026-06-14 (task #3, doc/02-player-state.md §4 + doc/99-integration-matrix.md §5)
--
-- Player state durable schema. PostgreSQL is the source of truth for the Rust
-- gRPC service; the Java side holds a NeoForge Attachment cache that mirrors
-- these rows. See doc/02-player-state.md §4.1 for the sync topology.
--
-- Companion code:
--   - rust/crates/biocapital-core/src/player_state.rs        (domain types)
--   - rust/crates/biocapital-pg/src/player_state.rs           (repository)
--   - rust/crates/biocapital-grpc/src/player_state_service.rs (gRPC layer)
--
-- Idempotency: every CREATE uses IF NOT EXISTS so a partial run can resume.

BEGIN;

-- ── 1. player_state (main per-player row) ────────────────────────────────

CREATE TABLE IF NOT EXISTS player_state (
    player_uuid   UUID    PRIMARY KEY,
    pleasure      FLOAT   NOT NULL DEFAULT 0.0
                  CHECK (pleasure >= 0.0 AND pleasure <= 100.0),
    hunger        FLOAT   NOT NULL DEFAULT 50.0
                  CHECK (hunger >= 0.0 AND hunger <= 100.0),
    hidden_hp     FLOAT   NOT NULL DEFAULT 20.0
                  CHECK (hidden_hp >= 1.0),
    defeat_count  INT     NOT NULL DEFAULT 0
                  CHECK (defeat_count >= 0),
    max_hunger    INT     NOT NULL DEFAULT 100
                  CHECK (max_hunger >= 1),
    low_hp_hits   INT     NOT NULL DEFAULT 0
                  CHECK (low_hp_hits >= 0),
    created_tick  BIGINT  NOT NULL,
    updated_tick  BIGINT  NOT NULL
);

COMMENT ON TABLE  player_state               IS 'Per-player state — authoritative source for Rust gRPC (02 §4.1)';
COMMENT ON COLUMN player_state.pleasure     IS '快感值 / Pleasure bar; clamped [0,100]';
COMMENT ON COLUMN player_state.hunger       IS '饱食值 / Hunger bar; clamped [0,max_hunger]';
COMMENT ON COLUMN player_state.hidden_hp    IS '隐性 HP; floor 1.0, never below (02 §3.3)';
COMMENT ON COLUMN player_state.defeat_count IS '累计战败次数 (02 §3.4); monotonic';
COMMENT ON COLUMN player_state.max_hunger   IS '饥饿值上限; belly dev raises it (03 §2)';
COMMENT ON COLUMN player_state.low_hp_hits  IS 'hidden_hp 触底次数; alias of defeat_count (02 §1.1)';

-- ── 2. body_part_development (12 values per player) ──────────────────────

CREATE TABLE IF NOT EXISTS body_part_development (
    player_uuid   UUID         NOT NULL
                  REFERENCES player_state (player_uuid) ON DELETE CASCADE,
    part_name     VARCHAR(16)  NOT NULL
                  CHECK (part_name IN (
                      'HEAD', 'NECK', 'CHEST', 'BELLY', 'GENITAL', 'BUTT',
                      'BACK', 'LEFT_ARM', 'RIGHT_ARM', 'LEFT_LEG', 'RIGHT_LEG', 'FEET'
                  )),
    dev_value     FLOAT        NOT NULL DEFAULT 0.0
                  CHECK (dev_value >= 0.0 AND dev_value <= 100.0),
    updated_tick  BIGINT       NOT NULL,
    PRIMARY KEY (player_uuid, part_name)
);

COMMENT ON TABLE  body_part_development             IS 'Per-part development (03 §1.1 / 99 §5.1.1)';
COMMENT ON COLUMN body_part_development.part_name   IS 'BodyPart enum name (12 values, 03 §1.1)';
COMMENT ON COLUMN body_part_development.dev_value   IS '开发度; clamped [0,100] for storage (UI may show >100%)';

-- ── 3. Indexes ───────────────────────────────────────────────────────────

-- Reverse-chronological scans for `/biocapital stats` lookups by recency.
CREATE INDEX IF NOT EXISTS idx_player_state_updated
    ON player_state (updated_tick DESC);

-- Bulk load of a player's 12 part rows; covered by the PK index, but a
-- single-column index helps when the planner can't fold the PK prefix.
CREATE INDEX IF NOT EXISTS idx_body_part_player
    ON body_part_development (player_uuid);

-- ── 4. audit_player_state (append-only audit log) ────────────────────────
--
-- One row per state mutation that flows through the gRPC service. The
-- 99-integration-matrix §2.2 audit contract applies: actor, target, op,
-- before/after JSON, tick_millis, request_id (idempotency), at.
--
-- We use a separate table (not the generic `audit_*` family) so that the
-- PlayerStateService can write into it without contending with bank /
-- contract writers, and so that time-series partitioning (by
-- partition_key = 'YYYY-MM') is straightforward later.

CREATE TABLE IF NOT EXISTS audit_player_state (
    audit_id        BIGSERIAL PRIMARY KEY,
    partition_key   TEXT        NOT NULL DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM'),
    actor_uuid      UUID,                   -- nullable: system-initiated writes
    actor_type      TEXT        NOT NULL    -- 'PLAYER' | 'ADMIN_CMD' | 'RUST_SERVICE' | 'SABLE_JNI' | 'ENVIRONMENT' | 'HOSTILE_MOB'
                  CHECK (actor_type IN ('PLAYER','ADMIN_CMD','RUST_SERVICE','SABLE_JNI','ENVIRONMENT','HOSTILE_MOB')),
    target_uuid     UUID        NOT NULL,
    op              TEXT        NOT NULL    -- 'state.get' | 'state.update' | 'state.damage' | 'state.pleasure' | 'state.hunger' | 'state.part_dev'
                  CHECK (op IN ('state.get','state.update','state.damage','state.pleasure','state.hunger','state.part_dev')),
    before_json     JSONB       NOT NULL,
    after_json      JSONB       NOT NULL,
    source          TEXT,                   -- free-form, e.g. 'zombie', 'core_pod', 'BERRY' (mirrors proto request fields)
    request_id      UUID,                   -- idempotency dedupe (proto request_id fields)
    tick_millis     BIGINT      NOT NULL,
    at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    notes_json      JSONB
);

COMMENT ON TABLE  audit_player_state          IS 'Append-only audit for PlayerStateService (99 §2.2)';
COMMENT ON COLUMN audit_player_state.op       IS 'RPC op name; matches gRPC method name suffix';
COMMENT ON COLUMN audit_player_state.before_json IS 'PlayerStateSnapshot JSON pre-mutation';
COMMENT ON COLUMN audit_player_state.after_json  IS 'PlayerStateSnapshot JSON post-mutation';

CREATE INDEX IF NOT EXISTS idx_audit_player_state_target_time
    ON audit_player_state (target_uuid, tick_millis DESC);

CREATE INDEX IF NOT EXISTS idx_audit_player_state_op_time
    ON audit_player_state (op, tick_millis DESC);

CREATE INDEX IF NOT EXISTS idx_audit_player_state_request
    ON audit_player_state (request_id)
    WHERE request_id IS NOT NULL;

-- Time-series partitioning helper: each month lives in its own logical
-- partition_key, which the application layer (or a future cron job) can
-- promote to a real PostgreSQL declarative partition.
CREATE INDEX IF NOT EXISTS idx_audit_player_state_partition
    ON audit_player_state (partition_key, tick_millis DESC);

COMMIT;
