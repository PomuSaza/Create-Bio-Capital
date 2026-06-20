---
module: 00-overview
status: canonical
audience: all contributors
last_reviewed: 2026-06-14
---

# Create: Bio-Capital — 拆分后的设计总览

> 本文件替代原始 `Create_ Bio-Capital项目设计蓝图v2.md` 中的总览段落。
> 详细规格见同目录下 02–17 各模块文件；模块间依赖见 `99-integration-matrix.md`。
> 跨模块共享设计约束见 `01-cross-cutting-concerns.md`。

---

## 1. 项目愿景（Vision）

为 Minecraft 1.21.1 × NeoForge × Create 6.0.10 生态构建一个**完整的虚拟社会沙盒附属**。

- **去致死化生存**：所有原版致死伤害被路由到「隐性血量」池，配合「快感值（pleasure）」「饥饿值（hunger）」两条可见状态条与「部位开发度（per-BodyPart development）」机制构成核心玩法循环。
- **大工业整合**：核心舱、ATM、搅拌机、流体管道等设施完全遵循 **Create 6.0.10 的应力网络（Kinetic Network）+ 流体管道网络（Fluid Network）+ 条板箱（Create Depot / Item Hatch）模式**。
- **服务器侧主导**：所有金融、合约、生产事件由独立的 Rust 服务进程承担；Java 端只保留**输入采集**（右键、漏斗推送、状态广播）与**渲染**。原 Java 代码灰度退役。
- **硬件联动**：DG_LAB 通过标准 WebSocket 协议对接 Rust 服务（不经 Java）。
- **多附属可联动**：暴露 NeoForge 事件总线 hook + KubeJS 绑定 + JSON hook 描述符。

---

## 2. 架构总览

```
┌────────────────────────────────┐       gRPC (TCP)        ┌─────────────────────────┐
│  Minecraft 客户端 / Java 端    │ ───────────────────────▶│  Rust 服务端（独立进程）│
│  - NeoForge 1.21.1 模组         │   玩家事件 / 生产数据   │  - axum + tokio + sqlx │
│  - KubeJS bindings              │ ◀───────────────────────│  - PostgreSQL 同级目录   │
│  - Create 6.0.10 slim.jar       │   银行转账 / 合约操作   │  - DG_LAB WebSocket 网关│
└────────────────────────────────┘                          └─────────────────────────┘
                                                                        ▲
                                                                        │ WebSocket
                                                                        ▼
                                                              ┌──────────────────┐
                                                              │   DG_LAB 硬件    │
                                                              └──────────────────┘
                                                                        ▲
                                                                        │ HTTP/REST
                                                                        ▼
                                                              ┌──────────────────┐
                                                              │   React Web UI   │
                                                              └──────────────────┘
```

### 2.1 三层职责

| 层 | 语言 | 职责 | 不应负责 |
|---|---|---|---|
| 客户端/集成层 | Java (NeoForge 1.21.1) | 方块/物品/流体/实体注册、HUD 渲染、Create 应力/流体网络桥、用户右键事件采集、Create 工具交互适配 | 经济账本、合约逻辑、数据库、DG_LAB 通信 |
| 服务端 | Rust (axum + tokio + sqlx-postgres) | 银行账本、奴隶合约、核心舱生产公式、PostgreSQL 持久化、DG_LAB 网关、Web UI 数据源、审计日志、心跳 | 方块实体渲染、Create 网络细节 |
| Web UI | React + TypeScript SPA | 数值查询页、银行转账页、合约浏览页、审计导出 | 任何游戏内交互 |

### 2.2 Rust 重写范围（全量重写，Java 端彻底删除业务）

> **2026-06-18 用户决策（覆写原 §11.1 灰度退役原则）**：Java 端
> 不是灰度退役——除了必要的 JNI/NeoForge 衔接之外，**全部删干净**。
> 原 `src/main/java/mo/dystopia/biocapital/...` 本来就跑不了，留
> `// 业务逻辑在 Rust 端` + `@Deprecated` 占位没有意义。

- 新代码位于 `rust/` 子项目（具体目录约定见 `14-rust-services.md`）。
- Java 端**仅保留**：
  - `BioCapital.java`（`@Mod` 入口）+ `Config.java`（TOML 配置加载）
  - `NativeRustBindings.java`（JNI 桥；**方法签名不变**）
  - `*Init.java` 风格 static 注册块（`ModBlocks` / `ModItems` /
    `ModFluids` / `ModFluidTypes` / `ModBlockEntities` /
    `ModCreativeTabs`）
  - 12 值 `BodyPart.java` + 缓存镜像 `PlayerStateAttachment.java`
    （所有写入方法已 `@Deprecated` 为 no-op，Rust 是唯一写入者）
  - NeoForge 事件总线 hook（`AuthHandler.onPlayerLoggedIn` →
    `callBank(Authenticate)`；`BiocapitalCommand` 注册 `/biocapital *`
    指令树 → 全部走 JNI）
  - `CorePodBlock` / `CorePodBlockEntity` 仅保留 NeoForge 框架强需
    的方法签名（`useWithoutItem` / `calculateAddedStressCapacity` /
    `serverTick`），业务逻辑走 JNI
