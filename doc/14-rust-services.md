---
module: 14-rust-services
status: canonical — PRIORITY module
audience: all contributors
last_reviewed: 2026-06-14
depends_on: 00-overview, 01-cross-cutting-concerns, 08-bank, 10-hardware-dglab, 16-sable-bridge
priority_reason: "是其他模块的服务端锚点；与 Sable / DG_LAB / PostgreSQL 共用技术栈"
---

# Rust 服务端架构

> 本文件描述本模组的 Rust 服务端进程架构、gRPC schema、PostgreSQL schema、与 Sable 的整合方式。

---

## 1. 总览

### 1.1 项目布局

```
rust/
├── Cargo.toml                      # workspace root
├── Cargo.lock
├── README.md
├── crates/
│   ├── biocapital-core/            # 核心数据结构（PlayerState, BodyPart, etc.）
│   ├── biocapital-bank/            # 银行账本逻辑
│   ├── biocapital-contract/        # 奴隶契约逻辑
│   ├── biocapital-pod/             # 核心舱生产公式
│   ├── biocapital-environment/     # 环境效果
│   ├── biocapital-creature/        # 生物自定义
│   ├── biocapital-dglab/           # DG_LAB 集成（直连 Rust）
│   ├── biocapital-webui/           # Web UI HTTP API
│   ├── biocapital-grpc/            # gRPC server + client
│   ├── biocapital-pg/              # PostgreSQL schema + migrations
│   ├── biocapital-jni/             # JNI 桥（Java 调用入口）
│   └── biocapital-cli/             # CLI 入口（启动 / 迁移 / 备份）
├── proto/
│   └── biocapital.proto            # 共享 protobuf 定义
├── migrations/                     # sqlx migrations
└── docker/                         # Rust native 构建镜像（与 Sable 共享）
```

### 1.2 技术栈

> **2026-06-16 升级**（task #11/12）：MSRV `1.75` → `1.96`；`axum 0.7` → `0.8`；`tonic 0.11` → `0.14`；`prost 0.12` → `0.14`；`sqlx 0.7` → `0.8`；`tokio` features 由 `["full"]` 收紧为中等集（`rt-multi-thread` + `macros` + `net` + `time` + `sync` + `signal` + `fs` + `process` + `io-util`）；`async-trait` **保留**（项目重度 `Arc<dyn Trait>` 动态分发，AFIT 不 dyn-compatible；参考本次 AskUserQuestion 决策记录）。
>
> **2026-06-16 晚 task #14-#23**：build pipeline 全部修通。`rust/docker/Dockerfile` `rust:1.78-slim` → `rust:1.96-slim` + `libc6-dev-arm64-cross`；`build.gradle` configuration cache 兼容 + Windows .dll 名称修正 + cross-linker env。`./gradlew :buildRustNatives` 1m 12s 跑通 3 个 targets：linux-x86_64 (.so 916KB) / linux-aarch64 (.so 964KB) / windows-x86_64 (.dll 2.4MB)。macOS 目标（x86_64-apple-darwin / aarch64-apple-darwin）需 osxcross，2026-06-16 user decision 跳过，移入 `rustTargetMatrixMacos` secondary 矩阵。

| 层 | 库 | 验证状态 |
|---|---|---|
| MSRV | `rust-version = "1.96"`（2026-05-25 stable） | ✅ `cargo check --workspace` 0 errors / 0 warnings |
| 异步运行时 | `tokio = "1"` (中等 features 集，**非** `full`) | ✅ |
| HTTP / WebSocket | `axum = "0.8"` | ✅ |
| gRPC | `tonic = "0.14"` / `prost = "0.14"` | ✅ |
| PostgreSQL | `sqlx = "0.8"` (postgres, runtime-tokio-rustls) | ✅ |
| 序列化 | `serde`, `serde_json`, `prost`, `prost-types = "0.14"` | ✅ |
| 配置 | `figment` (支持 TOML + 环境变量) | ✅ |
| 日志 | `tracing`, `tracing-subscriber` | ✅ |
| 异步特征 | `async-trait = "0.1"`（保留原因见上） | ✅ |
| 错误处理 | `thiserror`, `anyhow` | ✅ |
| 测试 | `cargo test`, `proptest` | ✅ **272 passed / 0 failed**（11/12 crates） |
| **JNI 跨编译** | `rust/docker/Dockerfile` (rust:1.96-slim) | ✅ `build/natives/{linux-x86_64,linux-aarch64}/libbiocapital_jni.so` + `build/natives/windows-x86_64/biocapital_jni.dll` |

### 1.3 与 Sable 集成

- 详见 `16-sable-bridge.md`
- 共享 `docker/` 构建镜像
- 共享 `gradlew buildRustNatives` 流程

---

## 2. 启动流程

### 2.1 启动顺序

