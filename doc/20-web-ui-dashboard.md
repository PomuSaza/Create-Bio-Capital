---
module: 20-web-ui-dashboard
status: canonical — Web UI Agent 单独文档
audience: Web UI 前端开发 Agent + 任何要扩展 dashboard 的人
last_reviewed: 2026-06-20
depends_on: 00-overview, 01-cross-cutting-concerns, 15-web-ui, SYSTEM_PROMPT
---

# Web UI Dashboard — 完整规格（D18 / D22 决策）

> **2026-06-20 用户原话**（D18 决策）：
>
> "**服务器管理员通过uuid登录看到的是管理面板，而分发给玩家玩家连接的服务器并用自己的uuid登入后，看到的是给用户准备的UI**"
> "**这个UI也实时地在跟用户本身的客户端沟通**"
> "**webui由服务端服务器域名提供，但是与客户端进行沟通来实现功能**"
> "**Webui写的时候就要做好让另一个Agent专门制作webui前端界面的准备，所以要给那个Agent也提供文档**"
> "**这个文档是实时更新的**"
> "**在webui中需要实现的后端的功能，如何与前端联系，如何协助前端Agent进行开发**"
> "**类似于一些代理工具的dashboard，核心是核心，但是dashboard可以用用户来自定义多个，Dashboard可以用用户本地的部署，也可以是服务端提供，也可以是用户自己找的第三方，但是后面连接的方式和规范是一样的，连接到服务器**"

> **本文件是 Web UI 前端 Agent 的**唯一权威来源**。任何 Web UI 改动必须先看本文件 + 实时同步。

---

## 1. Web UI 是什么

> **不是**单纯的"数据查询后台"。是**双向交互的 dashboard** —— 类似代理工具（v2ray/Trojan/Clash 等）的 dashboard。

**核心特征**：
- **多视图**：服务器管理员视图 / 玩家视图（同一前端，不同权限）
- **双向通信**：Web UI ↔ MC 客户端**实时 WebSocket 双向**（**不**仅单向 HTTP）
- **灵活部署**：玩家本地 / 服务端提供 / 第三方托管（连接方式一致）
- **实时更新**：本文件**必须**实时同步

---

## 2. 三种部署模式

### 2.1 模式 A：服务端提供（默认）

- 服务器管理员部署 Web UI 到自己的服务器域名
- 玩家访问 `https://server.example.com/biocapital/` 进入 dashboard
- **Web UI 与 Rust server 同源**（同进程或同主机）

### 2.2 模式 B：玩家本地部署

- 玩家自己跑一个 Web UI 进程（electron app / docker / 本地 http server）
- 默认端口 `localhost:8765`
- 玩家配置：dashboard URL 指向本地实例
- **适用场景**：高级玩家 / 隐私需求 / 自定义

### 2.3 模式 C：第三方托管

- 第三方网站提供托管的 Web UI（类似 proxypanel 等）
- 玩家用第三方 dashboard URL + 自己的 UUID 登录
- 第三方 dashboard 连接到对应 Rust server
- **适用场景**：跨服玩家 / 不想自己部署

**关键约束**（**三种模式都要遵守**）：
- 连接到 Rust server 的 **协议必须一致**（`/players/{uuid}` 等 REST API 路径 + 鉴权方式）
- Web UI ↔ MC 客户端的 **WebSocket 协议必须一致**（§5）

---

## 3. 双视图架构

### 3.1 视图 A：服务器管理员视图

**登录**：
- URL：`/admin/login?uuid=<admin_uuid>`
- 鉴权：Rust 校验该 UUID 是否在 `admin_users` 表中（OP 权限等级 ≥ 3）
- 进入：`/admin/dashboard`

**功能**：
- 服务器总览（玩家数 / 资源数 / 性能）
- 玩家管理（查 / ban / grant_viewer）
- 资源配置（修改 config → 推送给所有在线玩家；D15/D16）
- 生物资源管理（上传/分发贴图/模型/JSON；D9）
- 审计查询（`/audit/query`）
- DG_LAB 设备总览（哪几个玩家已配对）
- DG_LAB **QR 码生成**（D18：Web UI 生成 QR，不是 MC 客户端）

