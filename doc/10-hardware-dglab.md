---
module: 10-hardware-dglab
status: canonical — 2026-06-14 晚 大改：base/max 强度模型 + OP override + 0..=200 范围
audience: all contributors
last_reviewed: 2026-06-14
depends_on: 00-overview, 01-cross-cutting-concerns, 02-player-state
references:
  - /home/saza/IdeaProjects/create_biocapital/dglab/websocket/v2/README.md（**官方权威**）
  - /home/saza/IdeaProjects/create_biocapital/dglab/bluetooth/v3/README.md（**官方权威**）
  - /home/saza/IdeaProjects/create_biocapital/dglab/websocket/v2/backend/README.md
  - /home/saza/.claude/projects/-home-saza-IdeaProjects-create-biocapital/memory/dglab-protocol-extracted.md
---

# 硬件联动 — DG_LAB

> 替代原始蓝图第 3 节「硬件联动接口」段落。
> 协议基础：DGLab 官方 v2 websocket + v3 蓝牙（已 2026-06-14 双重验证）。
> 强度语义：**本项目专属**（快感值映射 + base/max 模型 + OP override），**不**复用 DGLabCraft 业务逻辑。

---

## 1. 项目定位（与 DGLabCraft 区分）

### 1.1 共同点

- 底层硬件协议：DGLab 官方 v2 websocket + v3 蓝牙（郊狼 3.0 脉冲主机）
- 端口 9999（**手机 App** 跑 WebSocket server，**不**是 mod）
- JSON 信封协议（type/clientId/targetId/message 4 字段）
- 15 种 waveform 枚举
- 强度范围 0..=200 per channel
- 软上限 BF 指令断电保存

### 1.2 与 DGLabCraft 的**根本差异**（**关键**）

| 维度 | DGLabCraft | 本项目（Create: Bio-Capital） |
|---|---|---|
| **核心数据源** | Minecraft 原版伤害（30 个伤害倍率：cactus/arrow/lava/...） | **快感值 / 部位开发度 / 状态异常**（02 模块） |
| **强度语义** | 物理震动强度（伤害大 → 强度高） | **快感值映射百分比**（强度 = base × pleasure/100） |
| **玩家控制** | 被动接收伤害 | 玩家**主动**设 `base_intensity` / `max_intensity`（GUI + 指令） |
| **OP 介入** | 无 | **/biocapital admin override** 临时覆写（带 duration） |
| **大伤害处理** | 直接强度+ | 在 `base..max` 之间按 debuff/severity 估算（**不**直接 cap） |
| **解除控制** | N/A | `hidden_hp==1` + 大伤害**双重触发**回到正常态 |
| **WS 角色** | mod 跑 WS server | **手机 App** 跑 WS server（mod 是 client，LAN 连手机）|
| **协议参考** | mod 自己的实现 | **标准 DG_LAB v2 协议**（dglab/websocket/v2/README.md）|
| **Rust 角色** | mod 服务端中转 | **Rust 不中转 DG_LAB 指令**（仅提供游戏数据）|

> ❌ **DGLabCraft 的 30 个伤害倍率字段（cactus/sweetberry/arrow/trident/stalagmite/fall/mob_attack/player_attack/fly_into_wall/explosion/fireworks/on_fire/in_fire/lava/hot_floor/in_wall/cramming/falling_block/anvil/drown/freeze/magic/wither/dragon_breath/starve/nether/end/portal）完全不迁移**。
> DGLabCraft 的 jar **仅**用于反编译参考协议；其业务代码（30 个伤害处理）**不**复用。
> DGLabCraft 与本项目**无依赖关系**（不引用、不打包、不进 doc 外部仓库列表）。

---

## 2. 协议（**官方权威**）

> **2026-06-20 D6 决策覆写**：原 §2.1 误把"mod/Rust 跑 WS server"写为架构。**错**。
> **正确架构**：**手机 App 是 WS server**（port 9999 LAN），**Minecraft 客户端是 WS client**（连手机 LAN IP），**Rust 仅提供游戏数据**（**不**控制手机 WS）。

### 2.1 架构（D6 决策）

