---
module: 09-contracts
status: canonical
audience: all contributors
last_reviewed: 2026-06-14
depends_on: 00-overview, 01-cross-cutting-concerns, 02-player-state, 08-bank
---

# 奴隶契约系统（Slave Contracts）

> 替代原始蓝图第 5 节「奴隶契约系统」段落。
> 现 Java 实现见 `src/main/java/mo/dystopia/biocapital/bank/ContractManager.java`。
> 当前迭代：仅构建底层框架，**不**开发前端表现层。

---

## 1. 框架范围

### 1.1 当前实现

- `ContractManager`（Java 端）维护所有契约的内存表。
- 蓝图原始要求：**预留完整的 Web UI 和 ATM 交互接口，暂不进行前端表现层的开发**。
- 现状：**框架 + 接口位**已存在，前端**未实现**。

### 1.2 设计决策

- 契约逻辑**全部在 Rust 侧**。
- Java 端 `ContractManager` 仅作为**接口占位**（灰度退役）。
- Web UI 接口位由 15-web-ui 描述。

### 1.3 创建流程（D26 决策校准，2026-06-20）

> **D26 决策**："**仅 Web UI 创建**"（玩家在 webui 中填参数）
> **不**走 Minecraft 物品合成（**不**用欲望碎片 + 书本；**不**像 writable book 风格）。
> **不**是 MC 端"右键接受"，是 Web UI 端"确认"。

**完整流程**：

```
1. 契约方（玩家 A）打开 Web UI → 进入"创建契约"页面
2. 填写参数：
   - target_player_uuid（被契约方，玩家 B）
   - duration_days（游戏内天数；3 = 三天）
   - payout_percent（每游戏天 2% = 每日从 A 转 2% 给 B）
   - note（备注）
3. 提交 → Rust 校验 → 写 PG（status = "PENDING_TARGET_SIGN"）
4. Web UI 推送给玩家 B（通过 WebSocket 实时推 + Dashboard 通知）
5. 玩家 B 在 Web UI 中**看到**契约详情（只读）
6. 玩家 B 点"签约" → Rust 校验 → 写 PG（status = "ACTIVE"）
7. 服务端定时器：每游戏天按 payout_percent 转账
8. 持续 N 天 → 自动 terminate；或任一方主动 terminate
```

**Rust gRPC**：

```
service ContractService {
    rpc CreateContract(CreateContractRequest) returns (CreateContractResponse);
    rpc SignContract(SignContractRequest) returns (SignContractResponse);
    rpc TerminateContract(TerminateRequest) returns (TerminateResponse);
    rpc ListContracts(ContractQuery) returns (ContractListResponse);
}
```

**关键约束**：
- **签约方只能看到自己作为 target_player 的契约**（viewer token 鉴权）
- **发起方只能看到自己作为 initiator 的契约**
- **OP/admin 可看所有契约**（审计）
- 参数**不**可修改（一旦创建就锁死；如需修改则 terminate 旧 + 创建新）

---

## 2. 数据模型

### 2.1 契约（Contract）字段

| 字段 | 类型 | 说明 |
|---|---|---|
| `contract_id` | UUIDv7 | 契约唯一标识 |
| `master_uuid` | UUID | 主控方（受益者） |
| `slave_uuid` | UUID | 被契约方 |
| `created_tick` | BIGINT | 创建时间 |
| `status` | enum | `PENDING` / `ACTIVE` / `TERMINATED` / `EXPIRED` |
| `terms` | JSONB | 契约条款（自由定义） |
| `revenue_share_pct` | FLOAT | 主控方分红比例（0–100） |
| `redemption_cost` | BIGINT | 赎回所需 cat grass（用于赎回逻辑） |
| `expires_tick` | BIGINT | 到期时间（NULL = 永久） |

### 2.2 条款（Terms）JSONB Schema

```json
{
  "type": "DAILY_WAGE_TRANSFER" | "MINING_QUOTA" | "BREEDING_CONSENT" | "CUSTOM",
  "parameters": {
    // 任意 JSON，按 type 自由扩展
  }
}
```

> **扩展性**：条款类型由 Rust 端通过 trait 注册；KubeJS 脚本可注入新类型。

---

## 3. 框架组件

### 3.1 契约创建

- 双方（master + slave）必须**在线**且**处于同一 Level**。
- 双方必须**各自**点击「确认」按钮（Web UI）；按钮点击事件 gRPC `ContractPropose` / `ContractAccept`。
- 创建后状态 `PENDING`，3 秒后自动转 `ACTIVE`（除非双方撤销）。
- 双方不可与自己签契约。

### 3.2 自动分红

- `DAILY_WAGE_TRANSFER` 类型：每日 00:00（服务端 tick）自动从 slave 账户转账到 master 账户。
- 转账金额 = `slave.balance × revenue_share_pct / 100`。
- 转账走 08-bank 模块的 `BankManager.transfer`。

### 3.3 经济赎回