- **已删除**（task #7, 2026-06-17）：
  `BankManager` / `ContractManager` / `AtmBlock` / `BankCardItem` /
  `CatGrassItem` / `BioCapitalHud` / `DglabQrCodeScreen` /
  `ModMenuTypes` / `ModEntities` + 空 `bank` / `network` / `menu` /
  `world` / `hud` 包。
- 编译验证：`./gradlew compileJava` BUILD SUCCESSFUL（仅 NeoForge
  API 自身的 2 个 `Bus.GAME` deprecation warning，与本任务无关）。
- 审计记录：`wiki/JavaAudit.md`。

### 2.3 项目完成度（**诚实状态** — 2026-06-18）

> 引用：`wiki/webui.md` §8 + `doc/CHANGELOG.md` "Web UI audit" 段

| 指标 | 数值 |
|---|---|
| 总 task 数（标 "completed"）| 42 |
| 实际可路由/可鉴权/可 SSE/可 Prometheus | ✅ 100% |
| 实际 Java↔Rust↔PG 端到端跑通 | ❌ 从未跑过（无 E2E test）|
| 实际生产可用度 | **~70%** |
| 已知阻塞 | **5 个**：wire format 不通 + viewer token 双 store + Tonic gRPC 未启动 + 无 E2E test + KubeJS 缺失 |

**重要**：标 "completed" ≠ "production ready"。所有 task 都通过了
编译 + 单测，但**端到端**只有 Rust 内部能跑（`cargo test` 337→339 passed）。
Java↔Rust 实际 wire format 不一致（Java 用 `BiocapitalWireFormat` 手工
byte buffer，Rust JNI dispatch 用 `Debug` UTF-8 占位），因此从 Java
调任何 `NativeRustBindings.call*` 拿回的 bytes 不可信。

**未解的诚实缺口**（按优先级）：
1. **Java↔Rust wire format 对齐**（task #44，0.5-1 d）— 端到端断的根因
2. **PG viewer_tokens 表 + 双 store 同步**（task #45，1-2 d）— grant_viewer
   发的 token 在 webui 端看不到
3. **E2E 集成测试**（task #46，1 d）— 验证所有声称的能力真能跑
4. **Tonic gRPC server 启动**（task #47，0.5 d）— 外部 client 接入
5. **KubeJS bindings**（v15+，大块）— 3rd-party 集成

详见 `wiki/webui.md` §6-§9。

---

## 3. 译名表（中英对照）

| 中文 | English | 注册名 / 类型 | 备注 |
|---|---|---|---|
| 快感值 | Pleasure | `pleasure`（浮点 0–100） | HUD 显示，攻击力触发 |
| 饥饿值 | Hunger / Satiety | `hunger`（浮点 0–100） | HUD 显示 |
| 隐性血量 | Hidden HP | `hidden_hp`（浮点 ≥1，初始 20） | 后端隐藏，damage sink |
| 部位开发度 | Body Part Development | `part_dev`（按 BodyPart 枚举） | 工业生产被动加成 |
| 核心舱 | Core Pod | `create_biocapital:core_pod`（1×2×1 多方块） | 流体输入+应力输出 |
| 高潮流体 | High Tide | `create_biocapital:high_tide` | 副产物流体 |
| 超级润滑油 | Super Lubricant | `create_biocapital:super_lubricant` | 仅装饰，不改变转速上限 |
| 媚药水体 | Charm Potion | `create_biocapital:charm_potion` | 配方合成 |
| 精液 | Semen | `create_biocapital:semen` | 占位流体 |
| 欲望碎片 | Desire Fragment | `create_biocapital:desire_fragment` | 副产物物品 |
| 猫草 | Cat Grass | `create_biocapital:cat_grass` | 通用货币，单格堆叠 1000，批次号追踪 |
| 银行卡 | Bank Card | `create_biocapital:bank_card` | 身份凭证（DataComponent `OwnerUUID`） |
| ATM | ATM | `create_biocapital:atm` | 虚拟化条板箱（Create 交互） |
| 奴隶契约 | Slave Contract | `create_biocapital:contract_*` | Web UI 接口位 |
| 隐性战败 | Hidden Defeat | `defeat_state` | 非标准 buff，纯客户端表现 |
| 设备锁定 | Device Lock | `device_lock`（DataComponent） | 银行账户绑定设备 |
| 邀请码 | Invite Code | `invite_code`（UUIDv4） | 解绑设备时验证 |

