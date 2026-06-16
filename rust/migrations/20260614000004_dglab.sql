-- 20260614000004_dglab.sql (rewrite #3, 2026-06-14 晚 user task #110)
--
-- 第 THREE 次回溯修正（task #6 → task #97 → task #110）。
-- task #6   写 0..=200 per channel
-- task #97  改 0..=100 per channel （错误；DGLab 官方 v2/v3 都是 0..=200）
-- task #110 改回 0..=200 per channel + 新增 dglab_overrides / player_dglab_config
--
-- 参考 doc/10-hardware-dglab.md §4.4（canonical）+
-- doc/10 §2.5（v2 websocket「强度设置到指定值」+ v3 蓝牙「通道强度
-- 设定值」+「通道强度软上限」三处都明确 0~200）。
--
-- 本次重写要点（5 张表）：
--   1. dglab_tokens          —— 0..=200 per channel
--   2. dglab_strength_log    —— 0..=200 per channel + 6 值 trigger_source
--                              (PLEASURE_CHANGE / DAMAGE_TRIGGER / ADMIN_OVERRIDE /
--                               BIOCAPITAL_REWARD / IDLE / CLIENT)
--   3. audit_dglab           —— 0..=200 per channel + 9 值 op 枚举
--                              (含 dglab.override.issue / dglab.override.expire / dglab.config.set)
--   4. dglab_overrides       —— 新增；OP 临时覆写
--   5. player_dglab_config   —— 新增；玩家自设 base/max/waveform
--
-- wire 0..=200 == PG 0..=200，不需要 wire_to_pg / pg_to_wire 转换层。
--
-- Companion code (task #110 同步):
--   - rust/crates/biocapital-dglab/src/strength.rs (新)
--   - rust/crates/biocapital-dglab/src/override.rs (新)
--   - rust/crates/biocapital-dglab/src/config.rs   (新)
--   - rust/crates/biocapital-pg/src/dglab.rs        (DglabRepository + DglabOverrideRepository + PlayerDglabConfigRepository)
--   - rust/crates/biocapital-grpc/src/dglab_service.rs (DglabService 5 RPC → 8 RPC)
--   - rust/proto/biocapital.proto                   (DglabService 8 RPC + 3 新 message)
--
-- Idempotency: 沿用 99 §2.2 audit 约束；IF NOT EXISTS 模式可从
-- 部分运行恢复。

BEGIN;

-- === dglab_tokens（保持 0..=200 per channel）===
--
-- One row per issued token. `token_id` 是 UUID PK。`enabled` 是软
-- 撤销标志；下面的部分唯一索引保证「每玩家至多一个 enabled token」
-- （10 §4.1）。
--
-- `target_id` 来自 WebSocket `bind` 消息（10 §2.2）；
-- `max_strength_a` / `max_strength_b` 是玩家在 `player_dglab_config`
-- 设定的当前 max（与硬件 0..=200 范围一致）；`connected_at` /
-- `last_pulse_at` 给 gRPC 层「app 还在活动吗」廉价信号。

CREATE TABLE IF NOT EXISTS dglab_tokens (
    token_id        UUID         PRIMARY KEY,
    owner_uuid      UUID         NOT NULL,
    target_id       VARCHAR(64),
    max_strength_a  INT          NOT NULL DEFAULT 200
                                  CHECK (max_strength_a >= 0 AND max_strength_a <= 200),
    max_strength_b  INT          NOT NULL DEFAULT 200
                                  CHECK (max_strength_b >= 0 AND max_strength_b <= 200),
    enabled         BOOLEAN      NOT NULL DEFAULT TRUE,
    connected_at    TIMESTAMPTZ,
    last_pulse_at   TIMESTAMPTZ,
    created_tick    BIGINT       NOT NULL
);

COMMENT ON TABLE  dglab_tokens                IS 'DG_LAB token registry (10 §5, rewrite 2026-06-14 晚 task #110)';
COMMENT ON COLUMN dglab_tokens.token_id       IS 'Token UUID primary key';
COMMENT ON COLUMN dglab_tokens.owner_uuid     IS 'Player UUID who owns the token';
COMMENT ON COLUMN dglab_tokens.target_id      IS 'App-provided identifier from WebSocket bind (10 §2.2)';
COMMENT ON COLUMN dglab_tokens.max_strength_a IS 'Channel A max (player_dglab_config.max_intensity mirror; 0..=200, 10 §2.5)';
COMMENT ON COLUMN dglab_tokens.max_strength_b IS 'Channel B max (0..=200, 10 §2.5)';
COMMENT ON COLUMN dglab_tokens.enabled        IS 'Soft-revocation flag; false = disabled';
COMMENT ON COLUMN dglab_tokens.connected_at   IS 'Wall-clock of last WebSocket bind (10 §2.2)';
COMMENT ON COLUMN dglab_tokens.last_pulse_at  IS 'Wall-clock of most recent hardware pulse';
COMMENT ON COLUMN dglab_tokens.created_tick   IS 'Server tick (ms) of issuance';

-- 10 §4.1 不变式：每玩家至多一个 enabled token。
CREATE UNIQUE INDEX IF NOT EXISTS idx_dglab_tokens_owner_enabled
    ON dglab_tokens (owner_uuid) WHERE enabled = TRUE;

-- Index for target_id lookups (e.g. on `bind` retry from the same app).
CREATE INDEX IF NOT EXISTS idx_dglab_tokens_target
    ON dglab_tokens (target_id);

-- === dglab_strength_log（保持 0..=200 per channel）===
--
-- 一次强度变化一行。log 是 `DglabService.GetStrength` 快照和
-- 历史分析的 source of truth；不维护单独的 `dglab_strength_state`
-- 行，快照由最新 log 行重建。
--
-- `channel_a` / `channel_b` 范围 0..=200 per channel（10 §2.5）。
-- `waveform_a` / `waveform_b` 携带 WaveformType id（15 个官方 id
-- 见 10 §2.6 + §2.11）。
--
-- `trigger_source` 镜像 6 值枚举（10 §3.2 + §11.1）：
--   - PLEASURE_CHANGE   —— PlayerStateService.AddPleasure 副作用
--   - DAMAGE_TRIGGER    —— hidden_hp==1 或大伤害（10 §3.3 双重触发）
--   - ADMIN_OVERRIDE    —— OP /biocapital admin override
--   - BIOCAPITAL_REWARD —— 04 模块核心舱奖励
--   - IDLE              —— 静默（0 输出）
--   - CLIENT            —— 玩家 GUI 调 SetPlayerConfig

CREATE TABLE IF NOT EXISTS dglab_strength_log (
    log_id          UUID         PRIMARY KEY,
    owner_uuid      UUID         NOT NULL,
    channel_a       INT          NOT NULL
                    CHECK (channel_a >= 0 AND channel_a <= 200),
    channel_b       INT          NOT NULL
                    CHECK (channel_b >= 0 AND channel_b <= 200),
    waveform_a      VARCHAR(32),
    waveform_b      VARCHAR(32),
    trigger_source  VARCHAR(32)  NOT NULL
                    CHECK (trigger_source IN (
                      'PLEASURE_CHANGE', 'DAMAGE_TRIGGER',
                      'ADMIN_OVERRIDE', 'BIOCAPITAL_REWARD',
                      'IDLE',           'CLIENT'
                    )),
    tick_millis     BIGINT       NOT NULL,
    request_id      UUID
);

COMMENT ON TABLE  dglab_strength_log                  IS 'DG_LAB strength change history (10 §5, rewrite 2026-06-14 晚 task #110)';
COMMENT ON COLUMN dglab_strength_log.log_id          IS 'UUID primary key';
COMMENT ON COLUMN dglab_strength_log.owner_uuid      IS 'Player whose DG_LAB changed';
COMMENT ON COLUMN dglab_strength_log.channel_a       IS 'Channel A strength (0..=200) after change (10 §2.5)';
COMMENT ON COLUMN dglab_strength_log.channel_b       IS 'Channel B strength (0..=200) after change (10 §2.5)';
COMMENT ON COLUMN dglab_strength_log.waveform_a      IS 'Channel A waveform_id (10 §2.6) at emit time';
COMMENT ON COLUMN dglab_strength_log.waveform_b      IS 'Channel B waveform_id (10 §2.6) at emit time';
COMMENT ON COLUMN dglab_strength_log.trigger_source  IS 'PLEASURE_CHANGE | DAMAGE_TRIGGER | ADMIN_OVERRIDE | BIOCAPITAL_REWARD | IDLE | CLIENT (10 §3.2)';
COMMENT ON COLUMN dglab_strength_log.tick_millis     IS 'Server tick (ms) at change';
COMMENT ON COLUMN dglab_strength_log.request_id      IS 'Idempotency key (proto request_id)';

CREATE INDEX IF NOT EXISTS idx_dglab_strength_log_owner_time
    ON dglab_strength_log (owner_uuid, tick_millis DESC);

CREATE INDEX IF NOT EXISTS idx_dglab_strength_log_source_time
    ON dglab_strength_log (trigger_source, tick_millis DESC);

CREATE INDEX IF NOT EXISTS idx_dglab_strength_log_waveform_a
    ON dglab_strength_log (waveform_a) WHERE waveform_a IS NOT NULL;

-- === audit_dglab（保持 0..=200）===
--
-- 镜像 `audit_bank` / `audit_player_state` shape。`DglabService` 8 个
-- RPC 各自写一个 `op` 值；`actor_type` CHECK 含 `HARDWARE_DGLAB`
-- 为入站硬件驱动更新预留。
--
-- before/after 对覆盖 A/B 双通道；token / connection / override /
-- config 类 op 的强度列为 NULL，实际负载落在 `notes` JSONB。
--
-- `op` 是 9 值集合（10 §5 + §3.4 + §3.5）：
--   - dglab.token.generate / dglab.token.revoke
--   - dglab.strength.set
--   - dglab.connection.open / dglab.connection.close / dglab.bind
--   - dglab.override.issue / dglab.override.expire     (新)
--   - dglab.config.set                                 (新)

CREATE TABLE IF NOT EXISTS audit_dglab (
    log_id              UUID         PRIMARY KEY,
    actor_uuid          UUID         NOT NULL,
    actor_type          VARCHAR(16)  NOT NULL
                        CHECK (actor_type IN ('PLAYER', 'ADMIN_CMD', 'RUST_SERVICE', 'HARDWARE_DGLAB')),
    target_owner_uuid   UUID,
    op                  VARCHAR(32)  NOT NULL
                        CHECK (op IN (
                          'dglab.token.generate',
                          'dglab.token.revoke',
                          'dglab.strength.set',
                          'dglab.connection.open',
                          'dglab.connection.close',
                          'dglab.bind',
                          'dglab.override.issue',
                          'dglab.override.expire',
                          'dglab.config.set'
                        )),
    before_strength_a   INT
                        CHECK (before_strength_a IS NULL OR (before_strength_a >= 0 AND before_strength_a <= 200)),
    after_strength_a    INT
                        CHECK (after_strength_a  IS NULL OR (after_strength_a  >= 0 AND after_strength_a  <= 200)),
    before_strength_b   INT
                        CHECK (before_strength_b IS NULL OR (before_strength_b >= 0 AND before_strength_b <= 200)),
    after_strength_b    INT
                        CHECK (after_strength_b  IS NULL OR (after_strength_b  >= 0 AND after_strength_b  <= 200)),
    tick_millis         BIGINT       NOT NULL,
    request_id          UUID,
    notes               JSONB
);

COMMENT ON TABLE  audit_dglab                       IS 'Append-only audit for DglabService (10 §5, rewrite 2026-06-14 晚 task #110)';
COMMENT ON COLUMN audit_dglab.log_id                IS 'UUID primary key';
COMMENT ON COLUMN audit_dglab.actor_uuid            IS 'Initiator UUID (player / admin / service / hardware)';
COMMENT ON COLUMN audit_dglab.actor_type            IS 'PLAYER | ADMIN_CMD | RUST_SERVICE | HARDWARE_DGLAB';
COMMENT ON COLUMN audit_dglab.target_owner_uuid     IS 'Player whose token / strength / connection is affected';
COMMENT ON COLUMN audit_dglab.op                    IS 'dglab.token.generate | dglab.token.revoke | dglab.strength.set | dglab.connection.open | dglab.connection.close | dglab.bind | dglab.override.issue | dglab.override.expire | dglab.config.set';
COMMENT ON COLUMN audit_dglab.before_strength_a     IS 'Channel A strength pre-mutation (0..=200, 10 §2.5)';
COMMENT ON COLUMN audit_dglab.after_strength_a      IS 'Channel A strength post-mutation (0..=200, 10 §2.5)';
COMMENT ON COLUMN audit_dglab.before_strength_b     IS 'Channel B strength pre-mutation (0..=200)';
COMMENT ON COLUMN audit_dglab.after_strength_b      IS 'Channel B strength post-mutation (0..=200)';
COMMENT ON COLUMN audit_dglab.tick_millis           IS 'Server tick (ms) at audit emit';
COMMENT ON COLUMN audit_dglab.request_id            IS 'Cross-table idempotency key (proto request_id)';
COMMENT ON COLUMN audit_dglab.notes                 IS 'Optional context (token, ws_addr, target_id, source reason, override param/value)';

CREATE INDEX IF NOT EXISTS idx_audit_dglab_target_time
    ON audit_dglab (target_owner_uuid, tick_millis DESC);

CREATE INDEX IF NOT EXISTS idx_audit_dglab_op_time
    ON audit_dglab (op, tick_millis DESC);

CREATE INDEX IF NOT EXISTS idx_audit_dglab_actor_time
    ON audit_dglab (actor_uuid, tick_millis DESC);

-- === dglab_overrides（新增，OP 临时覆写）===
--
-- 10 §3.4：指令 `/biocapital admin override <user> <param>
-- <value> <duration_seconds>` 写入；过期立即恢复玩家自设。
-- 5 个 `param` 值：base / max / waveform_a / waveform_b / clear。
-- `clear` 时 value_int / value_str 都为 NULL。
--
-- 过期由 `biocapital-dglab` 的 tokio 100ms tick 扫描（与
-- scheduler 复用）—— `SELECT ... WHERE active = TRUE AND
-- expires_at < NOW()` → set active = FALSE → 写 audit_dglab
-- (op='dglab.override.expire')。

CREATE TABLE IF NOT EXISTS dglab_overrides (
    override_id          UUID         PRIMARY KEY,
    target_player_uuid   UUID         NOT NULL,
    param                VARCHAR(16)  NOT NULL
                         CHECK (param IN ('base', 'max', 'waveform_a', 'waveform_b', 'clear')),
    value_int            SMALLINT
                         CHECK (value_int IS NULL OR (value_int >= 0 AND value_int <= 200)),
    value_str            VARCHAR(32),
    issued_by            UUID         NOT NULL,
    issued_at            TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    expires_at           TIMESTAMPTZ  NOT NULL,
    active               BOOLEAN      NOT NULL DEFAULT TRUE
);

COMMENT ON TABLE  dglab_overrides                    IS 'OP temporary override (10 §3.4, new in task #110)';
COMMENT ON COLUMN dglab_overrides.override_id        IS 'Override UUID primary key';
COMMENT ON COLUMN dglab_overrides.target_player_uuid IS 'Player whose config is overridden';
COMMENT ON COLUMN dglab_overrides.param              IS 'base | max | waveform_a | waveform_b | clear';
COMMENT ON COLUMN dglab_overrides.value_int          IS 'For base/max: 0..=200; NULL for waveform_a/waveform_b/clear';
COMMENT ON COLUMN dglab_overrides.value_str          IS 'For waveform_a/waveform_b: 15 official id; NULL for base/max/clear';
COMMENT ON COLUMN dglab_overrides.issued_by          IS 'OP player UUID';
COMMENT ON COLUMN dglab_overrides.issued_at          IS 'Wall-clock of issuance';
COMMENT ON COLUMN dglab_overrides.expires_at         IS 'Wall-clock of expiry (immediately reverted after)';
COMMENT ON COLUMN dglab_overrides.active             IS 'False after sweeper expires the override';

CREATE INDEX IF NOT EXISTS idx_dglab_overrides_player_active
    ON dglab_overrides (target_player_uuid, active) WHERE active = TRUE;

CREATE INDEX IF NOT EXISTS idx_dglab_overrides_expiring
    ON dglab_overrides (expires_at) WHERE active = TRUE;

-- === player_dglab_config（新增，玩家自设）===
--
-- 10 §3.2：玩家 2 个设定 + 2 个 waveform 默认值。
--   base_intensity  默认 60   范围 0..=200
--   max_intensity   默认 80   范围 0..=200 且 ≥ base（CHECK 保证）
--   waveform_a      默认 'continuous'
--   waveform_b      默认 'pulse'
--
-- PlayerDglabConfig 是 EffectSource::compute_strength 公式 (10
-- §3.3 + §4.2) 的输入。
--
-- 没有显式 FK 到 player_state（本表是 player_dglab_config 独立
-- 表，玩家第一次调 SetPlayerConfig 时由 Rust 端 UPSERT 创行；
-- 不在 PG 层强制 player_state 存在）。

CREATE TABLE IF NOT EXISTS player_dglab_config (
    player_uuid     UUID         PRIMARY KEY,
    base_intensity  SMALLINT     NOT NULL DEFAULT 60
                    CHECK (base_intensity >= 0 AND base_intensity <= 200),
    max_intensity   SMALLINT     NOT NULL DEFAULT 80
                    CHECK (max_intensity >= 0 AND max_intensity <= 200 AND max_intensity >= base_intensity),
    waveform_a      VARCHAR(32)  NOT NULL DEFAULT 'continuous',
    waveform_b      VARCHAR(32)  NOT NULL DEFAULT 'pulse',
    updated_tick    BIGINT       NOT NULL
);

COMMENT ON TABLE  player_dglab_config                  IS 'Player self-set DG_LAB config (10 §3.2, new in task #110)';
COMMENT ON COLUMN player_dglab_config.player_uuid      IS 'Player UUID (PK)';
COMMENT ON COLUMN player_dglab_config.base_intensity   IS 'Daily base intensity (0..=200, default 60, 10 §3.2)';
COMMENT ON COLUMN player_dglab_config.max_intensity    IS 'Safety cap (0..=200, default 80, must be >= base, 10 §3.2)';
COMMENT ON COLUMN player_dglab_config.waveform_a       IS 'Channel A default waveform (15 official ids, default continuous)';
COMMENT ON COLUMN player_dglab_config.waveform_b       IS 'Channel B default waveform (15 official ids, default pulse)';
COMMENT ON COLUMN player_dglab_config.updated_tick     IS 'Server tick (ms) of last SetPlayerConfig';

COMMIT;
