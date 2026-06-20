---
module: 08-bank
status: canonical — PRIORITY module
audience: all contributors
last_reviewed: 2026-06-14
depends_on: 00-overview, 01-cross-cutting-concerns, 02-player-state
priority_reason: "用户指定优先拆分；是经济核心；猫草是通用货币；防作弊覆盖最重"
---

# 银行系统 — 猫草、银行卡、ATM

> 替代原始蓝图第 5 节「猫草货币与银行系统」段落。
> 现 Java 实现见 `src/main/java/mo/dystopia/biocapital/bank/BankManager.java`、`item/BankCardItem.java`、`item/CatGrassItem.java`、`block/AtmBlock.java`、`block/AtmBlockEntity.java`、`menu/BankMenu.java`。
> **PRIORITY**：本模块是经济核心，必须最先完成 Rust 重写并联调。

---

## 1. 角色定位

### 1.1 三种角色

| 资产 | 类型 | 角色 |
|---|---|---|
| 猫草（Cat Grass） | 物品（堆叠 1000） | **通用法定货币**（面值 = 1，单片叶 = 1 单位） |
| 银行卡（Bank Card） | 物品 | **身份凭证**（DataComponent `OwnerUUID`） |
| ATM | 方块 | **虚拟化条板箱**（Create 交互） |

### 1.2 关键不变量

- 猫草 **不带 NBT 标签**，**不带 DataComponent**，面值恒为 1。
- 银行卡 1:1 绑定一个账户（账户由 `OwnerUUID` 决定）。
- ATM **不是 GUI 方块**，是「条板箱式」方块（使用 Create 的漏斗/传送带交互）。
- 银行菜单 **仅**在右键银行卡空气时打开；ATM 刷卡**不打开任何 GUI**。

---

## 2. 猫草（Cat Grass）

### 2.1 注册信息

| 属性 | 值 |
|---|---|
| 注册名 | `create_biocapital:cat_grass` |
| 类型 | `Item`（不是 `BlockItem`） |
| 单格堆叠上限 | **1000** |
| 面值 | **1**（单片叶） |
| NBT | **无**（蓝图明确要求） |
| DataComponent | **无** |

### 2.2 批次号追踪（用户指定）

每片猫草在创建时分配一个**批次号**：

| 字段 | 类型 | 说明 |
|---|---|---|
| `batch_id` | UUIDv7 | 批次唯一标识 |
| `producer_uuid` | UUID | 生产者（玩家或原版系统） |
| `production_tick` | BIGINT | 生产时间戳 |
| `production_source` | enum | `ATM_DEPOSIT` / `BIOCAPITAL_REWARD` / `ADMIN_ISSUE` |

> **批次号不存储在物品 NBT**；仅存储在 PostgreSQL 的 `cat_grass_batches` 表。
> 玩家持有的猫草通过 ATM 转入账户时，批次号信息转入 PG；通过 ATM 转出时，从 PG 批次表出库。
> **审计报表可按批次号查询**：所有「该批次」猫草的来源、去向、当前持有账户。

### 2.3 实现

- `CatGrassItem extends Item`：
  - `@Override stacksTo = 1000`
  - 无任何 NBT / DataComponent
- `assets/create_biocapital/models/item/cat_grass.json`（占位，详见 17）
- `assets/create_biocapital/textures/item/cat_grass.png`（占位）

### 2.4 与漏斗/传送带兼容

- 玩家可使用任何物品交互方式（漏斗/漏斗门帘/传送带等通用管线）直接流转猫草。
- 流转不改变批次号；批次号记录在 PG。
- 任何流转事件触发 `CatGrassTransferEvent`。

---

## 3. 银行卡（Bank Card）

### 3.1 注册信息

| 属性 | 值 |
|---|---|
| 注册名 | `create_biocapital:bank_card` |
| DataComponent 1 | `owner_uuid: UUID`（绑定账户所有人） |
| DataComponent 2 | `device_lock: String`（首次签发时绑定设备） |
| 单格堆叠上限 | 1 |