```
┌─────────────────────────────────────────┐
│ Java 端 (NeoForge)                      │
│  1. 读取 create_biocapital.toml        │
│  2. 启动 Rust 服务子进程                │
│  3. 等待 Rust 服务端口开放               │
│  4. 初始化 gRPC client                  │
│  5. 注册 BlockEntity / Item / ...       │
│  6. 注册指令                            │
│  7. 启动 Done                           │
└─────────────────────────────────────────┘
                  ▲
                  │ gRPC + JNI
                  ▼
┌─────────────────────────────────────────┐
│ Rust 服务端 (biocapital-cli)            │
│  1. 读取 biocapital-server.toml        │
│  2. 连接 PostgreSQL                       │
│  3. 运行 SQL 迁移文件                     │
│     └─ 来自 <minecraft_dir>/config/    │
│        biocapital/sql/ 运行时目录（D1）  │
│     └─ 启动期按文件名顺序执行            │
│  4. 启动 gRPC server (port 50051)      │
│  5. 启动 HTTP server (port 8080)        │
│     └─ 含 SSE（Web UI 用）               │
│  6. 启动每日备份定时任务                 │
│  7. 启动 Sable JNI 服务                 │
│  8. 启动资源同步服务                     │
│     └─ 周期性推 server config +         │
│        生物资源到在线玩家（D9/D16）     │
│  9. Ready                               │
└─────────────────────────────────────────┘

> **2026-06-20 D6 决策**：
> ❌ **不**启动 WebSocket server（Rust **不**连手机 WS）
> ❌ **不**作为 DG_LAB client / gateway
> ✅ Rust **仅**暴露 gRPC + HTTP + SSE（供 Web UI + 资源同步）
> ✅ MC 客户端**独立**连手机 WS（详见 `doc/10-hardware-dglab.md` §2.1）
```

### 2.2 子进程管理

- Java 端通过 `ProcessBuilder` 启动 `biocapital-server` 二进制
- 监控子进程 stdout/stderr → 日志转 `LOGGER`
- 子进程崩溃 → 自动重启（最多 3 次，间隔 30 秒）

### 2.3 冷启动时间预算

- Java 端：从 JVM 启动到「首批玩家可加入」< 10 秒
- Rust 服务：从二进制启动到「接受 gRPC」< 5 秒

---

## 3. gRPC Schema

### 3.1 Proto 文件位置

`rust/proto/biocapital.proto`（**Java 端共享**，通过 `proto-gen` Gradle 任务生成 Java stub）。

// proto-gaps-filled: 2026-06-14
// 本节字段补全规则：
//   - PodIdentifier：来源 04-core-pod.md §6.3 SQL PRIMARY KEY (world_uuid, dimension, pos_x, pos_y, pos_z)
//   - PlayerState / PlayerStateUpdate：来源 02-player-state.md §1.1 + 99-integration-matrix.md §5 `player_state` 表
//   - Bank * 字段：来源 08-bank.md §5.1 SQL（bank_accounts / bank_transactions）+ 99 §5
//   - Contract * 字段：来源 09-contracts.md §2.1 + §5.1 SQL（contracts / contract_payouts）+ 99 §3.1 事件
//   - EnvironmentMod*: 来源 07-environment.md §8.3 SQL environment_effects（按 environment 分组）
//   - TokenResponse：来源 10-hardware-dglab.md §5 SQL dglab_tokens
//   - AuditRow：来源 99-integration-matrix.md §2.2 审计日志强制字段
//   - CreatureConfig：来源 13-bio-customization.md §2 creatures.json 全字段
// ListResponse 拆分：原 ContractService.ListContracts 与 DglabService.ListTokens 共享 ListResponse，
// 元素类型不同；改为 ContractListResponse / TokenListResponse 各自具名（见末尾说明）。

### 3.2 包结构