- 任何时候，slave 可发起赎回请求：消耗 `redemption_cost` cat grass。
- `redemption_cost` 需从 slave 账户扣除（gRPC `BankService.Withdraw`）。
- 扣除成功后契约自动终止（status → `TERMINATED`）。

### 3.4 终止条件

- 任一方主动 `Terminate`：契约立即终止。
- 契约到期（`expires_tick` 已过）：自动终止。
- 任一方被服务器封禁：自动终止。
- `redemption_cost` cat grass 不足：赎回失败，契约保持 ACTIVE。

---

## 4. 与 08-bank 联动

| 事件 | 触发 |
|---|---|
| 自动分红 | `BankService.Transfer` |
| 赎回扣款 | `BankService.Withdraw` |
| 主控方收到转账 | `BankTransactionEvent` 监听 |
| 账户被扣空 | `BankService.Transfer` 失败 → 契约 status 不变 |

> 所有联动通过 gRPC，不直接调用 BankManager（Java 端灰度退役）。

---

## 5. Rust 重写后的形态

### 5.1 PostgreSQL 表

```sql
CREATE TABLE contracts (
  contract_id UUID PRIMARY KEY,
  master_uuid UUID NOT NULL,
  slave_uuid UUID NOT NULL,
  status VARCHAR(16) NOT NULL CHECK (status IN ('PENDING', 'ACTIVE', 'TERMINATED', 'EXPIRED')),
  terms JSONB NOT NULL,
  revenue_share_pct FLOAT NOT NULL CHECK (revenue_share_pct BETWEEN 0 AND 100),
  redemption_cost BIGINT NOT NULL,
  created_tick BIGINT NOT NULL,
  activated_tick BIGINT,
  expires_tick BIGINT,
  terminated_tick BIGINT,
  termination_reason VARCHAR(64)
);

CREATE INDEX idx_contracts_master ON contracts(master_uuid);
CREATE INDEX idx_contracts_slave ON contracts(slave_uuid);
CREATE INDEX idx_contracts_status ON contracts(status, expires_tick);

-- 自动分红历史
CREATE TABLE contract_payouts (
  payout_id UUID PRIMARY KEY,
  contract_id UUID NOT NULL,
  tick_millis BIGINT NOT NULL,
  amount BIGINT NOT NULL,
  from_account_uuid UUID NOT NULL,
  to_account_uuid UUID NOT NULL,
  success BOOLEAN NOT NULL,
  error_message VARCHAR(256),
  FOREIGN KEY (contract_id) REFERENCES contracts(contract_id)
);

CREATE INDEX idx_payouts_contract ON contract_payouts(contract_id, tick_millis DESC);
```

### 5.2 gRPC 接口

```protobuf
service ContractService {
  rpc ProposeContract(ContractProposeRequest) returns (ContractResponse);
  rpc AcceptContract(ContractAcceptRequest) returns (ContractResponse);
  rpc RejectContract(ContractRejectRequest) returns (ContractResponse);
  rpc TerminateContract(ContractTerminateRequest) returns (ContractResponse);
  rpc RedeemContract(ContractRedeemRequest) returns (ContractResponse);
  rpc GetContract(ContractRequest) returns (ContractResponse);
  rpc ListContracts(ListRequest) returns (ListResponse);
}
```

### 5.3 自动分红调度器

- Rust 端启动时启动每日定时任务（tokio `interval`）。
- 每日 00:00（服务端 tick 校准）：扫描所有 ACTIVE contracts 中 `type = DAILY_WAGE_TRANSFER` 的契约，执行分红。
- 失败重试 3 次（间隔 10 秒）；失败则记录 `contract_payouts` 表 + 审计日志。
- **不**在主线程 / tokio 主 runtime 执行（使用 dedicated runtime）。

---

## 6. 性能影响

- 主线程 tick：契约列表查询 O(log n) （索引）
- 异步任务：每日 1 次扫描 + N 次分红（N = ACTIVE contracts 数）
- 内存：每契约约 1 KB（含 terms JSONB）
- 网络：分红走 gRPC 即时

---

## 7. 联动点

- `ContractCreatedEvent` / `ContractActivatedEvent` / `ContractTerminatedEvent`
- `ContractPayoutEvent` —— 分红完成时
- KubeJS：`events.onContractCreated(event => { event.contract })`

---

## 8. 当前迭代未完成事项

> 蓝图原文：「当前版本预留完整的 Web UI 和 ATM 交互接口，暂不进行前端表现层的开发。」

当前设计：

- Web UI 接口位：见 `15-web-ui.md` 的「Contracts」页面规划
- ATM 交互接口：**未规划**（蓝图原始描述仅说「预留」，无具体设计）
- 待用户确认是否需要在 ATM 上提供契约交互（如「查阅自己当前契约」）

---

## 9. 验收标准

- [ ] PostgreSQL 表 + 索引齐全
- [ ] gRPC 接口齐全
- [ ] 双方在线 + 双方确认才能创建
- [ ] 自动分红每日执行
- [ ] 赎回逻辑正确
- [ ] 终止条件正确
- [ ] 审计日志齐全
