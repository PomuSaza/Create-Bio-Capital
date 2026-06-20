-- 20260614000007_hostile_mobs.sql
-- Created: 2026-06-14 (task #9, doc/06-hostile-mobs.md + doc/99-integration-matrix.md §5.1)
--
-- Hostile-mob replacement durable schema. PostgreSQL is the
-- source of truth for the (vanilla_id, creature_id, drop_chance)
-- tuples; the Rust gRPC layer (`HostileMobService.GetDropChance`)
-- reads these rows to drive per-mob drop probability on defeat.
--
-- Companion code:
--   - rust/crates/biocapital-creature/src/domain/mob_replacement.rs (MobReplacement)
--   - rust/crates/biocapital-pg/src/mob_replacement.rs              (MobReplacementRepository)
--   - rust/crates/biocapital-grpc/src/hostile_mob_service.rs         (HostileMobService: ApplyHostileDamage / GetDropChance)
--   - rust/proto/biocapital.proto                                    (HostileMobService / CreatureIdRequest / DropChanceResponse)
--
-- Idempotency: every CREATE uses IF NOT EXISTS so a partial
-- run can resume. 99 §2.2 audit invariants are preserved —
-- the gRPC `ApplyHostileDamage` path writes its mutation row
-- to `audit_player_state` (the hostile-mob path is just a
-- `source` value in the existing DamageRequest contract;
-- see `biocapital-grpc::player_state_service::actor_type_for_source`
-- which already maps `"zombie" / "skeleton" / "creeper" / "mob"`
-- → `actor_type = "HOSTILE_MOB"`).

BEGIN;

-- ── 1. mob_replacements (variant routing table) ────────────────────────────
--
-- One row per (vanilla_id, creature_id) pair. The same vanilla
-- entity can map to multiple creature configs (e.g. A/B test,
-- seasonal variants); the gRPC layer picks the **highest-
-- priority enabled** row (see `MobReplacement::is_higher_priority_than`).
--
-- `drop_chance_desire_fragment` is the per-defeat drop probability
-- for a single Desire Fragment (06 §5.2: 3 % default, configurable
-- via the [HostileMobReplacement] TOML section; future task #11
-- surfaces it in the config UI).
--
-- `creature_id` is intentionally a string (not a hard FK) because
-- the `creature_configs` table is JSONB-backed in 13 §6.3. The
-- service layer validates that the creature exists on read.
--
-- `created_tick` / `updated_tick` use the same i64 tick counter
-- as `audit_core_pod.tick_millis` (BIGINT). They are NOT
-- millisecond timestamps — they are the server's logical tick
-- counter that the Sable tick loop exposes.

CREATE TABLE IF NOT EXISTS mob_replacements (
    mob_replacement_id           UUID         PRIMARY KEY,
    vanilla_id                   VARCHAR(64)  NOT NULL,
    creature_id                  VARCHAR(64)  NOT NULL,
    drop_chance_desire_fragment  FLOAT        NOT NULL DEFAULT 0.03
                                CHECK (drop_chance_desire_fragment >= 0
                                       AND drop_chance_desire_fragment <= 1),
    enabled                      BOOLEAN      NOT NULL DEFAULT TRUE,
    priority                     INT          NOT NULL DEFAULT 0,
    tags                         TEXT[]       NOT NULL DEFAULT '{}',
    created_tick                 BIGINT       NOT NULL,
    updated_tick                 BIGINT       NOT NULL
);

-- One row per (vanilla_id, creature_id) pair. Two rows with the
-- same vanilla_id and different creature_ids is the
-- "multiple variants" case (priority decides the winner).
CREATE UNIQUE INDEX IF NOT EXISTS idx_mob_replacements_vanilla
    ON mob_replacements(vanilla_id, creature_id);

-- Partial index — the gRPC `GetDropChance` hot path filters
-- `WHERE enabled = TRUE`. A partial index keeps it small even
-- when many rows are disabled.
CREATE INDEX IF NOT EXISTS idx_mob_replacements_enabled
    ON mob_replacements(enabled) WHERE enabled = TRUE;

-- Lookup by creature_id (used by future admin paths: "what
-- vanilla mobs does this creature replace?").
CREATE INDEX IF NOT EXISTS idx_mob_replacements_creature
    ON mob_replacements(creature_id);

COMMENT ON TABLE  mob_replacements IS
    'Vanilla → creature routing for hostile-mob replacement (06 §3.3 + §5.2 + §8). Read-mostly at runtime; the gRPC GetDropChance RPC is the only hot read path.';
COMMENT ON COLUMN mob_replacements.mob_replacement_id IS
    'Synthetic UUID PK. Stable across updates so audit rows can reference a single column.';
COMMENT ON COLUMN mob_replacements.vanilla_id IS
    'Vanilla entity type (ResourceLocation.toString form, e.g. "minecraft:zombie"). Mirrors Java EntityType.getKey(...).toString().';
COMMENT ON COLUMN mob_replacements.creature_id IS
    'CreatureConfig.id reference (the biocapital-creature crate''s CreatureConfig.id). Service-layer validated; not a hard FK because creature_configs is JSONB-backed.';
COMMENT ON COLUMN mob_replacements.drop_chance_desire_fragment IS
    'Per-defeat Desire Fragment drop probability in [0,1]. Default 0.03 (06 §5.2).';
COMMENT ON COLUMN mob_replacements.enabled IS
    'When FALSE the row is hidden from GetDropChance and the Java-side EntityJoinLevelEvent skip-list.';
COMMENT ON COLUMN mob_replacements.priority IS
    'Tie-breaker when multiple rows match the same vanilla_id. Higher wins. Defaults to 0.';
COMMENT ON COLUMN mob_replacements.tags IS
    'Free-form tags for future world-scoped filtering (task #11).';
COMMENT ON COLUMN mob_replacements.created_tick IS
    'Server tick at which the row was first inserted (BIGINT, NOT millis — the Sable logical tick counter).';
COMMENT ON COLUMN mob_replacements.updated_tick IS
    'Server tick at which the row was last updated.';

COMMIT;