```protobuf
syntax = "proto3";
package biocapital.v1;

import "google/protobuf/timestamp.proto";

// ── 通用 ─────────────────────────────────────
message Uuid { bytes value = 1; }              // 16-byte big-endian UUID
message PlayerIdentifier { Uuid player_uuid = 1; }

// inferred from 04-core-pod.md §6.3 (PostgreSQL core_pods PRIMARY KEY)
message PodIdentifier {
  Uuid world_uuid = 1;            // 世界唯一标识
  string dimension = 2;           // 维度 ID (overworld / the_nether / ...)
  int64 pos_x = 3;                // 方块坐标 X（与 SQL BIGINT 对齐）
  int64 pos_y = 4;                // 方块坐标 Y
  int64 pos_z = 5;                // 方块坐标 Z
}

message BlockPos {
  int32 x = 1;
  int32 y = 2;
  int32 z = 3;
}

message Empty {}

// 通用 ListRequest：limit/offset + 可选 owner 过滤（被多处复用）
message ListRequest {
  int32 limit = 1;
  int32 offset = 2;
  // 可选过滤：例如 ListContracts 可按 player 过滤
  PlayerIdentifier player_filter = 3;
}

// ── PlayerState ─────────────────────────────
service PlayerStateService {
  rpc GetState(PlayerIdentifier) returns (PlayerState);
  rpc UpdateState(PlayerStateUpdate) returns (PlayerState);
  rpc ApplyDamage(DamageRequest) returns (DamageResponse);
  rpc AddPleasure(PleasureRequest) returns (PlayerState);
  rpc AddHunger(HungerRequest) returns (PlayerState);
}

message PlayerState {
  Uuid player_uuid = 1;
  float pleasure = 2;
  float hunger = 3;
  float hidden_hp = 4;
  int32 low_hp_hits = 5;
  // key ∈ {HEAD, NECK, CHEST, BELLY, GENITAL, BUTT, BACK, LEFT_ARM, RIGHT_ARM, LEFT_LEG, RIGHT_LEG, FEET} (12 parts, see 03-body-development.md §1.1)
  map<string, float> parts = 6;  // BodyPart name -> dev value
  google.protobuf.Timestamp updated_at = 7;
  // inferred from 02 §1.1 + 99 §5 player_state 表
  int32 defeat_count = 8;        // 累计战败状态次数（02 §3.4）
  int64 active_contracts = 9;    // 当前活跃契约数（12 §2.1 指令输出）
  int32 max_hunger = 10;         // 当前 max hunger（03 §3.2 腹部开发度影响）
}

// inferred from 02 §1.1 (partial-update 风格)
// 2026-06-14 user decision: 移除 op 字段；proto3 字段存在性自动推断语义（set vs delta 由调用方约定）
message PlayerStateUpdate {
  PlayerIdentifier target = 1;
  google.protobuf.Timestamp updated_at = 2;
  // partial 字段（0 = 不修改）；caller 应只填需要改的字段
  float pleasure = 3;            // 0 = no-op
  float hunger = 4;
  float hidden_hp = 5;
  int32 low_hp_hits = 6;
  // key ∈ {HEAD, NECK, CHEST, BELLY, GENITAL, BUTT, BACK, LEFT_ARM, RIGHT_ARM, LEFT_LEG, RIGHT_LEG, FEET} (12 parts, see 03-body-development.md §1.1)
  // 语义：caller 通过填写全量（set）或部分 delta（与历史 parts 合并）由调用方约定
  map<string, float> parts = 7;  // BodyPart name -> dev value
}

message DamageRequest {
  PlayerIdentifier target = 1;
  float amount = 2;
  string source = 3;             // e.g. "zombie", "core_pod", "lava"
  string part = 4;               // BodyPart name；空 = 全身
  // inferred from 99 §2.2（request_id 用于幂等性去重）
  Uuid request_id = 5;
}

message DamageResponse {
  PlayerState new_state = 1;
  bool killed = 2;               // 对应 02 §3.4：原版死亡不复存在，始终为 false
}

message PleasureRequest {
  PlayerIdentifier target = 1;
  float amount = 2;
  string part = 3;               // BodyPart name；空 = 全身快感
  string source = 4;             // "BERRY" | "MOB_HIT" | "LAVA" | "CHARM_POTION" | "DEFEAT" | ...
  Uuid request_id = 5;
}

message HungerRequest {
  PlayerIdentifier target = 1;
  float amount = 2;              // 正数 = 喂食，负数 = 衰减
  string source = 3;             // "NATURAL_DECAY" | "FOOD" | "SWAMP" | ...
  Uuid request_id = 4;
}

// ── Bank ────────────────────────────────────
service BankService {
  rpc GetBalance(AccountRequest) returns (BalanceResponse);
  rpc Deposit(DepositRequest) returns (BalanceResponse);
  rpc Withdraw(WithdrawRequest) returns (BalanceResponse);
  rpc Transfer(TransferRequest) returns (BalanceResponse);
  rpc GetHistory(HistoryRequest) returns (HistoryResponse);
  rpc LockDevice(LockDeviceRequest) returns (LockDeviceResponse);
  rpc UnlockDevice(UnlockDeviceRequest) returns (LockDeviceResponse);
  rpc GenerateInviteCode(AccountRequest) returns (InviteCodeResponse);
  rpc AcceptInviteCode(AcceptInviteRequest) returns (LockDeviceResponse);
}

// inferred from 08 §5.1 bank_accounts SQL
message AccountRequest {
  PlayerIdentifier player = 1;   // 即 bank_accounts.owner_uuid
  string device_id = 2;          // 用于设备锁定校验（08 §3.5）
}

message BalanceResponse {
  int64 balance = 1;             // cat-grass units
  int64 max_balance = 2;         // bank_accounts.max_balance（默认 100,000,000）
  bool device_locked = 3;        // true = 此 device_id 与 bank_card.device_lock 不匹配
  Uuid account_uuid = 4;         // bank_accounts.account_uuid
}

message DepositRequest {
  PlayerIdentifier player = 1;
  int64 amount = 2;
  Uuid batch_id = 3;             // cat_grass_batches.batch_id（08 §2.2 批次号追踪）
  Uuid request_id = 4;           // 幂等性去重（08 §5.3 + 99 §2.2）
}

message WithdrawRequest {
  PlayerIdentifier player = 1;
  int64 amount = 2;
  Uuid request_id = 3;
}

message TransferRequest {
  PlayerIdentifier from = 1;
  PlayerIdentifier to   = 2;
  int64 amount = 3;
  string memo = 4;               // 可选备注（写入 bank_transactions 或 audit）
  Uuid request_id = 5;
}

message HistoryRequest {
  PlayerIdentifier player = 1;
  int32 limit = 2;               // 默认 16，与现有 Java BankMenu 一致（08 §3.4）
  int64 before_tick_millis = 3;  // 翻页游标：返回严格早于此时间的 N 条
}

message HistoryResponse {
  repeated BankTransaction entries = 1;
  int64 next_before_tick_millis = 2;  // 翻页用；-1 = 已到末尾
}

// inferred from 08 §5.1 bank_transactions SQL
message BankTransaction {
  Uuid tx_id = 1;                // bank_transactions.tx_id
  Uuid account_uuid = 2;         // bank_transactions.account_uuid
  string op = 3;                 // "DEPOSIT" | "WITHDRAW" | "TRANSFER_OUT" | "TRANSFER_IN"
  int64 amount = 4;
  int64 balance_after = 5;       // bank_transactions.balance_after
  Uuid counterparty_uuid = 6;    // bank_transactions.counterparty_uuid（可空）
  string counterparty_name = 7;  // bank_transactions.counterparty_name
  int64 tick_millis = 8;         // bank_transactions.tick_millis
  Uuid request_id = 9;           // bank_transactions.request_id
}

message LockDeviceRequest {
  PlayerIdentifier player = 1;
  string device_id = 2;          // 写入 bank_card.device_lock（08 §3.5）
}

message LockDeviceResponse {
  bool success = 1;
  bool already_locked = 2;       // 已被其他 device 锁定
  string current_device_id = 3;  // 当前 device_lock（便于 UI 展示）
}

message UnlockDeviceRequest {
  PlayerIdentifier player = 1;
  string device_id = 2;
  string invite_code = 3;        // 08 §3.5 邀请码机制
}

message InviteCodeResponse {
  string invite_code = 1;        // UUIDv4（08 §3.5）
  google.protobuf.Timestamp expires_at = 2;  // 默认 10 分钟（12 §2.7）
}

message AcceptInviteRequest {
  PlayerIdentifier player = 1;
  string invite_code = 2;
}

// ── Contract ────────────────────────────────
service ContractService {
  rpc ProposeContract(ContractProposeRequest) returns (ContractResponse);
  rpc AcceptContract(ContractAcceptRequest) returns (ContractResponse);
  rpc RejectContract(ContractRejectRequest) returns (ContractResponse);
  rpc TerminateContract(ContractTerminateRequest) returns (ContractResponse);
  rpc RedeemContract(ContractRedeemRequest) returns (ContractResponse);
  rpc GetContract(ContractRequest) returns (ContractResponse);
  rpc ListContracts(ListRequest) returns (ContractListResponse);
}

// inferred from 09 §2.1 + §5.1 contracts SQL
message ContractProposeRequest {
  PlayerIdentifier master  = 1;  // contracts.master_uuid
  PlayerIdentifier slave   = 2;  // contracts.slave_uuid
  string terms_type = 3;         // terms.type: "DAILY_WAGE_TRANSFER" | "MINING_QUOTA" | "BREEDING_CONSENT" | "CUSTOM"
  string terms_json = 4;         // terms.parameters JSON 文本（contracts.terms JSONB）
  float revenue_share_pct = 5;   // contracts.revenue_share_pct (0..100)
  int64 redemption_cost = 6;     // contracts.redemption_cost (cat grass)
  int64 expires_tick = 7;        // contracts.expires_tick（0 = 永久）
  Uuid request_id = 8;           // 幂等性
}

message ContractAcceptRequest   { Uuid contract_id = 1; Uuid request_id = 2; }
message ContractRejectRequest   { Uuid contract_id = 1; string reason = 2; Uuid request_id = 3; }
message ContractTerminateRequest { Uuid contract_id = 1; string reason = 2; Uuid request_id = 3; }
message ContractRedeemRequest   { Uuid contract_id = 1; Uuid request_id = 2; }
message ContractRequest         { Uuid contract_id = 1; }

// inferred from 09 §2.1 + 99 §3.1 事件所需字段
message ContractResponse {
  Uuid contract_id = 1;          // contracts.contract_id
  PlayerIdentifier master = 2;
  PlayerIdentifier slave  = 3;
  string status = 4;             // "PENDING" | "ACTIVE" | "TERMINATED" | "EXPIRED"
  string terms_type = 5;
  string terms_json = 6;
  float revenue_share_pct = 7;
  int64 redemption_cost = 8;
  int64 created_tick = 9;        // contracts.created_tick
  int64 activated_tick = 10;     // contracts.activated_tick（3 秒后自动激活）
  int64 expires_tick = 11;       // contracts.expires_tick
  int64 terminated_tick = 12;    // contracts.terminated_tick
  string termination_reason = 13;  // contracts.termination_reason
  google.protobuf.Timestamp updated_at = 14;
}

// 替代原 ListResponse：每个 service 单独定义元素类型
message ContractListResponse {
  repeated ContractResponse contracts = 1;
  int32 total_count = 2;         // 总数（不含 limit/offset 截断）
}

// ── CorePod ─────────────────────────────────
service CorePodService {
  rpc TickPod(PodIdentifier) returns (PodTickResult);
  rpc EnterPod(PodEnterRequest) returns (PodEnterResponse);
  rpc ExitPod(PodIdentifier) returns (PodExitResponse);
  rpc GetPodState(PodIdentifier) returns (PodState);
}

message PodTickResult {
  int64 stress_units = 1;        // 应力容量 SU（04 §2.2 calculateAddedStressCapacity）
  double rpm = 2;                // GENERATED_RPM（04 §2.2，默认 16.0）
  int32 input_fluid_mb = 3;      // 输入槽余量 mB
  int32 output_fluid_mb = 4;     // 输出槽余量 mB
  int32 byproduct_count = 5;     // 产出的欲望碎片数（04 §3.4）
  int64 endurance = 6;           // 剩余耐久 ticks
  bool depleted = 7;             // 耐久耗尽（04 §4.4 强制终止）
  google.protobuf.Timestamp tick_at = 8;
}

message PodEnterRequest {
  PodIdentifier pod = 1;
  PlayerIdentifier player = 2;
  // 04 §4.2 限制条件
  bool hunger_above_5 = 3;       // 玩家饥饿值 >= 5
  int32 input_fluid_mb = 4;      // 核心舱当前输入槽余量
  int64 endurance = 5;           // 核心舱当前耐久
}

message PodEnterResponse {
  bool accepted = 1;             // 是否进入托管状态
  string reason = 2;             // 拒绝原因（"occupied" | "hunger" | "no_fluid" | "no_endurance"）
  Uuid host_uuid = 3;            // 当前 host（即使 accepted=false 也可能存在）
}

message PodExitResponse {
  bool success = 1;
  Uuid previous_host = 2;        // 退出前的 host
}

// inferred from 04 §6.3 core_pods SQL
message PodState {
  PodIdentifier pod = 1;
  Uuid host_uuid = 2;            // core_pods.host_uuid（NULL = 无人托管）
  int64 endurance = 3;           // core_pods.endurance
  int32 recipe_cooldown = 4;     // core_pods.recipe_cooldown
  string input_fluid = 5;        // core_pods.input_fluid
  string output_fluid = 6;       // core_pods.output_fluid
  int32 input_fluid_mb = 7;
  int32 output_fluid_mb = 8;
  int32 byproduct_count = 9;     // core_pods.byproduct_count
  double stress_units = 10;      // 当前 SU 输出
  string status = 11;            // "IDLE" | "HOSTED" | "OFFLINE_HOSTED" | "DEPLETED"
  google.protobuf.Timestamp updated_at = 12;
}

// ── HostileMob ──────────────────────────────
service HostileMobService {
  rpc ApplyHostileDamage(DamageRequest) returns (DamageResponse);
  rpc GetDropChance(CreatureIdRequest) returns (DropChanceResponse);
}

message CreatureIdRequest {
  string creature_id = 1;        // variant_zombie / minecraft:zombie ...
}

// inferred from 06 §8.3 mob_replacements SQL
message DropChanceResponse {
  float desire_fragment_chance = 1;  // mob_replacements.drop_chance_desire_fragment
  // 原版 drops：列出 物品注册名 -> 概率；留作扩展
  map<string, float> vanilla_drops = 2;
  bool enabled = 3;              // mob_replacements.enabled
}

// ── Environment ─────────────────────────────
service EnvironmentService {
  rpc ApplyEnvironmentEffect(EnvironmentEffectRequest) returns (EnvironmentEffectResponse);
  rpc GetEnvironmentModifiers(BlockPos) returns (EnvironmentModifiers);
}

// 2026-06-14 user decision: intensity (float) + duration_ticks (int64) 已就位
message EnvironmentEffectRequest {
  BlockPos pos = 1;
  string environment = 2;        // "LAVA" | "SWAMP_MUD" | "SAND" | "MAGMA_BLOCK"
  string effect_id = 3;          // 子效果 id（多效果细分用）
  float intensity = 4;           // 0..1+ 倍率（07 §8 公式缩放）
  int64 duration_ticks = 5;      // 持续 tick；0 = 瞬时（2026-06-14 user decision: 与 PG BIGINT 对齐）
  Uuid entity_uuid = 6;          // 受影响实体（玩家或生物）
}

// inferred from 07 §8.3 environment_effects SQL
message EnvironmentEffectResponse {
  bool applied = 1;
  float pleasure_delta = 2;      // 实际施加的 pleasure 增量
  float hunger_delta = 3;        // 实际施加的 hunger 增量
  bool triggered_defeat = 4;     // 触发战败状态（07 §6）
}

message EnvironmentModifiers {
  // 07 §8.3 风格的 modifier 总览
  float pleasure_modifier = 1;   // 累计 +pleasure / tick
  float hunger_modifier = 2;     // 累计 -hunger / tick
  float movement_modifier = 3;   // 移动速度倍率（沼泽 0.5）
  bool no_fatal_damage = 4;      // 永不死亡（07 §2.1 / §3.2）
  string primary_environment = 5;// 主要环境（优先级最高者）
}

// ── Creature ────────────────────────────────
service CreatureService {
  rpc ListCreatures(ListRequest) returns (CreatureListResponse);
  rpc GetCreature(CreatureRequest) returns (CreatureConfig);
  rpc ReloadCreatures(Empty) returns (ReloadResponse);
}

message CreatureListResponse {
  repeated string creature_ids = 1;
}

message CreatureRequest {
  string creature_id = 1;
}

// inferred from 13 §2.1 creatures.json 全字段
message CreatureConfig {
  string creature_id = 1;                    // creatures.json.creature_id
  string display_name_zh = 2;                // display_name.zh_cn
  string display_name_en = 3;                // display_name.en_us
  string model_source = 4;                   // 继承哪个原版 mob
  string creature_type = 5;                  // "MONSTER" | "PASSIVE" | "BOSS"
  int32 geckolib_format_version = 6;         // 当前固定 2
  // audio
  string audio_ambient = 7;
  string audio_hurt = 8;
  string audio_death = 9;
  string audio_step = 10;
  float audio_volume = 11;
  float audio_pitch = 12;
  // textures
  string texture_main = 13;
  string texture_overlay = 14;               // 可空
  // model
  string model_geo = 15;
  repeated string model_animations = 16;
  string model_idle_animation = 17;
  float model_scale = 18;
  // stats
  float max_health = 19;
  float attack_damage = 20;
  float movement_speed = 21;
  // drops (JSONB-like；为简化留作 raw JSON 字符串)
  string drops_override_json = 22;
  // meta
  string replaces = 23;                      // 替换哪个原版 mob
  repeated string tags = 24;                 // tags[]
  bool enabled = 25;                         // creature_configs.enabled
  int64 loaded_tick = 26;                    // creature_configs.loaded_tick
  google.protobuf.Timestamp loaded_at = 27;
}

message ReloadResponse {
  int32 reloaded_count = 1;
  repeated string failed_creature_ids = 2;   // 解析失败的 creature
  google.protobuf.Timestamp reloaded_at = 3;
}

// ── DG_LAB ──────────────────────────────────
service DglabService {
  rpc SetStrength(SetStrengthRequest) returns (SetStrengthResponse);
  rpc GetStrength(AccountRequest) returns (StrengthResponse);
  rpc GenerateToken(AccountRequest) returns (TokenResponse);
  rpc RevokeToken(TokenRequest) returns (TokenResponse);
  rpc ListTokens(ListRequest) returns (TokenListResponse);
}

message SetStrengthRequest {
  PlayerIdentifier player = 1;
  int32 channel = 2;             // 0 = A, 1 = B
  int32 strength = 3;            // 0..200（10 §2.3 公式）
  string source = 4;             // "PLEASURE_CHANGE" | "ADMIN_CMD" | "CLIENT"（10 §4.3 + dglab_strength_log.trigger_source）
  Uuid request_id = 5;
}

message SetStrengthResponse {
  bool success = 1;
  int32 new_strength = 2;
  int32 max_strength_applied = 3;// 服务器 max_strength 限制（10 §4.2）
}

message StrengthResponse {
  int32 channel_a = 1;
  int32 channel_b = 2;
  int32 max_strength = 3;        // 当前限制值
  bool player_online = 4;        // 离线时强度强制 = 0（10 §4.2）
}

message TokenRequest {
  string token = 1;
}

// inferred from 10 §5 dglab_tokens SQL
message TokenResponse {
  string token = 1;              // dglab_tokens.token（UUIDv4）
  Uuid owner_uuid = 2;           // dglab_tokens.owner_uuid
  int64 created_tick = 3;        // dglab_tokens.created_tick
  int64 last_used_tick = 4;      // dglab_tokens.last_used_tick
  bool enabled = 5;              // dglab_tokens.enabled
  google.protobuf.Timestamp created_at = 6;
  google.protobuf.Timestamp last_used_at = 7;
}

message TokenListResponse {
  repeated TokenResponse tokens = 1;
  int32 total_count = 2;
}

// ── Audit ───────────────────────────────────
service AuditService {
  rpc Query(AuditQueryRequest) returns (AuditQueryResponse);
  rpc Export(AuditExportRequest) returns (ExportResponse);
}

// inferred from 12 §2.12 + 99 §2.2 审计强制字段
message AuditQueryRequest {
  string table = 1;              // "audit_bank" | "audit_dglab" | "audit_admin"
  Uuid actor_uuid = 2;           // 99 §2.2
  string actor_type = 3;         // "PLAYER" | "ADMIN_CMD" | "RUST_SERVICE" | "HARDWARE_DGLAB"
  Uuid target_uuid = 4;
  string target_type = 5;        // "PLAYER" | "BANK_ACCOUNT" | "CONTRACT" | "CORE_POD"
  string op = 6;                 // "bank.transfer" | "pod.produce" | "contract.sign" | ...
  google.protobuf.Timestamp from = 7;
  google.protobuf.Timestamp to   = 8;
  int32 limit = 9;
  int64 before_tick_millis = 10; // 翻页游标
}

message AuditQueryResponse {
  repeated AuditRow rows = 1;
  int64 next_before_tick_millis = 2;
}

// inferred from 99 §2.2 审计日志强制字段（actor / target / op / before / after / tick / request_id）
message AuditRow {
  Uuid actor_uuid = 1;
  string actor_type = 2;         // enum-as-string
  Uuid target_uuid = 3;
  string target_type = 4;        // enum-as-string
  string op = 5;                 // e.g. "bank.transfer"
  string before_json = 6;        // 99 §2.2 before JSONB
  string after_json = 7;         // 99 §2.2 after JSONB
  int64 tick_millis = 8;         // 99 §2.2 tick_millis
  Uuid request_id = 9;           // 99 §2.2 request_id
  google.protobuf.Timestamp at = 10;
  string notes_json = 11;        // 可选备注（audit_bank.notes JSONB）
}

message AuditExportRequest {
  string table = 1;
  google.protobuf.Timestamp from = 2;
  google.protobuf.Timestamp to   = 3;
  string format = 4;             // "csv" | "json"
  Uuid actor_uuid = 5;
  string op = 6;
}

message ExportResponse {
  bytes payload = 1;
  string content_type = 2;
  int64 row_count = 3;
}
```

