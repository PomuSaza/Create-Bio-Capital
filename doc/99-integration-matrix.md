---
module: 99-integration-matrix
status: canonical
audience: all contributors
last_reviewed: 2026-06-14
depends_on: all-modules
---

# 模块联动矩阵（Integration Matrix）

> 本文件是**所有模块依赖关系**的唯一权威来源。
> **任何模块变更必须更新本文件**。

---

## 1. 模块依赖图

```
00-overview
   │
   ▼
01-cross-cutting-concerns ──────────────────┐
   │                                         │
   ▼                                         │
11-config-system ────────────────────────────┤
12-command-system ───────────────────────────┤
17-asset-placeholders ───────────────────────┤
   │                                         │
   ▼                                         │
16-sable-bridge ─────────────────────────────┤
   │                                         │
   ▼                                         │
14-rust-services ────────────────────────────┤
   │                                         │
   ├──► 02-player-state ────────────────────┤
   │      │                                 │
   │      ├──► 03-body-development ─────────┤
   │      │                                 │
   │      ├──► 04-core-pod ─────────────────┤
   │      │      │                          │
   │      │      ▼                          │
   │      ├──► 05-byproducts-fluids ────────┤
   │      │                                 │
   │      ├──► 06-hostile-mobs ─────────────┤
   │      │      │                          │
   │      │      ▼                          │
   │      │      13-bio-customization ──────┤
   │      │                                 │
   │      ├──► 07-environment ──────────────┤
   │      │                                 │
   │      ├──► 08-bank ─────────────────────┤ (PRIORITY)
   │      │      │                          │
   │      │      ├──► 09-contracts ────────┤
   │      │                                 │
   │      └──► 10-hardware-dglab ──────────┤
   │                                         │
   └─────────────────────────────────────────┘
                │
                ▼
            15-web-ui ───────────────────────┐
                                              │
            99-integration-matrix (本文件) ───┘
```

---

## 2. 依赖矩阵

| 模块 | depends_on | overridden_by |
|---|---|---|
| 00-overview | (root) | (none) |
| 01-cross-cutting-concerns | 00 | (none — 所有冲突以本文件为准) |
| 02-player-state | 00, 01 | — |
| 03-body-development | 00, 02 | — |
| 04-core-pod | 00, 01, 02 | — |
| 05-byproducts-fluids | 00, 01, 04 | — |
| 06-hostile-mobs | 00, 01, 02, 13 | — |
| 07-environment | 00, 02, 05 | — |
| 08-bank (PRIORITY) | 00, 01, 02, 18 | — |
| 09-contracts | 00, 01, 02, 08, 18 | — |
| 10-hardware-dglab | 00, 01, 02 | — |
| 11-config-system | 00, 01, 02, 08, 18 | — |
| 12-command-system | 00, 01, 02, 08, 18 | — |
| 13-bio-customization | 00, 01, 06 | — |
| 14-rust-services (PRIORITY) | 00, 01, 08, 10, 16, 18 | — |
| 15-web-ui | 00, 01, 08, 09, 10, 12, 14, 18 | — |
| 16-sable-bridge | 00, 14 | — |
| 17-asset-placeholders | 00, 01 | — |
| **18-tg-whitelist** (新增 2026-06-14) | 00, 01, 08 | — |
| SYSTEM_PROMPT | 00, 01, 99 | — |
| 99-integration-matrix | all-modules | — |

---

## 3. 事件总线映射

### 3.1 NeoForge 事件（Java 端）

| 事件 | 触发模块 | 监听模块 |
|---|---|---|
| `PlayerStateChangeEvent` | 02 | 03, 04, 06, 07 |
| `BodyPartChangeEvent` | 03 | 02 |
| `CorePodStateChangeEvent` | 04 | 06, 15 |
| `CorePodProductionEvent` | 04 | 15 |
| `FluidEffectEvent` | 05 | 02, 03 |
| `FluidProductionEvent` | 05 | 14 |
| `MobReplacedEvent` | 06 | — |
| `HostileAttackEvent` | 06 | 02, 03 |
| `DefeatStateEnterEvent` | 06, 07 | 02, 03 |
| `DefeatStateExitEvent` | 06, 07 | 02, 03 |
| `EnvironmentEffectEvent` | 07 | 02 |
| `CatGrassTransferEvent` | 08 | 14 |
| `BankTransactionEvent` | 08 | 09, 15 |
| `ATMInsertEvent` | 08 | 14 |
| `ATMExtractEvent` | 08 | 14 |
| `BankCardDeviceLockChangeEvent` | 08 | 14 |
| `HardwareTokenIssuedEvent` | 18 | 08, 12 |
| `HardwareTokenBoundEvent` | 18 | 08, 12 |
| `HardwareTokenExpiredEvent` | 18 | 08 |
| `HardwareTokenReplacedEvent` | 18 | 08 |
| `WhitelistReloadEvent` | 18 | 12, 15 |
| `PlayerAuthenticateEvent` | 18 | 02, 12, 15 |
| `ContractCreatedEvent` | 09 | 08, 15 |
| `ContractActivatedEvent` | 09 | 08 |
| `ContractTerminatedEvent` | 09 | 08 |
| `ContractPayoutEvent` | 09 | 08, 15 |
| `DglabStrengthChangeEvent` | 10 | 02, 15 |
| `DglabConnectionEvent` | 10 | 15 |
| `DglabConfigChangeEvent` | 10 | 12, 15 |
| `DglabOverrideEvent` | 10 | 12, 15 |
| `CreatureConfigReloadedEvent` | 13 | 06, 14 |
| `CreatureMissingAssetEvent` | 13 | 15 |
| `CommandExecutedEvent` | 12 | 14, 15 |

### 3.2 KubeJS 绑定

| KubeJS 事件 | 对应 NeoForge 事件 |
|---|---|
| `events.onPlayerStateChange` | `PlayerStateChangeEvent` |
| `events.onBodyPartChange` | `BodyPartChangeEvent` |
| `events.onCorePodProduce` | `CorePodProductionEvent` |
| `events.onBankTransaction` | `BankTransactionEvent` |
| `events.onContractCreated` | `ContractCreatedEvent` |
| `events.onDglabStrengthChange` | `DglabStrengthChangeEvent` |
| `events.onHardwareTokenBind` | `HardwareTokenBoundEvent` |
| `events.onHardwareTokenExpired` | `HardwareTokenExpiredEvent` |
| `events.onWhitelistReload` | `WhitelistReloadEvent` |
| `events.onPlayerAuthenticate` | `PlayerAuthenticateEvent` |

---

## 4. gRPC 服务映射

