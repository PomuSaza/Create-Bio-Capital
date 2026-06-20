---
module: 15-web-ui
status: canonical — SEPARATE module
audience: web-ui-developers
last_reviewed: 2026-06-14
depends_on: 00-overview, 01-cross-cutting-concerns, 08-bank, 09-contracts, 10-hardware-dglab, 12-command-system, 14-rust-services
---

# Web UI（独立模块）

> 用户原始要求：「Webui是一个单独的部分需要拆分」。
> 本文件仅描述 Web UI 的**路由表、数据契约、权限模型**；**不**包含 UI 实现细节。
> 开发本模块时只需研究「如何兼容 Web UI」（其他模块同理）。

---

## 0. 玩家自助（self-service）**唯一**前端（user 决策 2026-06-14 晚）

> **玩家在游戏内**不打开任何菜单查看银行余额 / 历史 / 契约 / 设备。右键银行卡仅触发 Sable JNI 钩子（`NativeRustBindings.callBank(0=GetBalance)`）作为快速占位反馈。
>
> **所有查询/转账/历史均通过 Web UI 完成**（路径见 §3）。
>
> 玩家登录 Web UI 的方式：管理员在游戏内用 `/biocapital admin grant_viewer <player>` 指令生成一次性 viewer token（`v1.<64hex>`，含 subject UUID），玩家在聊天栏接收后到 `http://<server>:8080/login` 粘贴即可。
>
> **不允许**在游戏内新增任何银行/契约/设备的 GUI 类（per user 2026-06-14 决策；task #119 删除 BankMenu / BankScreen 后无新增计划）。
>
> 参考：`doc/12-command-system.md` §3.4 指令树；`doc/18-tg-whitelist.md` §3 鉴权流程。

---

---

## 1. 技术栈 + 架构

| 层 | 选型 |
|---|---|
| 前端框架 | React 18 + TypeScript |
| 构建工具 | Vite |
| 状态管理 | TanStack Query（缓存 + 失效） |
| UI 库 | Tailwind CSS + shadcn/ui |
| 图表 | Recharts |
| HTTP | fetch + zod 校验 |
| 后端协议 | **直连 Rust HTTP + SSE**（**不经 DG_LAB 任何东西**）|

> **2026-06-20 D6 决策**：Web UI **不**连 DG_LAB 任何东西，**不**控制硬件。
> DG_LAB 控制**完全在 Minecraft 客户端**（详见 `doc/10-hardware-dglab.md` §2.1）。
> Web UI 的 `/devices` 路由只**显示**玩家当前连接的 DG_LAB 设备状态（查询 Rust 镜像数据），**不**发送控制指令。

> **当前迭代**：仅实现数据契约 + 路由表；具体 React 组件待开发。

---

## 2. 部署模式

### 2.1 独立部署

```
<minecraft_dir>/biocapital/webui/
├── dist/                     # 构建产物
├── index.html
└── assets/

<minecraft_dir>/biocapital-backups/
```

### 2.2 Rust 服务托管

- Rust 服务（axum）同时托管 Web UI 静态文件
- 默认端口：`biocapital-server.toml` 的 `httpPort = 8080`
- 访问 URL：`http://localhost:8080/`

### 2.3 HTTPS

- 默认 HTTP；HTTPS 必须显式启用（`enableHttps = true` + 提供证书）
- TLS 证书：`config/biocapital-server.crt` + `config/biocapital-server.key`

---

## 3. 路由表（Web UI 页面）

