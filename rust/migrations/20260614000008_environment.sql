-- 20260614000008_environment.sql
-- Created: 2026-06-14 (task #10, doc/07-environment.md + doc/99-integration-matrix.md §5.1)
--
-- Environment-effect durable schema. PostgreSQL is the source of
-- truth for the per-environment default rules (the
-- `EnvironmentService` reads these to resolve a request). The
-- Java side's `EnvironmentEffects` `LivingTickEvent` hook remains
-- a hot-path JNI dispatch that forwards each tick to
-- `EnvironmentService.ApplyEnvironmentEffect`; the Rust service
-- is the authoritative rule resolver.
--
-- Companion code:
--   - rust/crates/biocapital-environment/src/domain/environment.rs
--       (EnvironmentType / EnvironmentEffectRule / IntensityFormula
--        / DEFAULT_ENVIRONMENT_RULES in-process seed)
--   - rust/crates/biocapital-pg/src/environment.rs
--       (EnvironmentRepository trait + PgEnvironmentRepository)
--   - rust/crates/biocapital-grpc/src/environment_service.rs
--       (EnvironmentService 2 RPC: ApplyEnvironmentEffect +
--        GetEnvironmentModifiers)
--   - rust/crates/biocapital-environment/src/lib.rs
--       (apply_fluid_effect_environmental cross-method dispatch
--        for the FLUID_<X> tokens, task #8 routing preserved)
--
-- Idempotency: every CREATE uses IF NOT EXISTS so a partial run
-- can resume. 99 §2.2 audit invariants are preserved — the
-- `environment_default_rules` table is read-only at runtime
-- (seeded by the migration; 4 rows). The `audit_environment`
-- table is the per-event append-only log.

BEGIN;

-- ── 1. environment_default_rules (per-environment rule seed) ──────────────
--
-- One row per (environment) with the canonical 4 environments
-- (07 §2.2 / §3.2 / §4.1 / §5.1). The table is the
-- "default ruleset" — task #11 (config-system) will add a
-- per-player override layer via a future `create_biocapital.toml`
-- `[Environment]` section.
--
-- The CHECK constraints mirror the Rust domain enums exactly
-- (`biocapital_environment::domain::environment::{EnvironmentType,
-- EnvironmentModifier, IntensityFormula, EnvironmentSource}`)
-- so the wire-format stays in lock-step. Widening any CHECK
-- requires the corresponding Rust enum widening in the same
-- migration.
--
-- `primary_modifier` is the *only* modifier carried by the
-- rule; compound effects (e.g. swamp mud's pleasure + hunger +
-- movement) are split across multiple rows in
-- `environment_default_rules` (one row per (environment,
-- primary_modifier) tuple). The gRPC layer fans the rule into
-- the per-mutation dispatch pipeline.

CREATE TABLE IF NOT EXISTS environment_default_rules (
    rule_id            UUID         PRIMARY KEY,
    environment        VARCHAR(16)  NOT NULL
                       CHECK (environment IN ('LAVA', 'SWAMP_MUD', 'SAND', 'MAGMA_BLOCK')),
    primary_modifier   VARCHAR(32)  NOT NULL
                       CHECK (primary_modifier IN (
                           'PLEASURE_DELTA', 'HUNGER_DELTA', 'MOVEMENT_MODIFIER',
                           'NO_FATAL_DAMAGE', 'TRIGGER_DEFEAT', 'VISUAL_ONLY'
                       )),
    magnitude          FLOAT        NOT NULL DEFAULT 0.0,
    duration_ticks     BIGINT       NOT NULL DEFAULT 0,  -- 0 = instant / 1-shot
    intensity_formula  VARCHAR(32)  NOT NULL
                       CHECK (intensity_formula IN ('FIXED', 'LINEAR_DISTANCE', 'FIXED_DURATION')),
    source             VARCHAR(16)  NOT NULL
                       CHECK (source IN ('BLOCK_CONTACT', 'FLUID_IMMERSION', 'AIR_EXPOSURE')),
    enabled            BOOLEAN      NOT NULL DEFAULT TRUE,
    created_tick       BIGINT       NOT NULL
);

-- Only one enabled row per environment. The gRPC layer reads
-- `WHERE environment = $1 AND enabled = TRUE` for the hot path
-- so a partial index keeps the read tight even when the table
-- grows with admin overrides.
CREATE UNIQUE INDEX IF NOT EXISTS idx_env_default_rules_env
    ON environment_default_rules(environment) WHERE enabled = TRUE;

COMMENT ON TABLE  environment_default_rules IS
    'Per-environment default effect rules (07 §2.2 / §3.2 / §4.1 / §5.1). Authoritative seed; read-only at runtime; gRPC EnvironmentService.GetEnvironmentModifiers reads from this table.';
COMMENT ON COLUMN environment_default_rules.rule_id IS
    'Synthetic UUID PK. Stable across updates so audit rows can reference a single column.';
COMMENT ON COLUMN environment_default_rules.environment IS
    'LAVA / SWAMP_MUD / SAND / MAGMA_BLOCK. Mirrors EnvironmentType::CANONICAL in rust/crates/biocapital-environment/src/domain/environment.rs.';
COMMENT ON COLUMN environment_default_rules.primary_modifier IS
    'The single mutation this rule performs. Mirrors EnvironmentModifier (6 values) in the Rust domain enum.';
COMMENT ON COLUMN environment_default_rules.magnitude IS
    'The base magnitude of the modifier. Interpretation depends on primary_modifier: PLEASURE_DELTA → +pleasure / window; HUNGER_DELTA → ±hunger / window; MOVEMENT_MODIFIER → speed multiplier (e.g. 0.5 for swamp mud).';
COMMENT ON COLUMN environment_default_rules.duration_ticks IS
    'Window in server ticks. 20 = 1 s at 20 tps (per-second rules: LAVA / SWAMP_MUD / MAGMA_BLOCK). 72000 = 1 hour (SAND desertification, 07 §4.1).';
COMMENT ON COLUMN environment_default_rules.intensity_formula IS
    'FIXED / LINEAR_DISTANCE / FIXED_DURATION. Mirrors IntensityFormula enum (3 values) in the Rust domain enum.';
COMMENT ON COLUMN environment_default_rules.source IS
    'BLOCK_CONTACT / FLUID_IMMERSION / AIR_EXPOSURE. The gRPC layer dispatches on this column. Mirrors EnvironmentSource enum (3 values) in the Rust domain enum.';
COMMENT ON COLUMN environment_default_rules.enabled IS
    'When FALSE the row is hidden from the gRPC read path. The partial UNIQUE index uses `enabled = TRUE` to keep the read tight.';
COMMENT ON COLUMN environment_default_rules.created_tick IS
    'Server tick at which the row was first inserted (BIGINT, NOT millis — the Sable logical tick counter).';

-- ── 2. audit_environment (per-event append-only log) ──────────────────────
--
-- One row per `ApplyEnvironmentEffect` call. The 4 gRPC service
-- mutation paths write exactly one row:
--   - LAVA (FluidImmersion)         → `actor_type = "ENVIRONMENT"`
--   - SWAMP_MUD (BlockContact)      → `actor_type = "ENVIRONMENT"`
--   - SAND (BlockContact)           → `actor_type = "ENVIRONMENT"`
--   - MAGMA_BLOCK (BlockContact)    → `actor_type = "ENVIRONMENT"`
--   - FLUID_<X> (fluid token)       → `actor_type = "ENVIRONMENT"`
--                                     (cross-method dispatch through
--                                      `apply_fluid_effect_environmental`)
--   - PlayerState changes that the env effect triggers (e.g.
--     `add_pleasure`) are mirrored into `audit_player_state` with
--     `op = "state.pleasure"` and `source = "env:LAVA"` etc. — the
--     `audit_environment` row is the *environment* perspective.
--
-- The table is the per-environment audit log so the Web UI /
-- admin tools can list "every LAVA touch" without scanning
-- `audit_player_state`. 99 §2.2 强制字段 (actor / target / op /
-- before / after / tick / request_id) are present in a relaxed
-- form: `audit_environment` is keyed on the *environment*
-- dimension rather than the player-state dimension, so the
-- `actor_uuid` / `before_json` / `after_json` columns are
-- optional. The `op` / `actor_type` columns still align with
-- the 99 §2.2 contract (enforced via CHECK).

CREATE TABLE IF NOT EXISTS audit_environment (
    log_id            UUID         PRIMARY KEY,
    actor_uuid        UUID,                                       -- nullable: system-initiated writes
    actor_type        VARCHAR(16)  NOT NULL
                      CHECK (actor_type IN (
                          'PLAYER', 'ADMIN_CMD', 'RUST_SERVICE', 'ENVIRONMENT'
                      )),
    target_player_uuid UUID,
    environment       VARCHAR(16)  NOT NULL,                     -- LAVA / SWAMP_MUD / SAND / MAGMA_BLOCK / FLUID_<X>
    world_uuid        UUID,
    dimension         VARCHAR(64),
    pos_x             BIGINT,
    pos_y             BIGINT,
    pos_z             BIGINT,
    intensity         FLOAT,
    duration_ticks    BIGINT,
    pleasure_delta    FLOAT,
    hunger_delta      FLOAT,
    triggered_defeat  BOOLEAN      NOT NULL DEFAULT FALSE,
    no_fatal_damage   BOOLEAN      NOT NULL DEFAULT FALSE,
    tick_millis       BIGINT       NOT NULL,
    request_id        UUID,
    notes             JSONB
);

-- Per-target reverse-chronological scans (`WHERE target_player_uuid = $1
-- ORDER BY tick_millis DESC`) power the per-player history view.
CREATE INDEX IF NOT EXISTS idx_audit_environment_target_time
    ON audit_environment(target_player_uuid, tick_millis DESC);

-- Per-environment reverse-chronological scans (`WHERE environment = $1
-- ORDER BY tick_millis DESC`) power the per-environment dashboard
-- (e.g. "every LAVA touch in the last hour").
CREATE INDEX IF NOT EXISTS idx_audit_environment_env_time
    ON audit_environment(environment, tick_millis DESC);

-- Per-request-id idempotency lookup (proto `EnvironmentEffectRequest.request_id`).
CREATE INDEX IF NOT EXISTS idx_audit_environment_request
    ON audit_environment(request_id) WHERE request_id IS NOT NULL;

COMMENT ON TABLE  audit_environment IS
    'Append-only audit for EnvironmentService (07 §6 + 99 §2.2 contract). One row per ApplyEnvironmentEffect call. 99 §2.2 fields are present in a relaxed form: this table is the *environment* perspective; the matching `audit_player_state` rows (op="state.pleasure" / "state.hunger" / "env.fluid") carry the player-state perspective.';
COMMENT ON COLUMN audit_environment.log_id IS
    'UUIDv7 primary key.';
COMMENT ON COLUMN audit_environment.actor_type IS
    'PLAYER | ADMIN_CMD | RUST_SERVICE | ENVIRONMENT. The gRPC EnvironmentService writes `ENVIRONMENT` for the Java-tick-dispatched calls. PLAYER is reserved for future /biocapital env apply <player> admin commands (task #13).';
COMMENT ON COLUMN audit_environment.environment IS
    'LAVA / SWAMP_MUD / SAND / MAGMA_BLOCK / FLUID_<X> token. Mirrors EnvironmentType::as_str() in the Rust domain enum.';
COMMENT ON COLUMN audit_environment.intensity IS
    'The `EnvironmentEffectRequest.intensity` value (07 §8 formula multiplier; 1.0 = default).';
COMMENT ON COLUMN audit_environment.duration_ticks IS
    'The `EnvironmentEffectRequest.duration_ticks` value (0 = instant).';
COMMENT ON COLUMN audit_environment.pleasure_delta IS
    'Actual pleasure delta applied (post-clamp, post-intensity-scale).';
COMMENT ON COLUMN audit_environment.hunger_delta IS
    'Actual hunger delta applied (post-clamp, post-intensity-scale).';
COMMENT ON COLUMN audit_environment.triggered_defeat IS
    'True if this effect triggered a defeat-state entry (07 §6). Mirrors `EnvironmentEffectResponse.triggered_defeat`.';
COMMENT ON COLUMN audit_environment.no_fatal_damage IS
    'True if the rule replaced a fatal-damage tick (07 §2.1 LAVA + §3.2 SWAMP_MUD). Mirrors `audit_player_state` source = "env:LAVA" with `killed = false`.';
COMMENT ON COLUMN audit_environment.tick_millis IS
    'Server logical tick counter at write time (BIGINT). NOT a millisecond timestamp — the Sable tick loop exposes this directly.';
COMMENT ON COLUMN audit_environment.request_id IS
    'Idempotency dedupe (proto `EnvironmentEffectRequest.request_id`). Partial-indexed for fast lookups.';

-- ── 3. Seed: 4 default environment rules ──────────────────────────────────
--
-- These rows match the in-process `DEFAULT_ENVIRONMENT_RULES` constant
-- in `rust/crates/biocapital-environment/src/domain/environment.rs`.
-- The Rust gRPC layer reads the table on every
-- `GetEnvironmentModifiers` call; the constant is the *fallback*
-- for cold-start / unit-test / offline deploy paths.
--
-- `gen_random_uuid()` requires pgcrypto; the migration runner in
-- `biocapital-pg` enables it via CREATE EXTENSION earlier in the
-- bootstrap. If pgcrypto is unavailable, fall back to
-- `md5(random()::text)::uuid` style generation.

INSERT INTO environment_default_rules
    (rule_id, environment, primary_modifier, magnitude, duration_ticks,
     intensity_formula, source, enabled, created_tick)
VALUES
    -- LAVA: +1.0 pleasure / s (07 §2.2)
    (gen_random_uuid(), 'LAVA',         'PLEASURE_DELTA',  1.0,  20,    'FIXED_DURATION', 'FLUID_IMMERSION', TRUE, 0),
    -- SWAMP_MUD: −2.0 hunger / s (07 §3.2)
    (gen_random_uuid(), 'SWAMP_MUD',    'HUNGER_DELTA',   -2.0,  20,    'FIXED_DURATION', 'BLOCK_CONTACT',   TRUE, 0),
    -- SAND: −0.5 hunger / hour (07 §4.1 desertification, 72000 ticks = 1 hour at 20 tps)
    (gen_random_uuid(), 'SAND',         'HUNGER_DELTA',   -0.5,  72000, 'FIXED_DURATION', 'BLOCK_CONTACT',   TRUE, 0),
    -- MAGMA_BLOCK: +0.5 pleasure / s (07 §5.1)
    (gen_random_uuid(), 'MAGMA_BLOCK',  'PLEASURE_DELTA',  0.5,  20,    'FIXED_DURATION', 'BLOCK_CONTACT',   TRUE, 0)
ON CONFLICT DO NOTHING;

COMMIT;