#### 3.2.1 ListResponse 拆分说明

`ContractService.ListContracts` 与 `DglabService.ListTokens` 原本共享一个 `ListResponse` message，但两个 RPC 的元素类型不同（契约 vs token）。本版本拆分：

- `ContractListResponse { repeated ContractResponse contracts = 1; int32 total_count = 2; }`
- `TokenListResponse { repeated TokenResponse tokens = 1; int32 total_count = 2; }`

`CreatureService.ListCreatures` 保留 `CreatureListResponse`（原本就是分开的）；其他可能用 list 的 service 在本版本未涉及，按需新增具名 ListResponse。

#### 3.2.2 字段来源索引

| Message | 关键来源 |
|---|---|
| `PodIdentifier` | 04 §6.3 core_pods PRIMARY KEY |
| `PodState` | 04 §6.3 core_pods SQL |
| `PodTickResult` | 04 §2.2 (SU/RPM), §3.4 (byproduct) |
| `PlayerState` 扩展 | 02 §1.1, 99 §5 player_state 表 |
| `PlayerStateUpdate` | 02 §1.1 partial-update 风格 |
| `BankTransaction` | 08 §5.1 bank_transactions SQL |
| `BalanceResponse` | 08 §5.1 bank_accounts SQL |
| `DepositRequest.batch_id` | 08 §2.2 cat_grass_batches |
| `HistoryRequest` | 08 §3.4 默认 16 条 |
| `ContractResponse` | 09 §2.1, 99 §3.1 事件 |
| `ContractProposeRequest` | 09 §2.1 + §5.1 SQL |
| `EnvironmentModifiers` | 07 §8.3 environment_effects |
| `TokenResponse` | 10 §5 dglab_tokens |
| `CreatureConfig` | 13 §2.1 creatures.json |
| `AuditQueryRequest` | 12 §2.12 + 99 §2.2 |
| `AuditRow` | 99 §2.2 强制字段 |
| `DamageRequest.request_id` / `PleasureRequest.request_id` | 99 §2.2 幂等性 |