| 角色 | 端 | 实现 |
|---|---|---|
| **WebSocket SERVER** | **DG_LAB 手机 App** | 手机 App 在局域网（手机热点）上跑 WS server，port 9999 |
| **WebSocket CLIENT** | **Minecraft 客户端**（Java 端） | MC 客户端启动期扫手机 QR / 输入手机 IP，连接 `ws://<phone-ip>:9999/` |
| **蓝牙** | 手机 App ↔ 郊狼 3.0 | 手机 App 内部处理，**Rust 不管 / MC 不管** |
| **Rust 服务** | 独立进程 | **仅**提供游戏数据（pleasure、HP、part_dev、living_effects）；**不**连 WS，**不**中转 DG_LAB 指令 |
| **Web UI** | React | **直连** Rust（HTTP + SSE），**不**经 DG_LAB 任何东西 |

```
┌────────────────────┐       Bluetooth        ┌────────────────────┐
│  Coyote V3 硬件     │ ◀───────────────────▶ │  DG_LAB 手机 App    │
│  (郊狼 3.0 脉冲)    │   强度 + 波形         │  (WS server :9999) │
└────────────────────┘                         └────────┬───────────┘
                                                         │
                                              WebSocket (LAN)
                                                         │
                                                         ▼
                                            ┌────────────────────────┐
                                            │  Minecraft 客户端       │
                                            │  (WS client + 算法)    │
                                            │  ★ pleasure → 强度     │
                                            │  ★ 强度指令 → 玩具     │
                                            └────────┬───────────────┘
                                                     │ gRPC
                                                     ▼
                                            ┌────────────────────────┐
                                            │  Rust 服务              │
                                            │  (gRPC + HTTP + SSE)   │
                                            │  ★ 仅游戏数据          │
                                            │  ★ 不连 WS             │
                                            └────────────────────────┘
```

### 2.2 消息信封（标准 v2 协议）

```json
{ "type": "msg", "message": "<text>", "clientId": "<sessionId>", "targetId": "<app-id>" }
```

- `type`：`msg`（普通）/ `bind`（握手）/ `heartbeat`（心跳）/ `break`（断开）/ `error`
- ⚠️ JSON 字符**最大长度 1950**

### 2.3 配对流程（D6 决策版，**MC 客户端 = 网页端**）

```
1. MC 客户端启动 → 分配 clientId (UUID) → 准备 QR 码生成
2. MC 客户端 GUI 显示 QR 码：
   https://www.dungeon-lab.com/app-download.php#DGLAB-SOCKET#ws://<phone-ip>:9999/<clientId>
3. 玩家手机 APP 扫 QR → 解析 URL → 连接到 ws://<phone-ip>:9999/
4. APP 发 {type:"bind", clientId:"<app-id>", targetId:"<mc-clientId>"}
5. MC 客户端收到 bind → 验证 targetId 匹配 → 标记 is_bound=true → 配对成功 message="200"
6. 后续双向通信开始
```

**手机 IP 怎么来**（D6 决策）：
- 玩家在 MC 客户端 GUI **手动输入**手机 IP（如 `192.168.43.1`——手机热点默认网关）
- 或：MC 客户端扫**手机屏幕上的 QR 码**（不是 MC 自己生成的 QR）
- **不**用 mDNS / 局域网自动发现（增加复杂度，留后续 task）

### 2.4 QR 码格式（DGLab 官方 v2 §「终端二维码协议」）

```
https://www.dungeon-lab.com/app-download.php#DGLAB-SOCKET#ws://<phone-ip>:9999/<clientId>
```

> **注意**：`<clientId>` 是 **MC 客户端**的 ID（不是手机 APP 的 ID）。
> QR 码由 **MC 客户端** 生成（不是手机生成），含 MC 客户端 clientId + 手机 IP。

规则：
- 3 段由 **2 个 `#`** 分隔：APP 下载页 + `DGLAB-SOCKET` 标签 + SOCKET URL
- 不可含其他内容
- 本地 ws://；正式 wss://

### 2.5 强度范围（**0..=200 per channel**）

⚠️ **官方明确**：v2 websocket §「强度设置到指定值」+ v3 蓝牙 §「通道强度设定值」+ §「通道强度软上限」**都**是 `0 ~ 200`。