### 3.2 DataComponent 注册

```java
public static final Supplier<DataComponentType<UUID>> OWNER_UUID =
    DATA_COMPONENTS.registerComponentType(
        "bank_card_owner",
        builder -> builder.persistent(UUIDUtil.CODEC)
    );
```

> **必须**：注册必须在 items 注册事件之前完成（详见 01 第 1.4 节）。
> 现有实现使用 `RegisterEvent` listener 直接注册（不是 DeferredRegister），以避免类加载顺序问题。

### 3.3 右键空气 → 银行菜单

- 玩家右键空气（手持 `bank_card`） → 打开 `BankMenu`（容器 ID = unique）。
- 菜单归属由卡片 `OwnerUUID` 决定。
- 菜单是**唯一**的客户端交互点。

### 3.4 银行菜单 3 个面板

#### 面板 1：余额（Balance）

- 显示本卡当前账户余额（只读长整数）。
- 显示最近 16 条交易历史（来自 `BankManager.getHistory`）。
- 数据通过 `DataSlot` 同步。

#### 面板 2：转账（Transfer）

- 输入：目标玩家名（`PlayerName` 文本组件）+ 转账金额（long）
- 操作：点击「转账」按钮 → 服务端 `BankManager.transfer(fromUuid, toUuid, amount)`
- **规则**：余额 = 0 不可转出
- **规则**：转账金额必须 ≤ 当前余额
- **规则**：转账金额 > MAX_BALANCE 自动 cap 到 MAX_BALANCE

#### 面板 3：溯源（History）

- 最近 16 条本卡相关操作记录（存款/取款/转账/收转账）。
- 来源：`BankManager.getHistory(UUID)` —— 返回环形 buffer。
- 数据通过 `BankMenu` 的 `DataSlot` 同步。

### 3.5 设备锁定（已废弃，迁移到 doc/18）

> **2026-06-14 用户决策覆写**：原 doc/08 §3.5「设备锁定 + 邀请码」机制**完全废弃**。
> 替代方案见 [[18-tg-whitelist.md]]：TG 群白名单 + 硬件 token（3 slot FIFO + 30 天过期）。
> 本节保留为历史参照，新代码**不**得引用。

### 3.5.1 旧设备锁定（已废弃）

- 每张卡首次签发时绑定**设备 ID**。
- 默认设备：首次使用的 Level 维度 + 客户端 IP hash。
- 同卡在不同设备：仅允许查询/展示，不允许转账/取款/存款。
- 解绑：邀请码机制（UUIDv4）+ 服务端验证。

### 3.5.2 硬件 token 子系统（user task #83，引用 doc/18）

完整设计见 [[18-tg-whitelist.md]]。本章只列出与 bank 模块的集成点：

- `biocapital-bank` crate 持有 `HardwareTokenService` 域
- 登录风控：调 `BiocapitalAuth::authenticate(uuid, username, hardware_id_hash)`
- gRPC 5 个新增 RPC：`RequestHardwareToken` / `BindHardware` / `ListHardware` / `RevokeHardware` / `Authenticate`
- PG 表：`hardware_tokens` + `audit_hardware_token`（FIFO 触发器在 PG 层强制 3 slot 限制）
- proto schema 见 [[18-tg-whitelist.md]] §7

---

## 4. ATM（D27 决策校准，2026-06-20）

### 4.1 总体定位

> 本质是"虚拟化的条板箱"（保险柜式），使用 **Create 机械动力规范**交互。
> **不是 GUI 方块**。
> **D27 决策**：正面（朝向玩家）是物品框面（插入**银行卡**说明账号）；其他 5 面用**机械手/传送带/漏斗**输入猫草。

### 4.2 注册信息

| 属性 | 值 |
|---|---|
| 注册名 | `create_biocapital:atm` |
| 尺寸 | 1×1×1 |
| 朝向属性 | `FACING`（`BlockStateProperties.HORIVERTICAL_FACING`） |
| **正面**（FACING）| 1 张**银行卡**槽（`SLOT_CARD`） |
| **其他 5 面** | 5 个**猫草 I/O 槽**（`SLOT_GRASS`），接受机械手/传送带/漏斗 |