**DG_LAB QR 码生成**（D18 决策核心）：
- 管理员/玩家在 Web UI 点"生成 DG_LAB QR"
- Web UI 生成 UUID clientId + 询问玩家"手机 IP"（玩家输入）
- Web UI 渲染 QR code：`https://www.dungeon-lab.com/app-download.php#DGLAB-SOCKET#ws://<phone-ip>:9999/<clientId>`
- 玩家手机 APP 扫描 → 连接 MC 客户端
- **QR 码由 Web UI 生成**（**不**是 MC 客户端 GUI）

### 3.2 视图 B：玩家视图

**登录**：
- URL：`/player/login?uuid=<player_uuid>`
- 鉴权：viewer token（从 `/biocapital admin grant_viewer <player>` 获取）
- 进入：`/player/dashboard`

**功能**：
- 自己数值（pleasure / hunger / HP / 部位开发度）
- 自己的银行账户（余额 + 历史）
- 自己的契约（作为 initiator / target）
- 自己的 DG_LAB 设备（是否已配对 / 信号强度 / 强度历史）
- 部位自助调整（**D24 决策**：支付猫草降低部位开发度）
- 奴隶契约创建 / 签约（**D26 决策**：仅 Web UI 创建）
- DG_LAB QR 码生成（D18）

**权限边界**：
- 玩家**不能**查其他玩家数据（`/players/{uuid}` 仅自己 / 管理员）
- 玩家**不能**修改全局配置
- 玩家**不能**创建审计事件
- 玩家**能**看自己所有审计事件（自己发起的 + 影响自己的）

---

## 4. Web UI ↔ Rust server 通信

### 4.1 REST API（已实现）

详见 `doc/15-web-ui.md` §3 路由表 + `wiki/webui.md`。

主要端点（`Rust HTTP :8080`）：
- `GET /health` — 健康检查
- `GET /players/{uuid}` — 玩家数值
- `GET /bank/me` — 自己账户
- `POST /bank/transfer` — 转账
- `GET /contracts` — 契约列表
- `GET /audit/query?op=...&limit=...` — 审计查询
- `POST /admin/whitelist/reload` — 重载白名单
- ...

**鉴权**：viewer token（Bearer Token / Cookie）

### 4.2 Server-Sent Events（SSE）

`GET /events?filter=...` — Rust 推送实时事件给 Web UI。

事件类型：
- `whitelist_reload` — 白名单重载
- `player_state_change` — 玩家状态变化（数值）
- `bank_transaction` — 银行转账完成
- `contract_created` / `signed` / `terminated` — 契约事件
- `defeat_state_change` — 战败状态变化
- `living_effect_change` — living effect 变化
- `audit_alert` — 审计告警（异常事件）

### 4.3 WebSocket（Web UI ↔ Rust）

`ws://<rust>:9700/ws` — 双向 RPC（用于契约推送 / DG_LAB QR 实时刷新）。

消息格式（JSON）：
```json
{ "type": "contract.pending", "contract_id": "...", "from": "...", "to": "..." }
{ "type": "contract.signed", "contract_id": "..." }
{ "type": "dglab.qr_request", "clientId": "...", "phone_ip": "..." }
{ "type": "part_dev.lower", "part": "FEET", "cost": 50 }
```

---

## 5. Web UI ↔ MC 客户端通信（D22 决策核心）

> **D22 决策**："**方案 A：Web UI 同时连 Rust + MC 客户端**"
> Web UI Dashboard 同时连：
> - **Rust server**（gRPC / HTTP / SSE；§4）
> - **MC 客户端**（WebSocket server，mod 提供）

### 5.1 架构