> 之前 task #97 子 agent 改成 0..=100 是**错误**修正；**正确**值是 0..=200。
> Rust 端 wire 0..=200 == PG 0..=200，**不**需要 wire_to_pg / pg_to_wire 转换。

### 2.6 强度控制消息（mod → app）

格式：`strength-<通道>+<模式>+<数值>`
- 通道：`1`=A, `2`=B
- 模式：`0`=减少, `1`=增加, `2`=设为指定值
- 数值：0..=200

举例：
- `strength-1+2+35` → A 通道设为 35
- `strength-2+0+1` → B 通道 -1
- `strength-1+1+5` → A 通道 +5

### 2.7 强度回传（app → mod）

APP 在通道强度变化时自动上报：
```
strength-A 强度 + B 强度 + A 上限 + B 上限
```
例：`strength-11+7+100+35`

### 2.8 波形消息

`pulse-<通道>:["<HEX 波形数据>",...]`
- HEX 每条 8 字节（100ms）
- 数组最大 100 条（10 秒）
- 波形定义见 [dglab/websocket/v2/README.md] §「波形数据」

### 2.9 清空 / 心跳

- `clear-<1|2>` —— 清空通道波形队列
- `{type:"heartbeat", clientId:..., targetId:..., message:"200"}` —— 60s 一次

### 2.10 错误码

| 码 | 含义 |
|---|---|
| 200 | 成功 |
| 209 | 对方已断开 |
| 400 | ID 已被其他客户端绑定 |
| 401 | 目标客户端不存在 |
| 402 | 收发方未绑定 |
| 403 | 非法 JSON |
| 404 | 收信人离线 |
| 405 | message 长度 > 1950 |
| 406 | 缺 channel |
| 500 | 服务器异常 |

### 2.11 15 种 Waveform（官方）

```rust
pub enum WaveformType {
    Adamage, Bdamage, Aheal, Bheal,        // 通道 A/B 伤害/治愈
    Continuous, Pulse, Tapping, Wave, Vibration,  // 节奏类
    Sine, Square, Triangle, Ramp, Noise,   // 基础波形
    Custom,
}
```

---

## 3. **本项目专属**强度设计（user 2026-06-14 晚决策）

### 3.1 三层映射模型

```
[PlayerState] (pleasure / hidden_hp / damage_event)
        │
        ▼
[EffectSource::compute_strength] in Rust
        │
        ▼
[output] (0..=200 per channel)
        │
        ▼
[DGLab WebSocket → APP → 蓝牙 → 郊狼 3.0 硬件]
```

### 3.2 玩家 2 个设定（玩家 GUI + OP 指令都可改）

| 字段 | 范围 | 默认 | 含义 |
|---|---|---|---|
| `base_intensity` | 0..=200 | **60** | 日常游玩强度基准 |
| `max_intensity` | 0..=200 | **80** | 安全封顶（**始终 ≥ base**） |
| `waveform_a` | WaveformType | Continuous | A 通道默认波形 |
| `waveform_b` | WaveformType | Pulse | B 通道默认波形 |

存储在 PG `player_dglab_config` 表：
```sql
CREATE TABLE player_dglab_config (
  player_uuid UUID PRIMARY KEY REFERENCES player_state(player_uuid),
  base_intensity SMALLINT NOT NULL DEFAULT 60 CHECK (base_intensity >= 0 AND base_intensity <= 200),
  max_intensity SMALLINT NOT NULL DEFAULT 80 CHECK (max_intensity >= 0 AND max_intensity <= 200 AND max_intensity >= base_intensity),
  waveform_a VARCHAR(32) NOT NULL DEFAULT 'continuous',
  waveform_b VARCHAR(32) NOT NULL DEFAULT 'pulse',
  updated_tick BIGINT NOT NULL
);
```

### 3.3 计算公式

**正常态**（hidden_hp > 1 且**非**大伤害）：
```
output_a = base_intensity × (pleasure / 100)
output_b = max_intensity × (pleasure / 100)  // 备用映射（user 决策时给的例子）
```

例：base=60, max=80, pleasure=20%
- output = 60 × 0.20 = 12（取整）

**触发态**（**双重触发**）：
- 条件 1：`hidden_hp == 1`（玩家陷入糟糕状态但不死亡）
- 条件 2：单帧伤害 > `DAMAGE_THRESHOLD`（默认 5 HP，可配置）
- 任一条件满足即触发

