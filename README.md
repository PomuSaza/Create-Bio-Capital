# Create: Bio-Capital

> **为 Minecraft 1.21.1 × NeoForge × Create 6.0.10 生态构建的完整虚拟社会沙盒附属。**
> **去致死化生存 + Create 工业整合 + Rust 银行账本 + 奴隶契约 + DG_LAB 硬件联动。**

[![状态](https://img.shields.io/badge/status-alpha-orange)]()
[![生产可用度](https://img.shields.io/badge/production_ready-~70%25-yellow)]()
[![NeoForge](https://img.shields.io/badge/NeoForge-1.21.1-blue)]()
[![Create](https://img.shields.io/badge/Create-6.0.10-orange)]()
[![Rust](https://img.shields.io/badge/Rust-stable-red)]()
[![PG](https://img.shields.io/badge/PostgreSQL-16-blue)]()
[![Sable License](https://img.shields.io/badge/license-Sable_Polyform_Shield_1.0.0-purple)]()

---

## 📌 目录

- [项目简介与特点](#-项目简介与特点)
- [当前状态（诚实）](#-当前状态诚实)
- [架构](#-架构)
- [玩家指南](#-玩家指南)
- [开发者接入指南](#-开发者接入指南)
- [贡献流程](#-贡献流程)
- [许可](#-许可)
- [相关链接](#-相关链接)

---

## 🎯 项目简介与特点

> **2026-06-20 用户原话**（D11 决策）：
>
> "设定上这是一个**虚拟城镇的建设的沙盒**，本质上补充了之前 Minecraft **没有的色情玩法**，并且补充了**没有办法在其中建立可追溯的可持续的社会的缺陷**。"
> "项目核心在于**打破传统生存模式的死亡惩罚与道德上层干预**，通过将玩家的**感官体验与生理数据深度绑定**，转化为**机械动力框架下可量化、可交易的工业资产**。"
> "项目将在此高自由度的经济网络中，**自发演进并共建具备高度交互性的二次元城镇文明**。"
> "**开发流程是先做出一个可运行的最小版本作为框架然后陆续的添加需要的功能**。"

### 5 条核心定位

1. **核心定位**：虚拟城镇建设沙盒
2. **核心玩法**：补充 Minecraft 缺失的色情玩法 + 可追溯的可持续社会
3. **核心机制**：打破传统生存模式的死亡惩罚 + 道德上层干预
4. **核心绑定**：感官体验 + 生理数据 → 机械动力框架下可量化、可交易的工业资产
5. **核心涌现**：高自由度经济网络 → 自发演进的二次元城镇文明

### 核心特点（D6 + D18 + D19-D28 决策校准后）

| 维度 | 特点 |
|---|---|
| **去致死化生存** | HP=0 **不**死亡，触发**战败状态**（D3：**粉色 UI 遮罩层** + 视角摇晃，**不是** buff）；玩家可「放弃物品回床」或「用猫草恢复」（D20 区分：放弃物品 = 清 inventory（**不**动 armor）+ **不**动 buff；猫草 = 清 debuff + 扣猫草）|
| **进服前硬校验** | D21 决策：2 种情况**硬拦**（未同步 server config / 环境不兼容），其他仅 warning |
| **独立 living effect 系统** | D8 决策：自定义 `BiocapitalLivingEffects`（用 NeoForge `Attachment`），**不**走 vanilla `MobEffect`（vanilla 有 duration timer，不适合"状态性"buff 如淫纹）|
| **部位开发度** | 12 个 BodyPart 枚举；**D23 决策**：12 部位对应 12 种增益（足部=速度/胳膊=力量/胸=防御等）；GENITAL 敏感度（隐性不提醒加成）；**D24 决策**：玩家可 Web UI 支付猫草**降低**部位开发度（双向）|
| **高潮机制** | D28 决策：pleasure 触达阈值后满快感 + 1 秒衰减 + 部位开发度 +1% + 上限 100% |
| **大工业整合** | 核心舱（1×2×1 多方块，**岩浆输入**，**任意面机械手/传送带/漏斗**）+ ATM + 流体管道 + 条板箱（Create Depot）；**D19 决策**：核心舱**必须有玩家绑定**才能输出应力（16 SU）|
| **核心舱触手** | D25 决策：mod jar 内置占位 + 服务器可下发独立触手模型 + 动画（idle / engage / climax）|
| **服务端权威** | 银行账本、奴隶合约、核心舱生产公式、PostgreSQL 持久化、**资源同步**全部由独立 Rust 服务进程承担；Java 端只保留输入采集 + 渲染 + **MC 客户端连手机 WS** |
| **银行账本** | 通用货币「猫草」单格堆叠 1000，批次号追踪；银行卡 DataComponent `OwnerUUID`；**D27 决策**：ATM 正面物品框 + 银行卡；其他 5 面机械手/传送带/漏斗 |
| **奴隶契约** | **D26 决策**：**仅 Web UI 创建**（**不**是 Minecraft 物品合成）；契约方在 webui 填参数 + target_player 在 webui"签约"；走 Rust gRPC + PG |
| **DG_LAB 硬件联动** | **D6 + D18 决策**：手机蓝牙连 Coyote V3 硬件；**手机跑 LAN WS**（port 9999）；**MC 客户端**连手机 WS；**Rust 不中转** DG_LAB 指令；**DG_LAB QR 码由 Web UI 生成** |
| **Web UI Dashboard** | **D18 + D22 决策**：双向 dashboard（**不**仅查询后台）；服务器管理员视图（OP 权限 ≥ 3）+ 玩家视图（viewer token）；三种部署模式（服务端 / 玩家本地 / 第三方）；Web UI 同时连 Rust + MC 客户端（WebSocket 双向）。详见 [`doc/20-web-ui-dashboard.md`](doc/20-web-ui-dashboard.md) |
| **服务器资源分发** | D9 决策：服务器可下发自定义生物行为 JSON + 状态 icon + 触手模型/贴图/声音；玩家进服 + 5 min hash 检查后增量同步；**不**污染玩家本地 |
| **配置覆盖** | D15/D16/D17 决策：双目录（玩家本地 + 服务器下发）；进服 + 5 min hash 检查同步；**所有**配置都被服务器覆盖 |
| **多附属可联动** | 暴露 NeoForge 事件总线 hook + KubeJS bindings + JSON hook 描述符 |
| **诚实完成度** | 总体 ~70% 生产可用；5 个已知未解缺口 + 文档/进程层面 ~95% |

---

## 🚧 当前状态（诚实）

> **引用**：`doc/00-overview.md` §2.3 + `doc/CHANGELOG.md` 最近「Web UI audit / Java audit」段

| 指标 | 数值 |
|---|---|
| 总 task 数（标「completed」）| 42 + 后续 follow-up |
| 路由 / handler / PG / SSE / Auth / Prometheus | ✅ 100% 可工作 |
| `cargo test --workspace --exclude biocapital-pg` | ✅ 317+ tests pass |
| `./gradlew compileJava` | ✅ BUILD SUCCESSFUL |
| `scripts/e2e.sh` | ✅ 6/6 pass（health / whitelist / player / SSE / bank transfer / audit query）|
| Java↔Rust↔PG 端到端 wire format | ⚠️ **未端到端跑通**（task #44 in_progress）|
| 实际生产可用度 | **~70%** |

### 已知未解缺口（按优先级）

| # | 缺口 | 阻塞模块 | 状态 |
|---|---|---|---|
| 1 | Java↔Rust wire format 对齐 | Java↔Rust 全链路 | ⚠️ partial（task #44 in_progress）|
| 2 | PG `viewer_tokens` 表 + 双 store 同步 | grant_viewer 跨端 | ⚠️ partial（task #45 in_progress）|
| 3 | E2E 集成测试持续化 | 全部 | ⚠️ partial（task #46 完成 6/6；缺 CI 集成）|
| 4 | Tonic gRPC server 启动 | 外部 client 接入 | ⚠️ partial（task #47 in_progress）|
| 5 | KubeJS bindings | 3rd-party 集成 | ❌ not started（v15+）|

### 重要声明

> **「task 标 completed」≠「production ready」**。
> 所有 task 都通过了编译 + 单测，但端到端只有 Rust 内部能跑。
> Java↔Rust 实际 wire format 不一致（Java 用 `BiocapitalWireFormat` 手工 byte buffer，Rust JNI dispatch 用 `Debug` UTF-8 占位），因此从 Java 调任何 `NativeRustBindings.call*` 拿回的 bytes **不可信**。

---

## 🏗 架构（D6 决策校准后）

> **关键**：Rust **不**作为 DG_LAB 网关；MC 客户端**独立**连手机 WS。

```
                                  ┌────────────────────────┐
                                  │    Rust 服务进程         │
                                  │  (axum + tokio + sqlx)  │
                                  │  - 银行账本              │
                                  │  - 合约 / Core Pod 公式  │
                                  │  - PG 同级目录           │
                                  │  - HTTP / gRPC / SSE    │
                                  │  - 资源同步服务 (5 min)  │
                                  └────────┬───────────────┘
                                           │
                            gRPC + HTTP + SSE
                                           │
                                           ▼
┌──────────────────────┐         ┌────────────────────────┐
│  MC 客户端 / Java 端  │         │   React Web UI         │
│  - NeoForge 1.21.1   │         │  - 数值查询 / 转账      │
│  - HUD / 事件采集     │         │  - 合约浏览 / 审计导出  │
│  - living effect 渲染│         │  - viewer token 鉴权   │
│  - WS client (LAN)  │         └────────────────────────┘
│  - pleasure→强度算法 │                  ▲
│  - 强度指令→玩具     │                  │ HTTP/REST
└─────────┬───────────┘                  │
          │                              │
          │ WebSocket (LAN)              │
          │ (DG_LAB 标准 v2 协议)         │
          ▼                              │
┌──────────────────────┐                │
│  DG_LAB 手机 App     │                │
│  - LAN WS server     │                │
│  - port 9999         │                │
│  - 二维码配对          │                │
└─────────┬───────────┘                │
          │                              │
          │ Bluetooth                    │
          ▼                              │
┌──────────────────────┐                │
│  Coyote V3 硬件       │                │
│  - A/B 双通道          │                │
│  - 强度 0~200         │                │
│  - 软上限断电保存       │                │
└──────────────────────┘                │
                                        │
   ★ Rust 不经手机 WS ──────────────────┘
   ★ Rust 不中转 DG_LAB 指令
   ★ Rust 只读/写游戏数据
```

### 三层职责（D6 校准后）

| 层 | 语言 | 职责 | 不应负责 |
|---|---|---|---|
| 客户端/集成层 | Java (NeoForge 1.21.1) | 方块/物品/流体/实体注册、HUD 渲染、Create 应力/流体网络桥、用户右键事件采集、living effect 渲染、**WebSocket client 连手机 LAN WS**、**pleasure → 强度算法本地执行** | 经济账本、合约逻辑、数据库、**DG_LAB 中转** |
| 服务端 | Rust (axum + tokio + sqlx-postgres) | 银行账本、奴隶合约、核心舱生产公式、PostgreSQL 持久化、HTTP / gRPC / SSE 供 Web UI、审计日志、心跳、**周期性 config + 生物资源同步** | 方块实体渲染、Create 网络细节、**DG_LAB WebSocket 网关**、**WS 转发** |
| 手机 / 硬件 | DG_LAB APP + Coyote V3 | APP 提供 LAN WebSocket server (port 9999) + 二维码配对；Coyote V3 蓝牙连接 APP，执行强度 + 波形指令 | 游戏数据计算、网络层中转 |
| Web UI | React + TypeScript SPA | 数值查询页、银行转账页、合约浏览页、审计导出；**直连 Rust HTTP + SSE**，**不经 DG_LAB 任何东西** | 任何游戏内交互 |

### Rust 工作区（`rust/crates/`）（D6 校准后）

| Crate | 职责 |
|---|---|
| `biocapital-cli` | 启动器：detect PG、运行 SQL（**D1**：从运行时目录 `config/biocapital/sql/`）、启动 gRPC + HTTP（**不**启动 WebSocket — D6）|
| `biocapital-grpc` | Tonic gRPC server / client |
| `biocapital-jni` | JNI 桥：被 Java `NativeRustBindings.call*` 调用 |
| `biocapital-pg` | sqlx-postgres 仓库层 + D1 SQL 加载（运行时目录）|
| `biocapital-webui` | axum HTTP + SSE + 鉴权 + admin/audit/bank/contract/players 路由 + **D9 资源同步服务** |

### Java 端最小集（**仅**衔接 NeoForge + JNI）

- `BioCapital.java`（`@Mod` 入口 + `Config.load()` + `NativeRustBindings.init()`）
- `Config.java`（TOML 配置加载 + HUD/HostileMobRemoval 客户端字段）
- `NativeRustBindings.java`（JNI 桥；**方法签名不变**）
- `*Init.java` 风格 static 注册块（`ModBlocks` / `ModItems` / `ModFluids` / `ModFluidTypes` / `ModBlockEntities` / `ModCreativeTabs`）
- 12 值 `BodyPart.java` + `PlayerStateAttachment.java`（写方法 `@Deprecated` 为 no-op）
- NeoForge 事件总线 hook（`AuthHandler.onPlayerLoggedIn` → `callBank(Authenticate)`；`BiocapitalCommand` 注册 `/biocapital *` 指令树 → 全部走 JNI）
- `CorePodBlock` / `CorePodBlockEntity` 仅保留 NeoForge 框架**强需**的方法签名

> Java 业务逻辑**已全部下沉到 Rust**（2026-06-18 决策）。

---

## 🎮 玩家指南

> **2026-06-20 MVP-3 更新**：Minecraft 客户端 HUD 尚未实测（需要真实 MC runtime）。但 **Rust 服务端 + HTTP API + JNI 符号匹配 + 6/6 e2e 端到端 PASS** 已实测。
> 完整 run 步骤 + 验证矩阵见 **[`doc/RUN.md`](doc/RUN.md)** —— 一行命令就能跑完所有无 Minecraft 测试。

### 安装

1. 确认客户端：Minecraft 1.21.1 + NeoForge + Create 6.0.10
2. 下载本 mod 的最新 release jar
3. 放入 `mods/` 目录
4. 启动游戏一次 → 生成 `config/create_biocapital.toml`
5. 启动 Rust 服务（由服务器管理员部署，玩家无需关心）

> **当前 MVP-3 状态**：Rust server + Web UI + e2e 6/6 PASS 已**实测**。Minecraft 客户端 HUD 渲染**未实测**（需要真实 MC 客户端）。完整 run 步骤见 [`doc/RUN.md`](doc/RUN.md)。

### 加入服务器

- 服务器管理员在 `config/biocapital-whitelist.toml` 加入你的 TG 群白名单玩家 ID
- 加入服务器时 mod 自动 `Authenticate` 走 JNI → Rust → PG
- 玩家名通过 `PlayerNameRepository` 缓存 + 回查

### 自助操作（Web UI）

**玩家不打开游戏内任何银行 / 契约 / 设备 GUI**——右键银行卡仅触发 Sable JNI 占位反馈。所有自助操作通过 **Web UI** 完成：

| 操作 | 路径 | 鉴权 |
|---|---|---|
| 查账户余额 | `/players/{uuid}` | viewer token |
| 银行转账 | `/bank/transfer` | viewer token |
| 看合约 | `/contracts/{id}` | viewer token |
| 申请设备解绑 | `/devices/request-unlock` | viewer token |
| 审计导出 | `/audit/query?op=...` | viewer token |

> viewer token 由服务器管理员通过 `/biocapital admin grant_viewer <player>` 生成。

### 核心玩法循环

```
矿 / 农业采集
  └─► 核心舱（流体输入 + 应力输出）
        └─► 副产物：high_tide / super_lubricant / charm_potion / semen / desire_fragment
              ├─► 喂核心舱：提高部位开发度 part_dev
              ├─► ATM 卖出：换 cat_grass（批次号追踪）
              └─► 制造 DG_LAB 触发事件
                    └─► 快感值 pleasure 上升 / 饥饿值 hunger 上升
                          └─► 隐性血量 hidden_hp 上升 → 解锁更多玩法
```

### 死亡处理

**无死亡**。所有原版致死伤害（摔落、岩浆、怪物攻击、虚空）→ 路由到 `hidden_hp` 池 → 客户端显示 `defeat_state` buff（纯表现）。

### TG 群白名单

玩家必须先加入 `config/biocapital-whitelist.toml`（详见 `doc/18-tg-whitelist.md`）才能加入服务器。

---

## 🛠 开发者接入指南

### 必读文档顺序

新成员请按下列顺序阅读（**任何修改前必查**）：

1. **`README.md`（本文件）**—— 项目全貌与诚实状态
2. **`doc/00-overview.md`** —— 愿景、架构、译名表、§2.3 诚实完成度、§4 设计取舍
3. **`doc/RUN.md`** —— 怎么 run + 怎么核实（2026-06-20 MVP-3 新增；30 秒 TL;DR + 验证矩阵）
3. **`doc/01-cross-cutting-concerns.md`** —— 跨模块共享约束（**冲突时以此为准**）
4. **`doc/14-rust-services.md`** ⭐ —— Rust 服务架构 + gRPC schema
5. **`doc/16-sable-bridge.md`** —— Java↔Rust JNI 调用模式
6. **`doc/11-config-system.md`** —— 所有 TOML 配置格式
7. **`doc/99-integration-matrix.md`** —— 模块依赖矩阵（**变更必查必改**）
8. 各业务模块（02–10、13、18）
9. **`doc/12-command-system.md`** / **`doc/15-web-ui.md`** / **`doc/17-asset-placeholders.md`**
10. **审计记录**（参考实际完成度）：
    - `wiki/JavaAudit.md` —— Java 端审计（2026-06-18）
    - `wiki/webui.md` §6-§9 —— Web UI 端到端审计
    - `wiki/WireFormatAudit.md` —— Java↔Rust wire format 审计
    - `wiki/Compare.md` —— 与原蓝图合规对照
11. **`doc/SYSTEM_PROMPT.md`** ⭐ —— 驱动 agent 的完整规则；强制审计回路 + 实时文档纪律
12. **`doc/CHANGELOG.md`** —— 完整变更历史与诚实完成度
13. **`memory/MEMORY.md`** —— 跨会话记忆索引（用户偏好 + 反馈 + 项目状态）

### 模块依赖矩阵（高层视图）

| 模块 | 唯一职责 | depends_on |
|---|---|---|
| 00 | 总览 | (root) |
| 01 | 跨模块共享约束 | 00 |
| 02 | PlayerState | 00, 01 |
| 03 | BodyPart | 00, 02 |
| 04 | CorePod | 00, 01, 02 |
| 05 | 流体 + 配方 | 00, 01, 04 |
| 06 | 敌对生物 | 00, 01, 02, 13 |
| 07 | 环境 | 00, 02, 05 |
| 08 ⭐ | 银行 + 猫草 | 00, 01, 02 |
| 09 | 契约 | 00, 01, 02, 08 |
| 10 | DG_LAB | 00, 01, 02 |
| 11 | 配置 | 00, 01, 02, 08 |
| 12 | 指令 | 00, 01, 02, 08 |
| 13 | 生物自定义 | 00, 01, 06 |
| 14 ⭐ | Rust 服务 | 00, 01, 08, 10, 16 |
| 15 | Web UI | 00, 01, 08, 09, 10, 12, 14 |
| 16 | Sable JNI | 00, 14 |
| 17 | 占位策略 | 00, 01 |
| 18 | TG 群白名单 | 00, 01, 11 |
| 99 | 联动矩阵 | all |

### 3rd-party 附属接入

#### 通过 NeoForge 事件总线

```java
@SubscribeEvent
public static void onPlayerTransfer(BankTransferEvent.Post event) {
    // event.getActor(), event.getTarget(), event.getAmount(), event.getBatchId()
}
```

详见 `doc/99-integration-matrix.md` §3 事件清单。

#### 通过 KubeJS bindings（v15+ 跟进中）

> **⚠️ KubeJS bindings 当前未实现**（v15+ follow-up）。
> 临时方案：直接监听 NeoForge 事件总线即可。

#### 通过 JSON hook 描述符（运行时热加载）

`config/biocapital-hooks/*.json` —— 第三方附属挂接点，Rust 服务启动期读取。

### 资产占位策略

所有贴图 / 模型 / 音频**必须**先以 `.txt` 占位描述，**不得**嵌入二进制：

- 项目内：`src/main/resources/assets/create_biocapital/<name>.png.txt`
- 用户自定义：`config/biocapital/creatures/<id>/...`
- 模板：`doc/assets/creatures/_template/`

详见 `doc/17-asset-placeholders.md`。

---

## 🤝 贡献流程

### 强制审计回路

**任何**实现任务必须经过下列三步强制回路（详见 `doc/SYSTEM_PROMPT.md` §14 + §21）：

```
Step 1: EXECUTE（实现 subagent）
   读 doc/<模块>.md + depends_on + 99 + CHANGELOG + 审计记录
   写代码 / 改 doc
   跑 cargo test / gradlew compileJava 自检
   输出：修改清单 + 联动矩阵更新 + 待 CHANGELOG 条目

Step 2: AUDIT（独立审计 subagent —— 不得复用实现 subagent）
   git diff 看改了什么
   按实际逻辑逐项检查：正确性 / 并发一致性 / 安全 / 健壮性
   不信注释 / 不信命名 / 不信测试名
   每条标注：文件:行号 + 问题 + 为何 + 改法
   按必须改 / 建议改 / 可不改 三档排序
   输出：审计报告

Step 3a: IMPROVE（打回重写）   若必须改 ≠ 空
Step 3b: COMMIT（自主 commit） 若必须改 = 空
```

### 自主 commit 纪律

- ✅ commit 到**当前分支**（默认 `dev-raw0`）
- ✅ commit message 含审计 subagent ID + 必须改 0 / 建议改 N / 可不改 M
- ❌ 永远不 commit 到 `main`
- ❌ 永远不未经用户授权 merge 到 `main`
- ❌ 永远不 force push 共享分支

详见 `doc/SYSTEM_PROMPT.md` §23。

### PR 流程

1. 从 `main` 拉新分支：`git switch -c task/<id>-<slug> origin/main`
2. 开发 + 自检 + 提交（commit 纪律见上）
3. 推送当前分支：`git push origin task/<id>-<slug>`
4. 在 GitHub 开 PR 指向 `dev-raw0`（**不是 main**）
5. PR 描述必须含：
   - 修改清单
   - 审计 subagent ID + 报告结论
   - 验证命令 + 输出（cargo test / gradlew / e2e / curl）
   - 联动矩阵更新条目
   - 诚实完成度变化
6. 等待 CI + 人工 review

### 文档更新纪律

**任何**模块变更**必须**同步：

- `doc/<目标模块>.md` + depends_on 文件
- `doc/99-integration-matrix.md`
- `doc/CHANGELOG.md`
- `README.md`（如完成度变化 / 用户决策覆写 / 架构变化）
- `memory/MEMORY.md`（如有新踩坑 / 反馈 / 用户决策）

详见 `doc/SYSTEM_PROMPT.md` §20。

---

## 📝 许可

### 本项目

**待用户最终决定**（请在 issues 中讨论；详见 `doc/00-overview.md` §4 + §5）。

### 关键依赖

| 依赖 | 许可 |
|---|---|
| [Sable](https://github.com/ryanhcode/sable) JNI + Docker buildRustNatives | **Polyform Shield 1.0.0**（声明依赖）|
| [Create](https://github.com/Creators-of-Create/Create) 6.0.10 | MIT |
| [NeoForge](https://github.com/neoforged/NeoForge) 1.21.1 | LGPL-2.1 |
| [DGLabCraft](https://github.com/CaiJi-ikun/DG_LAB) | （参考其 repo）|

---

## 🔗 相关链接

### 项目内部

| 路径 | 用途 |
|---|---|
| [`doc/00-overview.md`](doc/00-overview.md) | 总览 + §2.3 诚实完成度 |
| [`doc/01-cross-cutting-concerns.md`](doc/01-cross-cutting-concerns.md) | 跨模块共享约束 |
| [`doc/99-integration-matrix.md`](doc/99-integration-matrix.md) | 模块依赖矩阵（变更必查必改）|
| [`doc/CHANGELOG.md`](doc/CHANGELOG.md) | 完整变更 + 诚实完成度 |
| [`doc/SYSTEM_PROMPT.md`](doc/SYSTEM_PROMPT.md) ⭐ | agent 完整规则 |
| [`doc/14-rust-services.md`](doc/14-rust-services.md) ⭐ | Rust 服务架构 |
| [`doc/08-bank.md`](doc/08-bank.md) ⭐ | 银行 + 猫草（PRIORITY）|
| [`doc/16-sable-bridge.md`](doc/16-sable-bridge.md) | JNI 桥 |
| [`doc/15-web-ui.md`](doc/15-web-ui.md) | Web UI 路由表 |
| [`wiki/JavaAudit.md`](wiki/JavaAudit.md) | Java 端审计 |
| [`wiki/webui.md`](wiki/webui.md) | Web UI 端到端审计 |
| [`wiki/WireFormatAudit.md`](wiki/WireFormatAudit.md) | Java↔Rust wire format 审计 |
| [`wiki/Compare.md`](wiki/Compare.md) | 与原蓝图合规对照 |
| [`wiki/Setup.md`](wiki/Setup.md) | 开发环境搭建 |
| [`wiki/BuildRequirements.md`](wiki/BuildRequirements.md) | 构建依赖 |
| [`memory/MEMORY.md`](memory/MEMORY.md) | 跨会话记忆索引 |


## ⚠️ 重要声明

本项目**正在进行大规模重写**（2026-06 至今）。

- 编译 / 单测 / e2e 大部分通过
- 但端到端仍有 ~30% 缺口（详见「当前状态」）
- 文档体系 (`doc/`) 已建立为唯一权威来源
- 所有 3rd-party 接入请先读 `doc/SYSTEM_PROMPT.md` §14 强制审计回路

任何 PR **必须**经过独立审计回路才能 merge。**绝不**接受「作者自审通过」类提交。

---

**本 README 由 `doc/SYSTEM_PROMPT.md` §24 维护规则驱动；任何完成度 / 用户决策 / 架构变化必须同步更新本文件。**