### 3.3 版本策略

- 包路径固定 `biocapital.v1`
- Breaking change 必须新建 `biocapital.v2`，v1 至少保留两个 minor 版本

---

## 4. PostgreSQL Schema

### 4.1 数据库创建

- 用户名/密码：`biocapital:biocapital`（可在 toml 改）
- 数据库名：`biocapital`
- 启动时自动执行 `CREATE DATABASE IF NOT EXISTS biocapital`
- 启动时自动执行 `migrations/*.sql`

### 4.2 迁移工具

- `sqlx migrate add <name>` 创建迁移
- `sqlx migrate run` 应用迁移
- `biocapital-cli migrate` 命令调用

### 4.3 表清单

详见各模块文档。本文件仅汇总：

| 表 | 模块 |
|---|---|
| `player_state` | 02-player-state |
| `body_part_development` | 03-body-development |
| `core_pods` | 04-core-pod |
| `fluid_effects` | 05-byproducts-fluids |
| `mob_replacements` | 06-hostile-mobs |
| `environment_effects` | 07-environment |
| `bank_accounts` | 08-bank |
| `cat_grass_batches` | 08-bank |
| `bank_transactions` | 08-bank |
| `audit_bank` | 08-bank |
| `contracts` | 09-contracts |
| `contract_payouts` | 09-contracts |
| `dglab_tokens` | 10-hardware-dglab |
| `dglab_strength_log` | 10-hardware-dglab |
| `creature_configs` | 13-bio-customization |
| `audit_admin` | 12-command-system |
| `audit_dglab` | 10-hardware-dglab |