| 路径 | 名称 | 权限 |
|---|---|---|
| `/` | 登录页 | 公开 |
| `/dashboard` | 总览 | 已登录 |
| `/players/me` | 自己的数值 | 已登录 |
| `/players/:uuid` | 任意玩家数值 | 已登录 + 自己/管理员 |
| `/bank/me` | 自己的银行 | 已登录 |
| `/bank/transfer` | 转账 | 已登录 |
| `/bank/history` | 历史 | 已登录 |
| `/contracts` | 契约列表 | 已登录 |
| `/contracts/:id` | 契约详情 | 已登录 |
| `/contracts/new` | 创建契约 | 已登录 + 自己/管理员 |
| `/devices` | DG_LAB 设备 | 已登录 + 自己/管理员 |
| `/audit` | 审计查询 | 管理员 |
| `/admin/config` | 配置管理 | 管理员 |
| `/admin/creatures` | 生物自定义 | 管理员 |

---

## 4. 数据契约（JSON Schema）

### 4.1 玩家数值（`/players/:uuid`）

```typescript
interface PlayerStateResponse {
  player_uuid: string;           // UUIDv4
  player_name: string;
  pleasure: number;
  hunger: number;
  hidden_hp: number;
  low_hp_hits: number;
  parts: { [body_part: string]: number };
  balance: number;
  active_contracts: number;
  defeat_count: number;
  updated_at: string;            // ISO 8601
}
```

### 4.2 银行转账（`/bank/transfer`）

```typescript
interface TransferRequest {
  from_account_uuid: string;
  to_player_name: string;
  amount: number;                // 正整数
  request_id: string;            // UUIDv4，幂等性去重
}

interface TransferResponse {
  success: boolean;
  actual_amount: number;
  balance_after: number;
  error_code?: string;
  error_message?: string;
}
```

### 4.3 契约列表（`/contracts`）

```typescript
interface ContractResponse {
  contract_id: string;
  master_uuid: string;
  master_name: string;
  slave_uuid: string;
  slave_name: string;
  status: 'PENDING' | 'ACTIVE' | 'TERMINATED' | 'EXPIRED';
  terms: Record<string, unknown>;
  revenue_share_pct: number;
  redemption_cost: number;
  created_at: string;
  expires_at?: string;
}
```

### 4.4 DG_LAB 设备（`/devices`）

```typescript
interface DeviceResponse {
  token: string;
  player_uuid: string;
  player_name: string;
  created_at: string;
  last_used_at?: string;
  enabled: boolean;
  current_strength: number;
}
```

### 4.5 审计查询（`/audit`）

```typescript
interface AuditQueryRequest {
  actor_uuid?: string;
  target_uuid?: string;
  op?: string;
  from_tick?: number;
  to_tick?: number;
  limit?: number;                // default 100, max 1000
  offset?: number;
}

interface AuditQueryResponse {
  results: Array<{
    log_id: string;
    actor_uuid: string;
    actor_type: string;
    target_uuid: string;
    target_type: string;
    op: string;
    before: Record<string, unknown>;
    after: Record<string, unknown>;
    tick_millis: number;
    request_id: string;
    notes?: Record<string, unknown>;
  }>;
  total_count: number;
}
```

---

## 5. HTTP API 路由（Rust 服务端）

### 5.1 基础

- 所有路由前缀 `/api/v1/`
- 所有响应格式：`application/json`
- 错误格式：`{ "error_code": string, "error_message": string, "request_id": string }`

### 5.2 鉴权

- Cookie-based session（HttpOnly + Secure + SameSite=Strict）
- 登录：`POST /api/v1/auth/login`（账号 + 密码 / OAuth）
- 注销：`POST /api/v1/auth/logout`
- 会话超时：`create_biocapital.toml` 的 `[WebUI] sessionTimeoutMinutes`

### 5.3 端点清单