```
[Player's PC]
├── MC 客户端 (NeoForge mod)
│   ├── gRPC → Rust server（游戏事件 + 玩家状态）
│   └── WebSocket server（mod 提供；端口 X，监听 dashboard）
│
├── Web UI Dashboard（浏览器 / electron）
│   ├── HTTP / gRPC → Rust server（§4）
│   └── WebSocket client → MC 客户端的 WS server（本地）
│
└── DG_LAB 手机 App
    └── WebSocket → MC 客户端（LAN，QR 配对后）
```

### 5.2 MC 客户端 WS server（mod 提供）

- 端口：可配（默认 `localhost:9876`）
- 协议：JSON RPC（类似 `dglab/websocket/v2` 的简化版）
- 鉴权：玩家登录 Web UI 时拿到的 token（与 Rust viewer token 同步）

**消息类型**（MC 客户端 → Web UI）：
```json
{ "type": "ready", "player_uuid": "..." }
{ "type": "player_state", "pleasure": 50, "hunger": 80, "hp": 15, ... }
{ "type": "defeated", "reason": "HP_ZERO", "options": [...] }
{ "type": "dglab.connected", "clientId": "..." }
{ "type": "dglab.strength_feedback", "channel": "A", "value": 50 }
```

**消息类型**（Web UI → MC 客户端）：
```json
{ "type": "request_state", "fields": ["pleasure", "hunger"] }
{ "type": "dglab.qr_generated", "qr_url": "...", "clientId": "..." }
{ "type": "climax_threshold", "value": 100 }
{ "type": "part_dev.lower", "part": "FEET" }
```

### 5.3 鉴权

- Web UI 登录时获得 token（来自 Rust viewer token）
- Web UI 连接 MC 客户端 WS 时携带此 token
- MC 客户端验证 token 与登录玩家 UUID 一致

---

## 6. 实时更新本文件

**本文件是**活的"——任何 Web UI 改动必须**同步**更新本文件。

触发时机（按 `doc/SYSTEM_PROMPT.md` §20）：
- 新增 Web UI 路由
- 修改 Web UI 鉴权
- 修改 Web UI ↔ Rust API
- 修改 Web UI ↔ MC 客户端 WS 协议
- 新增 Web UI 视图（管理员 / 玩家 / 未来：审计员）
- 修改部署模式

---

## 7. 给 Web UI Agent 的提示

> **这是给另一个 Agent 的文档**——它需要从本文件起步，**不**依赖其他 agent 的输出。

**起步建议**：
1. 读 `doc/00-overview.md` §1 了解项目愿景
2. 读 `doc/15-web-ui.md` 了解已实现的路由
3. 读 `doc/08-bank.md` 了解银行数据模型
4. 读 `doc/09-contracts.md` §1.3 了解契约流程
5. 读 `doc/10-hardware-dglab.md` §2 了解 DG_LAB 协议
6. 读 `doc/11-config-system.md` 了解配置结构
7. 读 `doc/02-player-state.md` 了解玩家状态

**MVP 起步**（**先做最小可用**，按 `doc/19-dev-process.md`）：
- 实现**视图 B（玩家视图）**的 3 个核心页面：数值页 / 银行页 / 契约页
- 视图 A（管理员）**留后续增**
- 实时通信**先**用 SSE（`/events`）；WS 双向**留后续增**
- DG_LAB QR 生成**留后续增**

---

## 8. 引用

- `doc/15-web-ui.md` — 路由表 + 数据契约（已实现）
- `doc/01-cross-cutting-concerns.md` §1.4 — 双目录配置 + 服务器覆盖
- `doc/02-player-state.md` — 玩家数据模型
- `doc/08-bank.md` — 银行 / ATM / 猫草
- `doc/09-contracts.md` §1.3 — 契约流程（D26）
- `doc/10-hardware-dglab.md` — DG_LAB 协议
- `doc/19-dev-process.md` — MVP 优先
- `doc/SYSTEM_PROMPT.md` — agent 规则 + 实时文档纪律