### 4.4 索引策略

- 所有外键自动索引
- 所有 `(uuid, tick_millis DESC)` 查询复合索引
- 月度审计表分区：`audit_2026_06`、`audit_2026_07` ...

---

## 5. SLO（Service Level Objective）

> 蓝图原始：「服务器优化指标描述」。本节协调各模块预算。

### 5.1 可用性

- Rust 服务可用率 ≥ 99.9%（每月停机 < 43 分钟）
- PG 备份成功率 100%
- DG_LAB 网关可用率 ≥ 99%

### 5.2 性能

- gRPC p99 latency < 50 ms
- Web UI HTTP p99 latency < 200 ms
- DG_LAB WebSocket message p99 latency < 100 ms

### 5.3 容量

- 支持 ≥ 50 名玩家同时在线
- 每玩家 PlayerState gRPC sync < 5 KB / 5 秒
- 每分钟审计写入 < 1000 行（峰值）

### 5.4 资源占用

- Rust 服务常驻 < 512 MB（不含 PG）
- PG 连接池 5–10
- 主线程 tick 不超过 50 ms

---

## 6. 日志与监控

### 6.1 日志

- 全部走 `tracing` + `tracing-subscriber`
- 输出文件：`biocapital/logs/biocapital-server-YYYY-MM-DD.log`
- 轮转：daily + size 100 MB
- 保留：30 天