> **2026-06-16 task #11/12 依赖升级**：service 列表 / RPC 数 / method_id 全部**未变**；本节无功能新增。版本相关变更见 `doc/14-rust-services.md` §1.2（`tonic 0.11 → 0.14` / `prost 0.12 → 0.14`）。

| 模块 | Rust crate | gRPC service |
|---|---|---|
| 02-player-state | `biocapital-core` | `PlayerStateService` |
| 03-body-development | `biocapital-core` | `PlayerStateService` (parts field) |
| 04-core-pod | `biocapital-pod` | `CorePodService` |
| 05-byproducts-fluids | `biocapital-core` | (无独立 service，附着在 `PlayerStateService` 与 `EnvironmentService`) |
| 06-hostile-mobs | `biocapital-core` | `HostileMobService` |
| 07-environment | `biocapital-environment` | `EnvironmentService` |
| 08-bank | `biocapital-bank` | `BankService` |
| 09-contracts | `biocapital-contract` | `ContractService` |
| 10-hardware-dglab | `biocapital-dglab` | `DglabService` (8 RPC, 2026-06-14 晚 task #110) |
| 13-bio-customization | `biocapital-creature` | `CreatureService` |
| 12-command-system | `biocapital-cli` | `AuditService` (查询) |
| **18-tg-whitelist** (新增 2026-06-14) | `biocapital-bank` | `BankService` (5 个硬件 token RPC) |

---

## 5. PostgreSQL 表映射

| 模块 | 表 |
|---|---|
| 02-player-state | `player_state`, `audit_player_state` |
| 03-body-development | `body_part_development` |
| 04-core-pod | `core_pods` |
| 05-byproducts-fluids | `fluid_effects` |
| 06-hostile-mobs | `mob_replacements` |
| 07-environment | `environment_effects` |
| 08-bank | `bank_accounts`, `cat_grass_batches`, `bank_transactions`, `audit_bank` |
| 09-contracts | `contracts`, `contract_payouts` |
| 10-hardware-dglab | `dglab_tokens`, `dglab_strength_log`, `audit_dglab`, `dglab_overrides`, `player_dglab_config` |
| 12-command-system | `audit_admin` |
| 13-bio-customization | `creature_configs` |
| 14-rust-services | `audit_*` (所有审计) |
| **18-tg-whitelist** (新增 2026-06-14) | `hardware_tokens`, `audit_hardware_token` |

### 5.1 关键表 CHECK 约束与详细 schema

> 本节列示 2026-06-14 涉及决策的关键表 schema 约束补充。完整 DDL 见各模块文档。

#### 5.1.1 `body_part_development`（2026-06-14 user decision）

```sql
CREATE TABLE body_part_development (
  player_uuid   UUID    NOT NULL REFERENCES player_state (player_uuid),
  part_name     VARCHAR(16) NOT NULL
                CHECK (part_name IN (
                  'HEAD', 'NECK', 'CHEST', 'BELLY', 'GENITAL', 'BUTT',
                  'BACK', 'LEFT_ARM', 'RIGHT_ARM', 'LEFT_LEG', 'RIGHT_LEG', 'FEET'
                )),
  dev_value     FLOAT   NOT NULL DEFAULT 0.0,
  updated_tick  BIGINT  NOT NULL,
  PRIMARY KEY (player_uuid, part_name)
);
```

> 12 个枚举值取自 `03-body-development.md §1.1`。`part_name` 与 proto `PlayerState.parts` map key 一一对应。

#### 5.1.8 `mob_replacements`（2026-06-14 task #9 新增）

```sql
CREATE TABLE mob_replacements (
  mob_replacement_id          UUID         PRIMARY KEY,
  vanilla_id                  VARCHAR(64)  NOT NULL,
  creature_id                 VARCHAR(64)  NOT NULL,
  drop_chance_desire_fragment FLOAT        NOT NULL DEFAULT 0.03
                              CHECK (drop_chance_desire_fragment >= 0
                                     AND drop_chance_desire_fragment <= 1),
  enabled                     BOOLEAN      NOT NULL DEFAULT TRUE,
  priority                    INT          NOT NULL DEFAULT 0,
  tags                        TEXT[]       NOT NULL DEFAULT '{}',
  created_tick                BIGINT       NOT NULL,
  updated_tick                BIGINT       NOT NULL
);
CREATE UNIQUE INDEX idx_mob_replacements_vanilla
  ON mob_replacements(vanilla_id, creature_id);
CREATE INDEX idx_mob_replacements_enabled
  ON mob_replacements(enabled) WHERE enabled = TRUE;
CREATE INDEX idx_mob_replacements_creature
  ON mob_replacements(creature_id);
```

> Authoritative per-(vanilla_id, creature_id) routing for
> hostile-mob replacement (06 §3.3 + §5.2 + §8). Read-mostly
> at runtime; the gRPC `HostileMobService.GetDropChance` RPC
> is the only hot read path. `creature_id` is intentionally
> a string (not a hard FK) because the `creature_configs`
> table is JSONB-backed in 13 §6.3; the service layer
> validates that the creature exists on read.
>
> Tie-breaking when multiple rows match a vanilla id: the
> gRPC layer sorts by `priority DESC, mob_replacement_id DESC`
> and returns the first row's `drop_chance_desire_fragment`.
> `tags` is reserved for future world-scoped filtering
> (task #11).
>
> `created_tick` / `updated_tick` use the same i64 tick
> counter as `audit_core_pod.tick_millis` (BIGINT) — they are
> the server's logical tick counter that the Sable tick loop
> exposes (NOT millisecond timestamps).
>
> Complete DDL in
> `rust/migrations/20260614000007_hostile_mobs.sql`. This
> task synchronises §5 (`06-hostile-mobs → mob_replacements`
> table list — already present) and adds the §5.1.8 schema
> block. §3.1 / §4 / §6 stay current; the `MobReplacedEvent` /
> `HostileAttackEvent` entries were already listed.

#### 5.1.0 `audit_player_state`（2026-06-14 task #3 新增）

```sql
CREATE TABLE audit_player_state (
  audit_id      BIGSERIAL PRIMARY KEY,
  partition_key TEXT        NOT NULL,  -- 'YYYY-MM'，时序分区键
  actor_uuid    UUID,
  actor_type    TEXT        NOT NULL CHECK (actor_type IN ('PLAYER','ADMIN_CMD','RUST_SERVICE','SABLE_JNI','ENVIRONMENT','HOSTILE_MOB')),
  target_uuid   UUID        NOT NULL,
  op            TEXT        NOT NULL CHECK (op IN ('state.get','state.update','state.damage','state.pleasure','state.hunger','state.part_dev')),
  before_json   JSONB       NOT NULL,
  after_json    JSONB       NOT NULL,
  source        TEXT,                   -- proto request 字段（'zombie' / 'BERRY' / ...）
  request_id    UUID,                   -- 幂等性去重
  tick_millis   BIGINT      NOT NULL,
  at            TIMESTAMPTZ NOT NULL DEFAULT now(),
  notes_json    JSONB
);
```

> Append-only 审计（99 §2.2 强制字段：actor / target / op / before / after / tick / request_id）。
> 与 `audit_bank` / `audit_dglab` / `audit_admin` 平行；为 PlayerStateService 单独建表，避免与银行写入相互阻塞，并为后续按月分区铺路。

#### 5.1.2 `environment_effects`（2026-06-14 user decision）

```sql
CREATE TABLE environment_effects (
  effect_id        UUID    PRIMARY KEY,
  environment      VARCHAR(16) NOT NULL,  -- LAVA / SWAMP_MUD / SAND / MAGMA_BLOCK
  entity_uuid      UUID    NOT NULL,
  intensity        FLOAT   NOT NULL DEFAULT 1.0,
  duration_ticks   BIGINT  NOT NULL DEFAULT 0,  -- 0 = permanent
  tick             BIGINT  NOT NULL,
  pleasure_delta   FLOAT,
  hunger_delta     FLOAT,
  triggered_defeat BOOLEAN NOT NULL DEFAULT FALSE
);
```

> `intensity` 与 `duration_ticks` 对应 proto `EnvironmentEffectRequest` 字段（见 `14-rust-services.md §3.2`）。

#### 5.1.3 `audit_bank`（2026-06-14 task #4 新增）

```sql
CREATE TABLE audit_bank (
  log_id              UUID        PRIMARY KEY,
  actor_uuid          UUID        NOT NULL,
  actor_type          VARCHAR(16) NOT NULL
                      CHECK (actor_type IN ('PLAYER', 'ADMIN_CMD', 'RUST_SERVICE', 'HARDWARE_DGLAB')),
  target_account_uuid UUID        NOT NULL,
  op                  VARCHAR(16) NOT NULL,
  before_balance      BIGINT      NOT NULL,
  after_balance       BIGINT      NOT NULL,
  tick_millis         BIGINT      NOT NULL,
  request_id          UUID,
  notes               JSONB
);
```

> Append-only 审计（99 §2.2 强制字段：actor / target / op / before / after / tick / request_id）。
> 与 `audit_player_state` / `audit_dglab` / `audit_admin` 平行；BankService 9 个 RPC 全部写一行。
> `op` 枚举值（与 `14-rust-services.md §3.2` 的 `BankService` RPC 一一对应）：
> `bank.deposit` / `bank.withdraw` / `bank.transfer` / `bank.lock_device` /
> `bank.unlock_device` / `bank.invite` / `bank.accept_invite`。
> `target_account_uuid` 是 `bank_accounts.account_uuid`。
> `before_balance` / `after_balance` 在 lock / unlock / invite 三类无金额变更的 op 上相等（08 §3.5 + §5.3）。
> 完整 DDL 在 `rust/migrations/20260614000002_bank.sql`，表本体（`bank_accounts` /
> `cat_grass_batches` / `bank_transactions`）定义同文件。

#### 5.1.4 `core_pods` 与 `audit_core_pod`（2026-06-14 task #5 新增）

```sql
CREATE TABLE core_pods (
  world_uuid      UUID         NOT NULL,
  dimension       VARCHAR(64)  NOT NULL,
  pos_x           BIGINT       NOT NULL,
  pos_y           BIGINT       NOT NULL,
  pos_z           BIGINT       NOT NULL,
  host_uuid       UUID,
  endurance       FLOAT        NOT NULL DEFAULT 100.0
                                CHECK (endurance >= 0 AND endurance <= 100),
  recipe_cooldown BIGINT       NOT NULL DEFAULT 0,
  input_fluid_id  VARCHAR(128),
  input_fluid_mb  INT          NOT NULL DEFAULT 0,
  output_fluid_id VARCHAR(128),
  output_fluid_mb INT          NOT NULL DEFAULT 0,
  byproduct_count BIGINT       NOT NULL DEFAULT 0,
  created_tick    BIGINT       NOT NULL,
  updated_tick    BIGINT       NOT NULL,
  PRIMARY KEY (world_uuid, dimension, pos_x, pos_y, pos_z)
);
CREATE INDEX idx_core_pods_host
  ON core_pods(host_uuid) WHERE host_uuid IS NOT NULL;
CREATE INDEX idx_core_pods_updated
  ON core_pods(updated_tick DESC);

CREATE TABLE audit_core_pod (
  log_id                       UUID         PRIMARY KEY,
  actor_uuid                   UUID         NOT NULL,
  actor_type                   VARCHAR(16)  NOT NULL
                                            CHECK (actor_type IN (
                                              'PLAYER', 'ADMIN_CMD', 'RUST_SERVICE'
                                            )),
  target_pod_world_uuid        UUID         NOT NULL,
  target_pod_dimension         VARCHAR(64)  NOT NULL,
  target_pod_pos_x             BIGINT       NOT NULL,
  target_pod_pos_y             BIGINT       NOT NULL,
  target_pod_pos_z             BIGINT       NOT NULL,
  op                           VARCHAR(32)  NOT NULL
                                            CHECK (op IN (
                                              'pod.tick', 'pod.enter', 'pod.exit',
                                              'pod.stress_compute', 'pod.produce'
                                            )),
  stress_units                 FLOAT,
  rpm                          FLOAT,
  input_fluid_mb               INT,
  output_fluid_mb              INT,
  byproduct_count              BIGINT,
  endurance_after              FLOAT,
  tick_millis                  BIGINT       NOT NULL,
  request_id                   UUID,
  notes                        JSONB
);
CREATE INDEX idx_audit_core_pod_target_time
  ON audit_core_pod(
    target_pod_world_uuid, target_pod_dimension,
    target_pod_pos_x, target_pod_pos_y, target_pod_pos_z,
    tick_millis DESC
  );
CREATE INDEX idx_audit_core_pod_op_time
  ON audit_core_pod(op, tick_millis DESC);
```

> Append-only 审计（99 §2.2 强制字段：actor / target / op / before / after /
> tick / request_id）。`core_pods` 的 5 元组主键与 proto `PodIdentifier`
> 一一对应（19/20 号 task 决策）；`audit_core_pod.target_pod_*` 列组合
> 等价于该主键的平铺。
>
> `op` 枚举值与 `14-rust-services.md §3.2` 的 `CorePodService` 4 个 RPC
> 一一对应 + `pod.produce`（每次产出物品时由 `TickPod` 路径附带写入）：
> `pod.tick` / `pod.enter` / `pod.exit` / `pod.produce` /
> `pod.stress_compute`（未来 Sable JNI 直连 `computePodStress` 经
> gRPC 转发时的预留 op；当前 JNI 直连**不**写此表，仅 gRPC 转发时写）。
>
> `endurance_after` 是耐久变更后的快照；`stress_units` / `rpm` 在
> `pod.enter` / `pod.exit` 上为 NULL（这两类 op 不涉及应力计算）。
>
> 完整 DDL 在 `rust/migrations/20260614000003_core_pod.sql`，本任务同步
> §5 `04-core-pod → core_pods` 表清单（不新增 service / 事件 / 配置节）。

#### 5.1.5 `audit_dglab`（2026-06-14 task #6 新增）

```sql
CREATE TABLE audit_dglab (
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
                        'dglab.connection.close'
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
CREATE INDEX idx_audit_dglab_target_time
  ON audit_dglab (target_owner_uuid, tick_millis DESC);
CREATE INDEX idx_audit_dglab_op_time
  ON audit_dglab (op, tick_millis DESC);
CREATE INDEX idx_audit_dglab_actor_time
  ON audit_dglab (actor_uuid, tick_millis DESC);
```

> Append-only 审计（99 §2.2 强制字段：actor / target / op / before /
> after / tick / request_id）。`actor_type` 扩展 4 值（含
> `HARDWARE_DGLAB`，为硬件驱动更新的未来路径预留）。
>
> `op` 枚举值与 `14-rust-services.md §3.2` 的 `DglabService` 5 个 RPC
> 一一对应（5 个 op）：`dglab.token.generate` /
> `dglab.token.revoke` / `dglab.strength.set`（含 `client` /
> `admin_cmd` / `pleasure_change` / `biocapital_reward` 四种
> trigger_source 的 `dglab_strength_log.trigger_source` 维度） /
> `dglab.connection.open` / `dglab.connection.close`。
>
> 强度上下界 0..=200 与 `dglab_strength_log.channel_a` /
> `channel_b` 的 CHECK 约束完全一致，源自 10 §2.3 / §4.2 硬件
> 上限。`token` / `connection` 类 op 的强度列在写库时**为 NULL**；
> 真实负载落在 `notes` JSONB。
>
> 完整 DDL 在 `rust/migrations/20260614000004_dglab.sql`，本任务
> 同步 §5 `10-hardware-dglab` 表清单追加 `audit_dglab`（不新增
> service / 事件 / 配置节；§3.1 / §4 / §6 保持现有）。
> 配套表 `dglab_tokens` / `dglab_strength_log` 在 §5 表清单已
> 列出，本任务**不**改其表名 / 列名 / CHECK 约束。

#### 5.1.6 `contracts` 与 `contract_payouts`（2026-06-14 task #7 新增）

```sql
CREATE TABLE contracts (
  contract_id         UUID         PRIMARY KEY,
  master_uuid         UUID         NOT NULL,
  slave_uuid          UUID         NOT NULL,
  status              VARCHAR(16)  NOT NULL
                      CHECK (status IN ('PROPOSED', 'ACTIVE', 'TERMINATED', 'REDEEMED', 'REJECTED')),
  terms_type          VARCHAR(32)  NOT NULL,
  terms_json          JSONB        NOT NULL,
  revenue_share_pct   FLOAT        NOT NULL DEFAULT 0.0
                      CHECK (revenue_share_pct >= 0 AND revenue_share_pct <= 100),
  redemption_cost     BIGINT       NOT NULL DEFAULT 0
                      CHECK (redemption_cost >= 0),
  expires_tick        BIGINT,
  created_tick        BIGINT       NOT NULL,
  updated_tick        BIGINT       NOT NULL,
  activated_tick      BIGINT,
  terminated_tick     BIGINT,
  redeemed_tick       BIGINT,
  reason              VARCHAR(256),
  -- 防止双方同 UUID 的脏数据（domain 层先拦，CHECK 兜底）
  CONSTRAINT contracts_distinct_parties CHECK (master_uuid <> slave_uuid)
);
CREATE INDEX idx_contracts_proposer ON contracts(master_uuid);
CREATE INDEX idx_contracts_acceptor ON contracts(slave_uuid);
CREATE INDEX idx_contracts_status   ON contracts(status, updated_tick DESC);
CREATE INDEX idx_contracts_expires  ON contracts(expires_tick)
  WHERE expires_tick IS NOT NULL;

CREATE TABLE contract_payouts (
  payout_id     UUID         PRIMARY KEY,
  contract_id   UUID         NOT NULL
                 REFERENCES contracts(contract_id) ON DELETE CASCADE,
  from_account  UUID         NOT NULL,
  to_account    UUID         NOT NULL,
  amount        BIGINT       NOT NULL CHECK (amount > 0),
  reason        VARCHAR(64)  NOT NULL,
  tick_millis   BIGINT       NOT NULL,
  request_id    UUID
);
CREATE UNIQUE INDEX idx_contract_payouts_request_id
  ON contract_payouts (request_id);  -- 幂等性
CREATE INDEX idx_contract_payouts_contract_time
  ON contract_payouts (contract_id, tick_millis DESC);
```

> 完整 DDL 在 `rust/migrations/20260614000005_contracts.sql`。
> 本任务在 §5 `09-contracts` 表清单追加 `contract_payouts` 行（§5
> 原有 `contracts` 已列出），§3.1 / §4 / §6 保持现有。
>
> 命名约定：SQL 列名沿用 doc 09 §2.1 + proto `ContractProposeRequest`
> 的 `master_uuid` / `slave_uuid`；Rust 域类型 `biocapital_contract::domain::Contract`
> 的内部字段为 `proposer_uuid` / `acceptor_uuid`（任务 #7 规范要求），
> gRPC 层在请求 / 响应边界做双向映射。`request_id` UNIQUE INDEX 保证
> `RedeemContract` 重放幂等（同 `bank_transactions.request_id`）；
> contract lifecycle 不携带 `request_id`，以 `contract_id` PK 为
> 去重主键。
>
> 状态机（5 状态）由 `biocapital_contract::domain::lifecycle` 的 5 个
> 转换函数 + `compute_payout` 唯一权威驱动：
>
> - `propose_contract`  → PROPOSED
> - `accept_contract`   → ACTIVE（仅 acceptor；过期则降级为 REJECTED）
> - `reject_contract`   → REJECTED（仅 acceptor；仅 PROPOSED）
> - `terminate_contract`→ TERMINATED（任一方；仅 ACTIVE）
> - `redeem_contract`   → REDEEMED（仅 proposer；仅 ACTIVE；跨 crate
>                         调 `BankService.Transfer`）
> - 过期扫描：`list_expired(current_tick)` + `update(REJECTED)`
>   由 tokio 定时任务驱动（落地于 task #14 配置系统落地后）。

#### 5.1.7 `fluid_effects`（2026-06-14 task #8 新增）

```sql
CREATE TABLE fluid_effects (
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
CREATE INDEX idx_fluid_effects_fluid  ON fluid_effects(fluid);
CREATE INDEX idx_fluid_effects_type   ON fluid_effects(effect_type);
CREATE INDEX idx_fluid_effects_source ON fluid_effects(source);
```

> Per-source effect payload table for the 4 biocapital fluids
> (05 §1 + §4). Authoritative list of effects; the table is **read-only**
> at runtime and seeded by the migration (7 rows in
> `rust/migrations/20260614000006_fluids.sql`).
>
> The CHECK constraints mirror the Rust domain enums exactly
> (`biocapital_core::fluids::{BiocapitalFluid, FluidEffectType, FluidSource}`)
> so wire-format stays in lock-step; widening any CHECK requires the
> corresponding Rust enum widening in the same migration.
>
> Routing (per 99 §4, 05 §4 + 07 §8):
>
> - `source = PRODUCTION` rows → consumed by `biocapital-pod`'s
>   `tick_pod` path (`STRESS_BOOST` rows only) and land in
>   `audit_core_pod.stress_units`.
> - `source = CONSUMPTION` rows → consumed by
>   `PlayerStateService::add_fluid_effect` (CONSUMPTION path). The
>   gRPC layer reads via
>   `biocapital_pg::FluidRepository::get_effects_for_fluid_source`
>   and applies PLEASURE_BOOST / HUNGER_BOOST / PART_DEV_BOOST
>   (GENITAL only) to the snapshot, with `op = "state.fluid_consume"`
>   in `audit_player_state`. DEFEAT_TRIGGER fires a warning but does
>   NOT apply the punitive debuff here (that lives in the hostile-mob
>   / environment tick pipeline, 06 §2.3 / 07 §6).
> - `source = ENVIRONMENT` rows → consumed by
>   `biocapital_environment::EnvironmentService::apply_fluid_effect_environmental`
>   (ENVIRONMENT path), invoked from the gRPC `EnvironmentService`
>   when `EnvironmentEffectRequest.environment` starts with
>   `"FLUID_"` (e.g. `"FLUID_HIGH_TIDE"`). `magnitude` is scaled by
>   the proto `EnvironmentEffectRequest.intensity` (07 §8 formula).
>
> The 7 seed rows cover the canonical per-fluid effect payloads from
> 05 §4; `super_lubricant` carries exactly one `DECORATIVE` row per
> the 00 §4 user-decision override (purely decorative — does NOT alter
> Create's machine RPM cap, contrary to the original blueprint).
>
> Java side (`ModFluids.java`) is **not** modified beyond a comment
> clarifying that effect resolution is server-side; it stays focused on
> the Create fluid network integration.

#### 5.1.10 `creature_configs` + `audit_creature_config`（2026-06-14 task #11 新增）

```sql
CREATE TABLE creature_configs (
  creature_id          VARCHAR(64) PRIMARY KEY,
  config_json          JSONB        NOT NULL,
  display_name_zh      TEXT,
  display_name_en      TEXT,
  enabled              BOOLEAN      NOT NULL DEFAULT TRUE,
  last_loaded_at       TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
  last_loaded_tick     BIGINT       NOT NULL,
  source_path          TEXT         NOT NULL,
  source_mtime         BIGINT       NOT NULL,
  reload_failed_count  INT          NOT NULL DEFAULT 0,
  notes                JSONB
);
CREATE INDEX idx_creature_configs_enabled
  ON creature_configs(creature_id) WHERE enabled = TRUE;
CREATE INDEX idx_creature_configs_source_mtime
  ON creature_configs(source_mtime);

CREATE TABLE audit_creature_config (
  log_id              UUID         PRIMARY KEY,
  actor_uuid          UUID,
  actor_type          VARCHAR(16)  NOT NULL
                      CHECK (actor_type IN ('RUST_SERVICE', 'ADMIN_CMD')),
  target_creature_id  VARCHAR(64),
  op                  VARCHAR(32)  NOT NULL
                      CHECK (op IN (
                        'creature.load',
                        'creature.reload',
                        'creature.unload',
                        'creature.reload_failed',
                        'creature.reload_all'
                      )),
  before_json         JSONB,
  after_json          JSONB,
  tick_millis         BIGINT       NOT NULL,
  request_id          UUID,
  notes               JSONB
);
CREATE INDEX idx_audit_creature_config_target_time
  ON audit_creature_config(target_creature_id, tick_millis DESC);
CREATE INDEX idx_audit_creature_config_op_time
  ON audit_creature_config(op, tick_millis DESC);
```

> `creature_configs` is the hot cache + durable truth for
> per-creature metadata parsed from
> `config/biocapital/creatures/<id>/creatures.json` (13 §1.2
> + §2 + §6.3). The full `CreatureConfig` JSONB is the
> authoritative payload; `display_name_zh` / `display_name_en`
> are denormalised caches mirroring
> `config_json -> 'display_name' -> 'zh_cn' / 'en_us'` so the
> `ListCreatures` hot path does not have to parse JSONB.
>
> `source_path` / `source_mtime` drive the 5 s
> `CreatureHotReloader` watch tick (13 §5.1 / task #11).
> `reload_failed_count` increments on every parse / validate
> failure; the Web UI can flag stale rows without grepping
> logs.
>
> `enabled = FALSE` rows are hidden from `ListCreatures` and
> ignored by `mob_replacements.creature_id` lookups (13 §2.2
> + 06 §3.3).
>
> `audit_creature_config` is the per-event append-only log;
> one row per `CreatureService` mutation. 99 §2.2 fields are
> preserved: actor / actor_type / target / op / before /
> after / tick / request_id / notes. `actor_type` extends the
> 99 §2.2 baseline with the 2-value subset relevant here
> (`RUST_SERVICE` for the background hot-reload watcher ticks;
> `ADMIN_CMD` for manual `ReloadCreatures` RPC calls).
>
> `op` values:
> - `creature.load` — first successful insert (no prior PG row)
> - `creature.reload` — subsequent upsert (mtime change)
> - `creature.unload` — file vanished from disk → row deleted
> - `creature.reload_failed` — parse / validate threw; row left
>   in place, `reload_failed_count` incremented
> - `creature.reload_all` — summary row written once at the
>   end of every `ReloadCreatures` RPC; `notes` JSONB carries
>   `{files_scanned, loaded, reloaded, failed, deleted, elapsed_ms}`
>
> `tick_millis` follows the same BIGINT server-tick convention
> as `audit_core_pod.tick_millis` (NOT wall-clock millis —
> see doc/06 §8 + doc/04 §X); the Sable logical tick counter
> is exposed via the `Clock` seam in `biocapital-creature`.
>
> Complete DDL in
> `rust/migrations/20260614000010_creature_configs.sql`. This
> task synchronises §5 `13-bio-customization → creature_configs`
> table list (already present) and adds the §5.1.10 schema
> block. §3.1 (`CreatureConfigReloadedEvent` /
> `CreatureMissingAssetEvent`) / §4 (`CreatureService →
> biocapital-creature`) stay current (task #80 already added
> the events; the service mapping was already in §4). §11.1
> verification flips `13-bio-customization` Rust schema from
> 🟡 → ✅.

```sql
CREATE TABLE environment_default_rules (
  rule_id UUID PRIMARY KEY,
  environment VARCHAR(16) NOT NULL
    CHECK (environment IN ('LAVA', 'SWAMP_MUD', 'SAND', 'MAGMA_BLOCK')),
  primary_modifier VARCHAR(32) NOT NULL
    CHECK (primary_modifier IN (
      'PLEASURE_DELTA', 'HUNGER_DELTA', 'MOVEMENT_MODIFIER',
      'NO_FATAL_DAMAGE', 'TRIGGER_DEFEAT', 'VISUAL_ONLY'
    )),
  magnitude FLOAT NOT NULL DEFAULT 0.0,
  duration_ticks BIGINT NOT NULL DEFAULT 0,
  intensity_formula VARCHAR(32) NOT NULL
    CHECK (intensity_formula IN ('FIXED', 'LINEAR_DISTANCE', 'FIXED_DURATION')),
  source VARCHAR(16) NOT NULL
    CHECK (source IN ('BLOCK_CONTACT', 'FLUID_IMMERSION', 'AIR_EXPOSURE')),
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  priority INT NOT NULL DEFAULT 0,
  created_tick BIGINT NOT NULL
);
CREATE UNIQUE INDEX idx_env_default_rules_env
  ON environment_default_rules(environment) WHERE enabled = TRUE;

CREATE TABLE audit_environment (
  log_id UUID PRIMARY KEY,
  actor_uuid UUID,
  actor_type VARCHAR(16) NOT NULL
    CHECK (actor_type IN ('PLAYER', 'ADMIN_CMD', 'RUST_SERVICE', 'ENVIRONMENT')),
  target_player_uuid UUID,
  environment VARCHAR(32) NOT NULL,    -- LAVA / SWAMP_MUD / SAND / MAGMA_BLOCK / FLUID_<X>
  world_uuid UUID,
  dimension VARCHAR(64),
  pos_x BIGINT, pos_y BIGINT, pos_z BIGINT,
  intensity FLOAT,
  duration_ticks BIGINT,
  pleasure_delta FLOAT,
  hunger_delta FLOAT,
  triggered_defeat BOOLEAN NOT NULL DEFAULT FALSE,
  no_fatal_damage BOOLEAN NOT NULL DEFAULT FALSE,
  tick_millis BIGINT NOT NULL,
  request_id UUID,
  notes JSONB
);
CREATE INDEX idx_audit_environment_target_time
  ON audit_environment(target_player_uuid, tick_millis DESC);
CREATE INDEX idx_audit_environment_env_time
  ON audit_environment(environment, tick_millis DESC);
CREATE INDEX idx_audit_environment_request
  ON audit_environment(request_id) WHERE request_id IS NOT NULL;
```

> `environment_default_rules` is the canonical 4-row seed for the
> 4 de-fatalised environments (07 §2.2 / §3.2 / §4.1 / §5.1);
> read-only at runtime; `EnvironmentService.GetEnvironmentModifiers`
> reads from this table. CHECK constraints mirror the Rust domain
> enums exactly (`biocapital_environment::domain::environment`).
>
> The CHECK constraints mirror the Rust domain enums exactly
> (`biocapital_environment::domain::environment::{EnvironmentType,
> EnvironmentModifier, IntensityFormula, EnvironmentSource}`) so
> the wire-format stays in lock-step; widening any CHECK requires
> the corresponding Rust enum widening in the same migration.
>
> Tie-breaking when multiple rows match an environment: the gRPC
> layer sorts by `priority DESC, rule_id DESC` and returns the
> first row. `enabled` is the soft-delete flag; the partial
> UNIQUE index on `(environment) WHERE enabled = TRUE` keeps the
> hot read tight.
>
> `audit_environment` is the per-event append-only log (one row
> per `ApplyEnvironmentEffect` call). 99 §2.2 fields are present
> in a relaxed form: this table is the **environment**
> perspective; the matching `audit_player_state` rows
> (`op = "state.pleasure" / "state.hunger"`) carry the
> player-state perspective. `request_id` is the idempotency
> dedupe key (proto `EnvironmentEffectRequest.request_id`).
>
> `intensity` / `duration_ticks` map to the proto
> `EnvironmentEffectRequest.intensity` /
> `EnvironmentEffectRequest.duration_ticks` (07 §8 formula).
>
> `actor_type` extends the 99 §2.2 baseline with `ENVIRONMENT`
> for the Java `LivingTickEvent`-driven path; `RUST_SERVICE` is
> used when the gRPC layer's `apply_fluid_effect_environmental`
> cross-method dispatch writes the row.
>
> Complete DDL in
> `rust/migrations/20260614000008_environment.sql` (4 seed
> rows: LAVA / SWAMP_MUD / SAND / MAGMA_BLOCK). This task
> synchronises §5 `07-environment → environment_default_rules +
> audit_environment` (replaces the older `environment_effects`
> row added in §5.1.2 by a richer 2-table split that separates
> the rule seed from the per-event audit log). §3.1 / §4 / §6
> stay current; the `EnvironmentEffectEvent` entry was already
> listed.



---

## 6. 配置文件映射

| TOML 字段 | 模块 | 默认值来源 |
|---|---|---|
| `[HUD_*]` | 02-player-state | 现有 Java `Config.java` |
| `[PlayerState]` | 02-player-state | 现有 `PlayerStateAttachment.java` |
| `[PlayerState.Sources]` | 02-player-state | 现有 Java（无）→ **新增调优** |
| `[BodyDevelopment]` | 03-body-development | 蓝图原始 |
| `[BodyDevelopment.Sources]` | 03-body-development | **新增调优** |
| `[CorePod]` | 04-core-pod | 现有 `CorePodBlockEntity.java` |
| `[CorePod.Hosting]` | 04-core-pod | 现有 `CorePodBlockEntity.java` |
| `[HostileMobReplacement]` | 06-hostile-mobs | 现有 `Config.java` |
| `[Environment]` | 07-environment | **新增调优** |
| `[Bank]` | 08-bank | 现有 `BankManager.java` |
| `[Contracts]` | 09-contracts | **新增调优** |
| `[DGLAB]` | 10-hardware-dglab | **新增调优** |
| `[WebUI]` | 15-web-ui | **新增调优** |
| `[Server]` | 14-rust-services | **新增调优** |
| `[JNI]` | 16-sable-bridge | library = `"biocapital_jni"`（11 §X） |

---

## 7. 指令 ↔ Web UI 路由映射

| 指令 | Web UI 路径 |
|---|---|
| `/biocapital stats me` | `/players/me` |
| `/biocapital stats <player>` | `/players/:uuid` |
| `/biocapital bank transfer` | `/bank/transfer` |
| `/biocapital bank history` | `/bank/history` |
| `/biocapital contract list` | `/contracts` |
| `/biocapital contract info` | `/contracts/:id` |
| `/biocapital contract terminate` | `/contracts/:id` (action) |
| `/biocapital contract redeem` | `/contracts/:id/redeem` |
| `/biocapital dglab list` | `/devices` |
| `/biocapital dglab generate_token` | `/devices` (action) |
| `/biocapital dglab revoke_token` | `/devices/:token/revoke` |
| `/biocapital audit query` | `/audit/query` |
| `/biocapital config reload` | `/admin/config/reload` |

---

## 8. 资产文件映射

| 资产 | 文件位置 | 占位文件位置 |
|---|---|---|
| 核心舱贴图 | `assets/create_biocapital/textures/block/core_pod_side.png` | `.png.txt` |
| ATM 贴图 | `assets/create_biocapital/textures/block/atm_side.png` | `.png.txt` |
| 沼泽泥地贴图 | `assets/create_biocapital/textures/block/swamp_mud.png` | `.png.txt` |
| 猫草贴图 | `assets/create_biocapital/textures/item/cat_grass.png` | `.png.txt` |
| 银行卡贴图 | `assets/create_biocapital/textures/item/bank_card.png` | `.png.txt` |
| 欲望碎片贴图 | `assets/create_biocapital/textures/item/desire_fragment.png` | `.png.txt` |
| 流体贴图（4 种） | `assets/create_biocapital/textures/fluid/*.png` | `.png.txt` |
| 桶贴图（4 种） | `assets/create_biocapital/textures/item/*_bucket.png` | `.png.txt` |
| 变体生物贴图 | `config/biocapital/creatures/<id>/textures/*.png` | `.png.txt` |
| 变体生物音频 | `config/biocapital/creatures/<id>/sounds/*.ogg` | `.ogg.txt` |
| 变体生物模型 | `config/biocapital/creatures/<id>/geo/*.geo.json` | `.geo.json.txt` |
| 变体生物动画 | `config/biocapital/creatures/<id>/animations/*.animation.json` | `.animation.json.txt` |

---

## 9. 资源翻译映射

| 中文 key | 英文 key | 来源模块 |
|---|---|---|
| `item.create_biocapital.cat_grass` | 同左 | 08 |
| `item.create_biocapital.bank_card` | 同左 | 08 |
| `block.create_biocapital.core_pod` | 同左 | 04 |
| `block.create_biocapital.atm` | 同左 | 08 |
| `block.create_biocapital.swamp_mud` | 同左 | 07 |
| `fluid.create_biocapital.high_tide` | 同左 | 05 |
| `fluid.create_biocapital.super_lubricant` | 同左 | 05 |
| `fluid.create_biocapital.charm_potion` | 同左 | 05 |
| `fluid.create_biocapital.semen` | 同左 | 05 |
| `fluid.create_biocapital.lactea` (改 milk 译名) | 同左 | 05 |
| `hud.create_biocapital.pleasure` | 同左 | 02 |
| `hud.create_biocapital.hunger` | 同左 | 02 |

---

## 10. 变更影响矩阵

### 10.1 修改 PlayerState 默认值

- 影响模块：02, 03, 04, 06, 07
- 需要同步：`config/create_biocapital.toml` 的 `[PlayerState]` 节
- 必须更新：**11-config-system.md**、**99-integration-matrix.md**（本文件）

### 10.2 修改 BankManager.MAX_BALANCE

- 影响模块：08, 09, 15
- 需要同步：`config/create_biocapital.toml` 的 `[Bank]` 节、PostgreSQL `bank_accounts.max_balance` 列、gRPC `BankService.Deposit/Withdraw/Transfer`
- 必须更新：**08-bank.md**、**11-config-system.md**、**14-rust-services.md**

### 10.3 修改核心舱 SU 公式

- 影响模块：04, 14
- 需要同步：`CorePodBlockEntity.getGeneratedStress`、Rust `biocapital-pod` crate 的 stress 计算、Sable JNI entrypoint
- 必须更新：**04-core-pod.md**、**14-rust-services.md**、**16-sable-bridge.md**

### 10.4 新增变体生物

- 影响模块：06, 13, 14
- 需要同步：`config/biocapital/creatures/<id>/`、`PostgreSQL creature_configs`、`gRPC CreatureService`
- 必须更新：**06-hostile-mobs.md**、**13-bio-customization.md**、**14-rust-services.md**

### 10.5 新增银行操作

- 影响模块：08, 09, 15
- 需要同步：`bank_transactions` 表、新 gRPC endpoint、新 Web UI 路由、新指令
- 必须更新：**08-bank.md**、**09-contracts.md**（如适用）、**12-command-system.md**、**15-web-ui.md**

### 10.6 修改 `SYSTEM_PROMPT.md` / 新增 `README.md` 段 / 写入 `memory/`（2026-06-20 新增）

> **触发条件**（SYSTEM_PROMPT §20 + §24）：
> - 修改 `doc/SYSTEM_PROMPT.md`（§10.1 既有规则）
> - 修改项目根 `README.md`（任何段：项目简介 / 当前状态 / 玩家指南 / 开发者接入指南 / 贡献流程 / 许可 / 链接）
> - 新增 `memory/<slug>.md`（任何类型：user / feedback / project / reference）
> - 任何 audit 回路完成（独立审计 subagent 报告）
> - commit / push 到任何分支

- 影响模块：SYSTEM_PROMPT, CHANGELOG, README, memory, 99-integration-matrix
- 需要同步：
  - `doc/CHANGELOG.md` 追加条目（§10.2 格式）
  - `README.md`（如完成度变化 / 用户决策 / 架构变化）
  - `memory/MEMORY.md` 索引（如新增 memory）
  - `doc/99-integration-matrix.md` 本节（即 §10.6 自身）
- **必须经 §21 强制审计回路（独立审计 subagent + 必须改 = 空）才能 commit**（§23）
- **绝不** commit 到 `main`（§23.3 + §11 #16）

---

## 11. 验收矩阵

### 11.1 模块验收状态

> **2026-06-15 诚实化（task #132）**：之前的 ✅ 列只能保证"子 agent 写出了代码文件"，**不**代表编译通过 / 端到端跑通。
>
> **2026-06-16 升级（task #11/12）**：用户已确认环境**有 cargo + 完整工具链**（task #11 反问 #1 校正），Rust 端 `cargo check` / `clippy` / `test` 全部跑通。**272 passed / 0 failed** across 11/12 crates（`biocapital-jni` 是 cdylib 无 lib tests）。**端到端 run 仍标 ⬜**（需 Minecraft client 实际跑）。
>
> **3 列语义**：
> - **代码存在**：✅ = 文件已落地；⬜ = 未开始
> - **Rust 编译通过**：✅ = `cargo check` 成功；⬜ = 未跑过
> - **端到端 run**：✅ = client 实际跑过；⬜ = 未跑过

| 模块 | 文档完整 | 占位完整 | 代码存在 | Rust 编译通过 | Java 编译通过 | 端到端 run |
|---|---|---|---|---|---|---|
| 00-overview | ✅ | N/A | ✅ | N/A | N/A | N/A |
| 01-cross-cutting | ✅ | N/A | ✅ | N/A | N/A | N/A |
| 02-player-state | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ |
| 03-body-development | ✅ | N/A | ✅ | ✅ | ✅ | ⬜ |
| 04-core-pod | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ |
| 05-byproducts-fluids | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ |
| 06-hostile-mobs | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ |
| 07-environment | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ |
| 08-bank (PRIORITY) | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ |
| 09-contracts | ✅ | N/A | ✅ | ✅ | ✅ | ⬜ |
| 10-hardware-dglab | ✅ | N/A | ✅ | ✅ | ✅ | ⬜ |
| 11-config-system | ✅ | N/A | ✅ | N/A | ✅ | ⬜ |
| 12-command-system | ✅ | N/A | ✅ | N/A | ✅ | ⬜ |
| 13-bio-customization | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ |
| 14-rust-services (PRIORITY) | ✅ | N/A | ✅ | ✅ | N/A | N/A |
| 15-web-ui | ✅ | N/A | ✅ | ✅ | N/A | N/A |
| 16-sable-bridge | ✅ | N/A | ✅ | ✅ | N/A | N/A |
| 17-asset-placeholders | ✅ | N/A | ✅ | N/A | N/A | N/A |
| 18-tg-whitelist | ✅ | N/A | ✅ | ✅ | ✅ | ⬜ |
| 99-integration-matrix | ✅ | N/A | ✅ | N/A | N/A | N/A |
| SYSTEM_PROMPT | ✅ | N/A | ✅ | N/A | N/A | N/A |
| README.md（root） | ✅ | N/A | ✅ | N/A | N/A | N/A |
| memory/MEMORY.md | ✅ | N/A | ✅ | N/A | N/A | N/A |

> ✅ 完成 / ⬜ 待验证 / 🟡 部分落地 / N/A 不适用
>
> **2026-06-15（task #133/134/155 完成）**：
> - Rust 端 `cargo check --workspace` ✅ 通过（task #134 cycle 修复 + task #133 编译错 + task #155 gRPC/webui 编译错）
> - Java 端 `gradle compileJava` ✅ 通过（task #131）
> - 端到端 run **全部** ⬜（需 Minecraft client 实际跑）
> - KubeJS 绑定**全部** ⬜（玩家层 script API；非阻塞，**不**影响 mod 主流程）

### 11.2 优先实现顺序（Rust 重写）

1. **14-rust-services** —— workspace + Cargo.toml + proto 定义
2. **16-sable-bridge** —— Sable 集成 + Docker
3. **02-player-state** —— PlayerState Rust 实现 + PG 表
4. **08-bank** —— 银行账本 Rust 实现 + PG 表 + gRPC
5. **04-core-pod** —— 核心舱生产公式
6. **10-hardware-dglab** —— DG_LAB 集成
7. **09-contracts** —— 契约
8. **05-byproducts-fluids** —— 流体效果
9. **06-hostile-mobs** —— 敌对生物替换
10. **07-environment** —— 环境效果
11. **13-bio-customization** —— 生物配置
12. **15-web-ui** —— Web UI 后端
13. **12-command-system** —— 指令
14. **11-config-system** —— 配置（最后，避免反复改 toml）
15. **17-asset-placeholders** —— 占位（贯穿）

---

## 12. 文档维护规则

### 12.1 何时更新本文件

- 任何模块新增/删除/重命名
- 任何新增 gRPC service / endpoint
- 任何新增 PostgreSQL 表
- 任何新增配置字段
- 任何新增 NeoForge 事件 / KubeJS 绑定

### 12.2 更新流程

1. 修改目标模块的 .md 文件
2. 修改 `99-integration-matrix.md` 对应章节
3. 在 `CHANGELOG.md` 追加条目
4. 提交 PR（含 doc/ 目录所有改动）

---

## 13. CHANGELOG 入口

`/home/saza/IdeaProjects/create_biocapital/doc/CHANGELOG.md`（待创建）

> 每次模块变更后追加：
> ```
> ## [<模块号>] - <YYYY-MM-DD>
> ### Added / Changed / Deprecated / Removed / Fixed / Security
> - <description>
> - <reference to integration-matrix update>
> ```

## 14. AGENT 入口

`/home/saza/IdeaProjects/create_biocapital/doc/SYSTEM_PROMPT.md`（已创建）

- 包含 agent 完整 system prompt（多 agent 编排器规则）
- 启动检查清单（§13）
- 工作循环（§14）
- 完工检查（§16）
- 禁用行为（§11）
- 任何 agent 修改 doc/ 前**必读**
