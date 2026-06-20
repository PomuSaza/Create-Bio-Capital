-- 20260614000006_fluids.sql
-- Created: 2026-06-14 (task #8, doc/05-byproducts-fluids.md + doc/99-integration-matrix.md §5)
--
-- Fluid-effect durable schema. PostgreSQL is the source of
-- truth for the 4 biocapital fluids and their per-source effect
-- payloads; the Rust gRPC layer reads these rows to drive
-- player-state mutations and core-pod stress bookkeeping.
--
-- Companion code:
--   - rust/crates/biocapital-core/src/fluids.rs           (BiocapitalFluid / FluidEffectType / FluidSource)
--   - rust/crates/biocapital-pg/src/fluid.rs              (FluidRepository)
--   - rust/crates/biocapital-grpc/src/player_state_service.rs (consumption path: add_fluid_effect)
--   - rust/crates/biocapital-environment/src/lib.rs       (environment path: apply_fluid_effect_environmental)
--
-- Idempotency: every CREATE uses IF NOT EXISTS so a partial
-- run can resume. 99 §2.2 audit invariants are preserved
-- (the `fluid_effects` table itself is read-only at runtime;
-- mutations land in `audit_player_state` via the gRPC layer
-- and in `audit_core_pod.stress_units` for STRESS_BOOST rows).

BEGIN;

-- ── 1. fluid_effects (per-source effect payloads) ──────────────────────────
--
-- One row per (fluid, effect_type, source) triple. A single fluid can
-- therefore carry multiple effect rows; e.g. High Tide has:
--   - (PLEASURE_BOOST, CONSUMPTION, +5,  200 ticks)
--   - (STRESS_BOOST,    PRODUCTION, +2,  0 ticks)
-- while Super Lubricant carries exactly one decorative row.
--
-- The CHECK constraints mirror the Rust domain enums exactly
-- (`biocapital_core::fluids::{BiocapitalFluid, FluidEffectType, FluidSource}`)
-- so the wire-format stays in lock-step. If a new fluid is added the
-- CHECK must be widened in the same migration that updates the Rust enum.

CREATE TABLE IF NOT EXISTS fluid_effects (
    effect_id      UUID         PRIMARY KEY,
    fluid          VARCHAR(32)  NOT NULL
                   CHECK (fluid IN (
                       'high_tide', 'super_lubricant', 'charm_potion', 'semen'
                   )),
    effect_type    VARCHAR(32)  NOT NULL
                   CHECK (effect_type IN (
                       'PLEASURE_BOOST', 'HUNGER_BOOST', 'PART_DEV_BOOST',
                       'DEFEAT_TRIGGER', 'STRESS_BOOST',   'DECORATIVE'
                   )),
    magnitude      FLOAT        NOT NULL DEFAULT 0.0,
    duration_ticks BIGINT       NOT NULL DEFAULT 0,  -- 0 = instant
    source         VARCHAR(16)  NOT NULL
                   CHECK (source IN (
                       'PRODUCTION', 'CONSUMPTION', 'ENVIRONMENT'
                   )),
    created_tick   BIGINT       NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_fluid_effects_fluid
    ON fluid_effects(fluid);
CREATE INDEX IF NOT EXISTS idx_fluid_effects_type
    ON fluid_effects(effect_type);
CREATE INDEX IF NOT EXISTS idx_fluid_effects_source
    ON fluid_effects(source);

COMMENT ON TABLE  fluid_effects IS
    'Per-source fluid effect payloads (05 §1 + §4). Authoritative list of effects; rows are read-only at runtime.';
COMMENT ON COLUMN fluid_effects.effect_id IS
    'Stable per-row UUID. Used by gRPC layer for idempotent audit dispatch.';
COMMENT ON COLUMN fluid_effects.fluid IS
    'Path-only fluid id. Matches BiocapitalFluid::as_path() in rust/crates/biocapital-core/src/fluids.rs.';
COMMENT ON COLUMN fluid_effects.effect_type IS
    'FluidEffectType enum (SCREAMING_SNAKE). Same 6 values as Rust enum.';
COMMENT ON COLUMN fluid_effects.source IS
    'FluidSource enum (PRODUCTION/CONSUMPTION/ENVIRONMENT). gRPC layer dispatches on this column.';
COMMENT ON COLUMN fluid_effects.duration_ticks IS
    'Server-tick lifetime; 0 = instant (apply once, never re-tick).';

-- ── 2. Seed: default effect rows for the 4 fluids ─────────────────────────
--
-- These rows match doc/05 §4 (per-fluid effect text):
--   High Tide        : CONSUMPTION  +5 pleasure / 200 ticks; PRODUCTION +2 stress
--   Super Lubricant  : PRODUCTION   decorative no-op (00 §4 key design override)
--   Charm Potion     : CONSUMPTION  +3 part_dev (GENITAL) / 600 ticks; +10 pleasure / 400 ticks
--   Semen            : PRODUCTION   +1 part_dev (GENITAL) / instant; CONSUMPTION DEFEAT_TRIGGER (placeholder)
--
-- `gen_random_uuid()` requires pgcrypto; the migration runner in
-- rust/crates/biocapital-pg enables it via CREATE EXTENSION earlier in
-- the bootstrap. If pgcrypto is unavailable, fall back to
-- `md5(random()::text)::uuid` style generation.

INSERT INTO fluid_effects
    (effect_id, fluid, effect_type, magnitude, duration_ticks, source, created_tick)
VALUES
    (gen_random_uuid(), 'high_tide',       'PLEASURE_BOOST', 5.0,  200, 'CONSUMPTION', 0),
    (gen_random_uuid(), 'high_tide',       'STRESS_BOOST',   2.0,  0,   'PRODUCTION',  0),
    (gen_random_uuid(), 'super_lubricant', 'DECORATIVE',     0.0,  0,   'PRODUCTION',  0),  -- 00 §4: 纯装饰
    (gen_random_uuid(), 'charm_potion',    'PART_DEV_BOOST', 3.0,  600, 'CONSUMPTION', 0),
    (gen_random_uuid(), 'charm_potion',    'PLEASURE_BOOST', 10.0, 400, 'CONSUMPTION', 0),
    (gen_random_uuid(), 'semen',           'PART_DEV_BOOST', 1.0,  0,   'PRODUCTION',  0),
    (gen_random_uuid(), 'semen',           'DEFEAT_TRIGGER', 0.0,  0,   'CONSUMPTION', 0);  -- 占位

COMMIT;