### 6.2 监控

- Prometheus metrics（可选，默认关闭）
- 暴露路径：`/metrics`，走 webui 单端口（`biocapital-server.toml [Server] http_port`，默认 8080）
- 实现位置：`biocapital-webui::metrics` + `biocapital-webui::router_with_monitoring`
- 鉴权：**不鉴权**（仅在 `prometheus_enabled=true` 时挂载）
- 单端口决策（2026-06-17）：不走单独 9090 端口；Prometheus 抓取方配 `metrics_path: /metrics` 即可

---

## 7. 测试策略

### 7.1 单元测试

- 每个 crate `cargo test`
- 覆盖率目标 ≥ 80% 核心模块（biocapital-bank, biocapital-pod, biocapital-dglab）

### 7.2 集成测试

- 每个 gRPC endpoint 必须有 happy path + 3 种异常路径
- 每个 migration 必须有 rollback 测试

### 7.3 端到端测试

- `/test/e2e/` 目录下
- Rust 驱动 gRPC client + 真 PG 实例 + mock DG_LAB

---

## 8. CI/CD

### 8.1 持续集成

- GitHub Actions / GitLab CI
- `cargo test` + `cargo clippy` + `cargo fmt --check`
- Gradle `build` 验证 Java 端编译

### 8.2 发布流程