| Method | Path | 说明 |
|---|---|---|
| POST | `/auth/login` | 登录 |
| POST | `/auth/logout` | 注销 |
| GET | `/players/:uuid` | 玩家数值 |
| GET | `/bank/balance` | 自己余额 |
| GET | `/bank/history` | 自己历史 |
| POST | `/bank/transfer` | 转账 |
| GET | `/contracts` | 契约列表 |
| POST | `/contracts/propose` | 创建契约提案 |
| POST | `/contracts/:id/accept` | 接受 |
| POST | `/contracts/:id/reject` | 拒绝 |
| POST | `/contracts/:id/redeem` | 赎回 |
| GET | `/devices` | 设备列表 |
| POST | `/devices/:token/revoke` | 撤销 token |
| GET | `/audit/query` | 审计查询 |
| GET | `/admin/config` | 配置查询 |
| POST | `/admin/config/reload` | 配置重载 |
| POST | `/admin/grant_viewer` | 颁发 viewer token（admin only；body `{player_uuid, ttl_seconds?}` → `{viewer_token, subject_uuid, expires_at, request_id}`；TTL 默认 7 天、上限 30 天；audit 写 `audit_admin`，fallback `audit_bank`，actor_type='ADMIN_CMD'） |
| GET | `/admin/creatures` | 生物列表 |
| POST | `/admin/creatures/reload` | 重载生物 |

---

## 6. 权限模型

### 6.1 Web UI 角色

| 角色 | 来源 | 能力 |
|---|---|---|
| `guest` | 未登录 | 仅访问 `/` |
| `player` | 登录 + 自己 | `/players/me`, `/bank/*`, `/contracts/*`, `/devices` |
| `moderator` | 登录 + 权限 3 | `player` 全部 + `/audit`, `/admin/config` |
| `admin` | 登录 + 权限 4 | `moderator` 全部 + `/admin/creatures` |

### 6.2 服务端校验

- **所有**权限校验**必须**在 Rust 服务端进行，**不**依赖前端隐藏
- 前端 UI 仅隐藏无权限的元素

---

## 7. 与指令系统对应

| Web UI 路径 | 对应指令 | 说明 |
|---|---|---|
| `/players/me` | `/biocapital stats me` | 数值显示 |
| `/players/:uuid` | `/biocapital stats <player>` | 任意玩家数值 |
| `/bank/transfer` | `/biocapital bank transfer` | 转账 |
| `/contracts/:id/redeem` | `/biocapital contract ...` | 赎回 |
| `/devices` | `/biocapital dglab list` | 设备列表 |
| `/audit/query` | `/biocapital audit query` | 审计 |
| `/admin/config/reload` | `/biocapital config reload` | 重载配置 |
| `/admin/grant_viewer` | `/biocapital admin grant_viewer <player>` | 颁发 viewer token（task #8） |
| `/admin/creatures/reload` | 无 | 暂无对应指令（Web UI only） |

---

## 8. 实时数据（Server-Sent Events）

### 8.1 用途

- 玩家数值变化时推送（替代轮询）
- 银行余额变化时推送
- DG_LAB 强度变化时推送
- 审计事件推送（管理员）

### 8.2 端点

| Path | 说明 |
|---|---|
| `GET /sse/players/me/state` | 自己的数值变化 |
| `GET /sse/bank/me/transactions` | 自己的银行变化 |
| `GET /sse/audit` | 审计事件 |

### 8.3 实现

- Rust 端 `axum::sse` 模块
- 客户端 `EventSource` API
- 心跳：每 30 秒

---

## 9. 性能影响

- HTTP server：axum + tokio，多线程
- 静态资源：CDN + gzip
- DB 查询：所有查询走索引
- SSE 连接数限制：每玩家 ≤ 3

---

## 10. 联动点

- 全部 gRPC service（详见 14）
- 全部 REST endpoint（详见本文件 5.3）
- 全部 SSE stream（详见本文件 8.2）

---

## 11. 当前迭代未完成事项

- [ ] React 组件实现
- [ ] SSE 客户端封装
- [ ] 主题 / 国际化
- [ ] 移动端适配

---

## 12. 验收标准

- [ ] 路由表完整
- [ ] 数据契约与 Rust 服务一致
- [ ] HTTP API 测试覆盖率 ≥ 80%
- [ ] SSE 推送低延迟
- [ ] 鉴权 + 权限校验完整
- [ ] 移动端 + 桌面端都能访问