### 4.3 侧感知能力

通过 `RegisterCapabilitiesEvent` 注册 `Capabilities.ItemHandler.BLOCK`：

| 面 | 返回 |
|---|---|
| **正面**（FACING，朝向玩家）| 仅**银行卡**槽（`mayPlace` 接受 `create_biocapital:bank_card`） |
| **其他 5 面** | 仅**猫草 I/O 槽**（`mayPlace` 接受 `create_biocapital:cat_grass`） |

> **D27 决策核心**：
> - 玩家手持**银行卡**对 ATM **正面**右键 → 插入卡片
> - **机械手 / 传送带 / 漏斗**从 ATM 其他 5 面输入 cat_grass（**符合 Create 工业规范**）
> - 玩家**不**用 GUI 与 ATM 交互（**不是 GUI 方块**）

### 4.4 行为规则

| 场景 | 行为 |
|---|---|
| **插入银行卡**（正面）| "身份验证"，**不打开 GUI**；卡片写入 `device_lock`（首次） |
| **机械手/传送带输入猫草**（其他 5 面）| 销毁物品，按 `cat_grass.face_value × count` 给**卡主人**账户加余额 |
| **抽出猫草**（其他 5 面）| 余额扣减，吐出对应数量的猫草 |
| **非猫草被漏斗推入**（其他面）| `mayPlace` 拒绝，原样返回 |
| **非银行卡被推入**（正面）| `mayPlace` 拒绝 |
| **无银行卡插在槽里** | 猫草 I/O 全部停摆，吐回猫草 |
| **玩家按"查询"** | 顶部 HUD 显示当前卡对应账户余额（详见 `doc/15-web-ui.md` + `doc/20-web-ui-dashboard.md`）|

### 4.5 防作弊（端口唯一性 + 双 store 同步）

- 同一 tick 内一个 ATM 只能接受一个端口输入。
- 端口唯一性由 `slot.facing` 决定。
- 违反则触发审计事件 `ATM_DOUBLE_INSERT`。
- **D6 衍生**：viewer token 双 store 同步（task #45 in_progress）—— Rust 端存储镜像 + Web UI 端独立缓存，每 5 min 同步。

### 4.5 防作弊（端口唯一性）

- 同一 tick 内一个 ATM 只能接受一个端口输入。
- 端口唯一性由 `slot.facing` 决定。
- 违反则触发审计事件 `ATM_DOUBLE_INSERT`。

---

## 5. 服务端账本（Rust）

### 5.1 PostgreSQL 表

```sql
-- 账户主表
CREATE TABLE bank_accounts (
  account_uuid UUID PRIMARY KEY,
  owner_uuid UUID NOT NULL,
  balance BIGINT NOT NULL CHECK (balance >= 0),
  max_balance BIGINT NOT NULL DEFAULT 100000000,
  device_lock VARCHAR(256),
  created_tick BIGINT NOT NULL,
  updated_tick BIGINT NOT NULL
);

-- 猫草批次
CREATE TABLE cat_grass_batches (
  batch_id UUID PRIMARY KEY,
  producer_uuid UUID,
  production_tick BIGINT NOT NULL,
  production_source VARCHAR(32) NOT NULL,
  total_amount BIGINT NOT NULL,
  remaining_amount BIGINT NOT NULL,
  current_holder_uuid UUID  -- null if in circulation
);

-- 交易历史（环形 buffer by query）
CREATE TABLE bank_transactions (
  tx_id UUID PRIMARY KEY,
  account_uuid UUID NOT NULL,
  op VARCHAR(16) NOT NULL,  -- DEPOSIT / WITHDRAW / TRANSFER_OUT / TRANSFER_IN
  amount BIGINT NOT NULL,
  balance_after BIGINT NOT NULL,
  counterparty_uuid UUID,
  counterparty_name VARCHAR(64),
  tick_millis BIGINT NOT NULL,
  request_id UUID,  -- 幂等性去重
  FOREIGN KEY (account_uuid) REFERENCES bank_accounts(account_uuid)
);

CREATE INDEX idx_bank_tx_account ON bank_transactions(account_uuid, tick_millis DESC);

-- 审计
CREATE TABLE audit_bank (
  log_id UUID PRIMARY KEY,
  actor_uuid UUID NOT NULL,
  actor_type VARCHAR(16) NOT NULL,
  target_account_uuid UUID NOT NULL,
  op VARCHAR(16) NOT NULL,
  before_balance BIGINT NOT NULL,
  after_balance BIGINT NOT NULL,
  tick_millis BIGINT NOT NULL,
  request_id UUID,
  notes JSONB
);
```