- 版本号：semver (e.g. `0.1.0-alpha.1`)
- Git tag 触发 release
- 二进制上传 GitHub Releases

---

## 9. 文档维护

- 每个 crate 必须有 `README.md` + 完整模块注释
- Public API 必须有 `///` doc comment
- 关键算法（核心舱生产、自动分红）必须有 `//!` module-level 解释

---

## 10. 性能影响（Rust 服务端总体）

- 启动：< 5 秒
- 常驻内存：< 512 MB（不含 PG）
- 主线程 tick：< 50 ms
- 网络：gRPC + HTTP + WebSocket 共用端口监听
- CPU：单核 80% 利用率下可持续运行

---

## 11. 联动点

- 全部 gRPC service 在 `99-integration-matrix.md` 中映射
- KubeJS 通过 Java → gRPC 调用
- Web UI 通过 HTTP REST 调用

---

## 12. 验收标准

- [ ] Rust workspace 完整可构建
- [ ] 所有 gRPC endpoint 注册 + 测试通过
- [ ] PostgreSQL schema + migrations 完整
- [ ] 启动期 init 流程正常
- [ ] 每日异地备份功能正常
- [ ] 与 Java 端 JNI 桥接正常
- [ ] SLO 监控到位
- [ ] CI/CD 通过