```
output = clamp(
  base_intensity
  + (max_intensity - base_intensity) × defeat_severity
  + pleasure_debuff_factor,
  0,
  max_intensity
)
```

- `defeat_severity` ∈ [0, 1]：当前 defat_state 严重度（每次 defeat 重新计算）
- `pleasure_debuff_factor` ∈ [-max_intensity, +max_intensity]：与 debuff 数 / 类型成反比
  - 多个 debuff → factor 越负
  - 特殊 debuff（如「魅惑」「战败」触发时）→ factor 可能为正（**主动强化**）

### 3.4 OP 临时覆写（**新功能**）

指令格式：`/biocapital admin override <user> <param> <value> <duration_seconds>`

| param | 含义 |
|---|---|
| `base` | 临时覆盖 `base_intensity` |
| `max` | 临时覆盖 `max_intensity`（必须 ≥ base） |
| `waveform_a` | 临时强制 A 通道 waveform |
| `waveform_b` | 临时强制 B 通道 waveform |
| `clear` | 立即清除该玩家当前所有 override |

- 过期后**立即恢复**（不渐进）
- 多个 OP 同时覆写：后到的覆盖前到的（无 stacking）
- 写 `audit_dglab.op = "dglab.override"` + `notes = {param, value, duration_seconds, issuer_uuid}`

PG 表：
```sql
CREATE TABLE dglab_overrides (
  override_id UUID PRIMARY KEY,
  target_player_uuid UUID NOT NULL,
  param VARCHAR(16) NOT NULL CHECK (param IN ('base', 'max', 'waveform_a', 'waveform_b', 'clear')),
  value_int SMALLINT,                -- 当 param 是 base/max
  value_str VARCHAR(32),             -- 当 param 是 waveform_a/waveform_b
  issued_by UUID NOT NULL,           -- OP 玩家 UUID
  issued_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  expires_at TIMESTAMPTZ NOT NULL,
  active BOOLEAN NOT NULL DEFAULT TRUE
);
CREATE INDEX idx_dglab_overrides_player_active ON dglab_overrides(target_player_uuid, active) WHERE active = TRUE;
CREATE INDEX idx_dglab_overrides_expiring ON dglab_overrides(expires_at) WHERE active = TRUE;
```

### 3.5 玩家 GUI 改 base/max

Java 端新增 GUI（**不**复用任何 DGLabCraft 业务代码）：
- `screen/biocapital/network/DglabConfigScreen.java` —— 3 滑块（base / max / waveform_a）+ 「保存」按钮
- 提交时调 Sable JNI `PlayerConfigService.SetPlayerConfig`
- Rust 端校验 `max >= base`（CHECK 约束已保证）

---

## 4. Rust 端实现

### 4.1 WebSocket Server（`rust/crates/biocapital-dglab/src/ws_server.rs`）

按 dglab/websocket/v2/README.md 完整实现（**已 task #96 落地**）：
- `tokio-tungstenite::accept_async` 监听端口
- 单连接 Mutex
- `bind` 消息处理 → 设 `target_id` + `is_bound`
- 出站：构造 JSON 信封 → `send(Message::Text(...))`
- QR 码 URL：`https://www.dungeon-lab.com/app-download.php#DGLAB-SOCKET#ws://<host>:<port>/<clientId>`

### 4.2 强度计算（`rust/crates/biocapital-dglab/src/strength.rs`，**新文件**）

```rust
pub struct EffectSource;

impl EffectSource {
    pub fn compute_strength(
        player_state: &PlayerStateSnapshot,
        config: &PlayerDglabConfig,
        override_: Option<&DglabOverride>,
        damage_event: Option<&DamageEvent>,
    ) -> (u8, u8) {  // (output_a, output_b)
        // 1. 取有效 base/max（override 优先）
        let base = override_.map(|o| o.effective_base(config.base_intensity))
            .unwrap_or(config.base_intensity);
        let max = override_.map(|o| o.effective_max(config.max_intensity))
            .unwrap_or(config.max_intensity);

        // 2. 判定触发态
        let triggered = player_state.hidden_hp <= 1.0
            || damage_event.map(|d| d.amount > DAMAGE_THRESHOLD).unwrap_or(false);

        // 3. 公式
        let output = if !triggered {
            (base as f32 * (player_state.pleasure / 100.0)) as u8
        } else {
            let severity = defeat_severity(player_state);
            let debuff_factor = pleasure_debuff_factor(player_state);
            (base as f32
                + (max - base) as f32 * severity
                + debuff_factor) as u8
        };
        let output = output.clamp(0, max as u8);
        (output, output)  // 默认 A/B 同值；差异化由 override_waveform 控制
    }
}
```