### 5.2 gRPC 接口

```protobuf
service BankService {
  rpc GetBalance(AccountRequest) returns (BalanceResponse);
  rpc Deposit(DepositRequest) returns (BalanceResponse);
  rpc Withdraw(WithdrawRequest) returns (BalanceResponse);
  rpc Transfer(TransferRequest) returns (BalanceResponse);
  rpc GetHistory(AccountRequest) returns (HistoryResponse);
  rpc LockDevice(LockDeviceRequest) returns (LockDeviceResponse);
  rpc UnlockDevice(UnlockDeviceRequest) returns (UnlockDeviceResponse);
  rpc GenerateInviteCode(AccountRequest) returns (InviteCodeResponse);
  rpc AcceptInviteCode(AcceptInviteRequest) returns (LockDeviceResponse);
}
```

### 5.3 幂等性

- 每个 gRPC 请求带 `request_id`（UUID）。
- Rust 端在 `bank_transactions` 中检查 `request_id` 是否已存在；若存在，返回原结果。

---

## 6. 性能影响

- 主线程 tick：ATM 处理漏斗事件 0.2 ms / ATM
- 异步任务：Rust 累积交易，每 5 秒 flush PG
- 内存：每账户约 256 字节；交易历史按索引分页加载
- 网络：5 秒一次 gRPC 心跳；交易即时同步

---

## 7. 联动点

- `CatGrassTransferEvent` —— 猫草流转时
- `BankTransactionEvent` —— 任何银行操作完成
- `ATMInsertEvent` / `ATMExtractEvent`
- `BankCardDeviceLockChangeEvent`
- KubeJS：`events.onBankTransaction(event => { event.account, event.amount, event.op })`

---

## 8. 与原始蓝图差异

| 主题 | 原始蓝图 | 现设计 |
|---|---|---|
| 银行卡右键空气 | 打开银行菜单（3 个面板） | 保留 |
| ATM | "虚拟化板条箱" | 保留 |
| 批次号追踪 | N/A | 新增（用户指定） |
| 设备锁定 | N/A | 新增（用户指定） |
| 邀请码 | N/A | 新增（用户指定） |
| MAX_BALANCE | 未规定 | 100,000,000（与现有 Java 一致） |
| HISTORY_SIZE | 16 | 16（与现有 Java 一致） |

---

## 9. 验收标准

- [ ] 猫草堆叠 1000，无 NBT，无 DataComponent
- [ ] 银行卡首次签发绑定 `OwnerUUID` + `device_lock`
- [ ] 银行卡右键空气打开菜单
- [ ] 银行菜单 3 面板齐全（余额/转账/溯源）
- [ ] ATM FACING 面只接银行卡
- [ ] ATM 其他 5 面只接猫草
- [ ] 猫草被推入销毁 + 账户 +balance
- [ ] 余额不足时提取失败
- [ ] 余额 = 0 时转账失败
- [ ] 设备锁定跨设备生效
- [ ] 邀请码解绑流程通顺
- [ ] PostgreSQL 表 + 索引 + 分区齐全
- [ ] gRPC 接口单元测试覆盖率 ≥ 90%
- [ ] 审计表正确记录 actor / before / after / request_id