---

## 4. 关键设计取舍（与原始蓝图差异）

| 主题 | 原始蓝图 | 现设计 | 说明 |
|---|---|---|---|
| MySQL | MySQL | **PostgreSQL** | 用户指定 |
| 数据库位置 | save 同级目录 | save 同级目录 + `biocapital/` 子目录 | 用户指定 |
| 数据库维护 | 主动创建 | 启动期 init + 心跳 + 异地备份（每日 03:00，保留 7 天） | 用户指定 |
| Rust 重写范围 | 全栈 | 仅服务端 Rust（Java 客户端保留） | 用户指定 |
| Java-Rust 协议 | 未定 | gRPC + Protobuf | 用户指定 |
| 银行防作弊 | N/A | 设备锁定 + 邀请码解绑 | 用户指定 |
| 反作弊覆盖 | N/A | 核心经济/资产路径全覆盖（含审计日志） | 用户指定 |
| 数值查询 | N/A | 服务器管理员 + 自己 + Web UI 全表 | 用户指定 |
| 超级润滑油 | 打破转速上限 | **纯装饰**（不改转速） | 用户覆盖原始蓝图 |
| 猫草追踪 | N/A | 批次号 + 玩家 | 用户指定 |
| Sable 集成 | N/A | 声明依赖 Sable（采用其 JNI + Docker buildRustNatives 架构） | 用户指定 |
| 联动点 | N/A | NeoForge 事件 + KubeJS + JSON hook + 文档 | 用户指定 |
| 服务器优化 | N/A | 仅指标描述（具体预算由各模块决定） | 用户指定 |

---

## 5. 服务器优化指标描述（不设默认预算）

每个模块在自身的「性能」段落描述其对服务器的影响面；总预算在 14-rust-services.md 的「SLO」章节统一协调。

- **TPS 稳定性**：50 名玩家同时在线下不发生主线程 tick 超时（>50 ms）。
- **主线程 tick 预算**：所有生产计算 / PostgreSQL 同步 / DG_LAB 事件分发**不得在主线程**。
- **内存占用**：Rust 服务进程常驻 < 512 MB（不含 PostgreSQL）。
- **冷启动时间**：从 JVM 启动到「首批玩家可加入」< 10 秒；Rust 服务从二进制启动到「接受 gRPC」< 5 秒。
- **磁盘 IO**：PostgreSQL 连接池 5–10；写入合并（coalescing）；审计表按月分区。

---

## 6. 模块索引

| 文件 | 主题 |
|---|---|
| `01-cross-cutting-concerns.md` | 配置、安全、反作弊、服务器优化指标 |
| `02-player-state.md` | PlayerState Attachment 设计 |
| `03-body-development.md` | 部位开发度枚举与增益表 |
| `04-core-pod.md` | 核心舱多方块结构与 Create 集成 |
| `05-byproducts-fluids.md` | 四种副产物流体与配方 |
| `06-hostile-mobs.md` | 敌对生物变体替换系统 |
| `07-environment.md` | 去致死化环境（岩浆/沼泽/沙/岩浆块） |
| `08-bank.md` | 猫草、银行卡、ATM、批次追踪（PRIORITY） |
| `09-contracts.md` | 奴隶契约框架 |
| `10-hardware-dglab.md` | DG_LAB 硬件联动 |
| `11-config-system.md` | 所有配置文件格式 |
| `12-command-system.md` | 指令与权限 |
| `13-bio-customization.md` | 生物外观/动画/音频自定义 |
| `14-rust-services.md` | Rust 服务架构与 gRPC schema |
| `15-web-ui.md` | Web UI 路由表与数据契约 |
| `16-sable-bridge.md` | Sable JNI 集成模式 |
| `17-asset-placeholders.md` | 资源占位文件策略 |
| `99-integration-matrix.md` | 模块依赖矩阵与变更影响 |
| `SYSTEM_PROMPT.md` | agent 驱动入口（多 agent 编排器规则） |
| `assets/creatures/_template/` | 生物自定义模板（占位文件） |

---

## 7. 阅读顺序建议

新成员请按下列顺序阅读：

1. 本文件 + `01-cross-cutting-concerns.md`（理解全局约束）
2. `14-rust-services.md`（理解服务端架构）
3. `16-sable-bridge.md`（理解 Java↔Rust 调用方式）
4. `11-config-system.md`（理解配置格式）
5. 各业务模块（02–10、13）
6. `12-command-system.md`、`15-web-ui.md`、`17-asset-placeholders.md`
7. `99-integration-matrix.md`（修改时必查）

变更时：**先改本文件**或对应模块文件，**再改 99-integration-matrix.md**，最后改 CHANGELOG。