### 4.3 调度器（`rust/crates/biocapital-dglab/src/scheduler.rs`）

```rust
pub struct EffectScheduler {
    active: RwLock<Option<ActiveEffect>>,
}

pub struct ActiveEffect {
    pub source: EffectSourceEnum,
    pub output_a: u8,
    pub output_b: u8,
    pub waveform_a: WaveformType,
    pub waveform_b: WaveformType,
    pub lease_until: DateTime<Utc>,
}

pub enum EffectSourceEnum {
    PleasureChange,
    DamageTrigger,    // hidden_hp==1 或大伤害
    AdminOverride,    // OP 强制
    BiocapitalReward, // 04 模块（核心舱奖励）
    Idle,             // 静默
}

impl EffectScheduler {
    pub async fn submit(&self, effect: ActiveEffect, server: &DglabWsServer) {
        // 1. 高优先级覆盖（AdminOverride > DamageTrigger > PleasureChange > Idle）
        // 2. 发 clear-1 + clear-2 → pulse-A + pulse-B → strength-1+2+a + strength-2+2+b
        // 3. 写 audit_dglab
    }
}
```

### 4.4 PG migration（**重写** `rust/migrations/20260614000004_dglab.sql`）

> ⚠️ task #97 子 agent 改成 0..=100 是错的。**正确**是 0..=200（与 DGLab 官方对齐）。

```sql
-- 2026-06-14 晚 user task #110 (第三次回溯修正)
-- task #6 写 0..=200 → task #97 改 0..=100 (错) → 本次改回 0..=200
-- 同时新增 dglab_overrides（OP 覆写）+ player_dglab_config（玩家自设）

-- === dglab_tokens（保持 0..=200 per channel）===
CREATE TABLE dglab_tokens (
  token_id UUID PRIMARY KEY,
  owner_uuid UUID NOT NULL,
  target_id VARCHAR(64),
  max_strength_a INT NOT NULL DEFAULT 200 CHECK (max_strength_a >= 0 AND max_strength_a <= 200),
  max_strength_b INT NOT NULL DEFAULT 200 CHECK (max_strength_b >= 0 AND max_strength_b <= 200),
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  connected_at TIMESTAMPTZ,
  last_pulse_at TIMESTAMPTZ,
  created_tick BIGINT NOT NULL
);
CREATE UNIQUE INDEX idx_dglab_tokens_owner_enabled ON dglab_tokens(owner_uuid) WHERE enabled = TRUE;
CREATE INDEX idx_dglab_tokens_target ON dglab_tokens(target_id);

-- === dglab_strength_log（保持 0..=200）===
CREATE TABLE dglab_strength_log (
  log_id UUID PRIMARY KEY,
  owner_uuid UUID NOT NULL,
  channel_a INT NOT NULL CHECK (channel_a >= 0 AND channel_a <= 200),
  channel_b INT NOT NULL CHECK (channel_b >= 0 AND channel_b <= 200),
  waveform_a VARCHAR(32),
  waveform_b VARCHAR(32),
  trigger_source VARCHAR(32) NOT NULL CHECK (trigger_source IN (
    'PLEASURE_CHANGE', 'DAMAGE_TRIGGER', 'ADMIN_OVERRIDE', 'BIOCAPITAL_REWARD', 'IDLE', 'CLIENT'
  )),
  tick_millis BIGINT NOT NULL,
  request_id UUID
);
CREATE INDEX idx_dglab_strength_log_owner_time ON dglab_strength_log(owner_uuid, tick_millis DESC);
CREATE INDEX idx_dglab_strength_log_source_time ON dglab_strength_log(trigger_source, tick_millis DESC);

-- === audit_dglab（保持 0..=200）===
CREATE TABLE audit_dglab (
  log_id UUID PRIMARY KEY,
  actor_uuid UUID NOT NULL,
  actor_type VARCHAR(16) NOT NULL CHECK (actor_type IN ('PLAYER', 'ADMIN_CMD', 'RUST_SERVICE', 'HARDWARE_DGLAB')),
  target_owner_uuid UUID,
  op VARCHAR(32) NOT NULL CHECK (op IN (
    'dglab.token.generate', 'dglab.token.revoke', 'dglab.strength.set',
    'dglab.connection.open', 'dglab.connection.close', 'dglab.bind',
    'dglab.override.issue', 'dglab.override.expire', 'dglab.config.set'
  )),
  before_strength_a INT, after_strength_a INT,
  before_strength_b INT, after_strength_b INT,
  tick_millis BIGINT NOT NULL,
  request_id UUID,
  notes JSONB
);
CREATE INDEX idx_audit_dglab_target_time ON audit_dglab(target_owner_uuid, tick_millis DESC);
CREATE INDEX idx_audit_dglab_op_time ON audit_dglab(op, tick_millis DESC);

-- === dglab_overrides（新增，OP 覆写）===
CREATE TABLE dglab_overrides (
  override_id UUID PRIMARY KEY,
  target_player_uuid UUID NOT NULL,
  param VARCHAR(16) NOT NULL CHECK (param IN ('base', 'max', 'waveform_a', 'waveform_b', 'clear')),
  value_int SMALLINT CHECK (value_int IS NULL OR (value_int >= 0 AND value_int <= 200)),
  value_str VARCHAR(32),
  issued_by UUID NOT NULL,
  issued_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  expires_at TIMESTAMPTZ NOT NULL,
  active BOOLEAN NOT NULL DEFAULT TRUE
);
CREATE INDEX idx_dglab_overrides_player_active ON dglab_overrides(target_player_uuid, active) WHERE active = TRUE;
CREATE INDEX idx_dglab_overrides_expiring ON dglab_overrides(expires_at) WHERE active = TRUE;

-- === player_dglab_config（新增，玩家自设）===
CREATE TABLE player_dglab_config (
  player_uuid UUID PRIMARY KEY,
  base_intensity SMALLINT NOT NULL DEFAULT 60 CHECK (base_intensity >= 0 AND base_intensity <= 200),
  max_intensity SMALLINT NOT NULL DEFAULT 80 CHECK (max_intensity >= 0 AND max_intensity <= 200 AND max_intensity >= base_intensity),
  waveform_a VARCHAR(32) NOT NULL DEFAULT 'continuous',
  waveform_b VARCHAR(32) NOT NULL DEFAULT 'pulse',
  updated_tick BIGINT NOT NULL
);
```

### 4.5 gRPC service（**扩展** `rust/crates/biocapital-grpc/src/dglab_service.rs`）

原 5 RPC + 新增 3 个：
- `GetPlayerConfig(AccountRequest) -> PlayerDglabConfig` —— 读 base/max/waveform
- `SetPlayerConfig(SetPlayerConfigRequest) -> PlayerDglabConfig` —— 玩家自己设
- `AdminOverride(AdminOverrideRequest) -> PlayerDglabConfig` —— OP 临时覆写

---

## 5. Java 端（**禁止** 复用 DGLabCraft 业务）

| 文件 | 职责 |
|---|---|
| `network/DglabQrCodeScreen.java` | QR 码显示（URL 见 §2.4） |
| `network/DglabConfigScreen.java` | 玩家 GUI：base/max/waveform 滑块 |
| `command/BiocapitalCommand.java`（已存在） | 增 `/biocapital admin override` 子命令 |
| `auth/HardwareIdCollector.java` | 硬件 ID 采集（task #83） |

**禁止**：
- ❌ 任何 `CactusMultiplier` / `ArrowMultiplier` 等 DGLabCraft 风格字段
- ❌ 30 个伤害源 mapping 表
- ❌ 任何 `event.damage` → 强度的硬编码映射

---

## 6. 配置

### 6.1 Java 端 `create_biocapital.toml`

```toml
[DGLAB]
ws_enabled = true
ws_host = "127.0.0.1"
ws_port = 9999
sync_channels = false
hud_enabled = true
hud_position = 0
base_intensity_default = 60     # 本项目新增
max_intensity_default = 80     # 本项目新增
damage_threshold = 5.0          # 本项目新增（HP）

# ❌ 不再有 cactus / sweetberry_bush / arrow 等 30 个伤害倍率
# 玩家通过 PlayerState 自然得到 pleasure 变化，不再从伤害源硬编码
```

### 6.2 Rust 端 `biocapital-server.toml`

```toml
[Server.Dglab]
ws_host = "0.0.0.0"
ws_port = 9999
heartbeat_interval_seconds = 60
session_id_length = 20
```

---

## 7. 性能影响

- 主线程 tick：无（WebSocket 在 tokio dedicated runtime）
- 玩家登录时计算强度：单次 < 1 ms（CPU bound）
- PG `dglab_strength_log` 写入：每分钟每玩家 < 60 行（峰值 < 600 行/分钟 50 玩家）
- 内存：每连接约 8 KB（WebSocket 缓冲）

---

## 8. 联动点

- `DglabStrengthChangeEvent` —— 玩家 DG_LAB 强度变化
- `DglabConnectionEvent` —— DG_LAB 连接/断开/bind
- `DglabConfigChangeEvent`（**新**）—— 玩家改 base/max/waveform
- `DglabOverrideEvent`（**新**）—— OP 覆写颁发 / 过期
- KubeJS：`events.onDglabStrengthChange(event => { event.player, event.strength_a, event.strength_b, event.waveform_a, event.waveform_b })`

---

## 9. 验收标准

- [ ] QR 码内容含正确 URL（§2.4 格式）
- [ ] bind 消息处理后 is_bound=true
- [ ] 强度范围 0..=200 per channel（**0..=100 是错误**）
- [ ] 玩家设 base=60, max=80, pleasure=20% → 实际输出 12
- [ ] hidden_hp==1 时强度上调到 max 区间
- [ ] 大伤害（> 5 HP）触发上调
- [ ] OP `/biocapital admin override <user> base 100 60` 后玩家 base=100 持续 60 秒
- [ ] 覆写过期后立即恢复玩家自设
- [ ] PG 4 张表（tokens/strength_log/audit_dglab/overrides）+ player_dglab_config 齐全
- [ ] 审计完整记录所有事件
- [ ] **不**实现 DGLabCraft 30 个伤害倍率字段

---

## 10. 不允许的事

- ❌ 复用 DGLabCraft 业务代码（30 个伤害倍率、cactus 处理等）
- ❌ 强度范围使用 0..=100（**官方明确是 0..=200**）
- ❌ 把 DG_LAB 强度硬编码为 Minecraft 伤害值的函数
- ❌ Rust 端跑 WebSocket **client** 主动连硬件
- ❌ 同一 targetId 多连接
- ❌ 玩家离线时 DG_LAB 仍有强度
- ❌ 凭训练数据猜测协议（必须参考 dglab/ 官方文档）

---

## 11. 与原始 doc 差异

| 主题 | 原 doc 假设 | 现设计（**第三次回溯**） |
|---|---|---|
| 强度范围 | 9700 端口 / 0..200 | 9999 端口 / **0..=200 per channel**（task #97 改 0..=100 是**错误**） |
| 强度语义 | 物理震动强度 | **快感值映射百分比**（base × pleasure/100） |
| 玩家控制 | 无 | **base_intensity / max_intensity**（默认 60/80） |
| OP 介入 | 无 | **`/biocapital admin override`** 临时覆写带 duration |
| 大伤害 | 1:1 强度增加 | **双重触发**（hidden_hp==1 + 大伤害）在 base..max 插值 |
| 伤害源 mapping | 30 个伤害倍率（DGLabCraft 残留） | **完全不实现**（用 PlayerState 自然 pleasure 变化） |
| 波形 15 种 | 5 种占位 | **15 种官方**（adamage/continuous/pulse/...） |
| 调度模型 | 单优先级 | **EffectSource 4 值** + lease + override 优先 |
| QR 码 | 无 | **新增**（DGLab 官方 URL 格式） |
| 来源 | DGLabCraft jar 反编译 | **DGLab 官方 dglab/ 目录**（权威） |
