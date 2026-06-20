---
module: 01-cross-cutting-concerns
status: canonical
audience: all contributors
last_reviewed: 2026-06-14
depends_on: 00-overview
---

# 跨模块共享约束（Cross-Cutting Concerns）

> 本文件描述影响**所有**模块的共享设计约束。任一模块文档中如与本文件冲突，**以本文件为准**（除非该模块明确标注 `overrides: 01-cross-cutting-concerns`）。

---

## 1. 配置加载规范

### 1.1 文件位置

| 文件 | 路径 | 用途 |
|---|---|---|
| `create_biocapital.toml` | `config/create_biocapital.toml` | Java 端所有可调项（用户视角） |
| `biocapital-server.toml` | `config/biocapital-server.toml` | Rust 服务进程配置（端口、PG 凭据、心跳间隔） |
| `biocapital-whitelist.toml` | `config/biocapital-whitelist.toml` | TG 群白名单玩家 ID 集合（详见 18） |
| `biocapital-hooks/*.json` | `config/biocapital-hooks/*.json` | 第三方附属挂接点（运行时热加载） |
| `biocapital/creatures/*.json` | `config/biocapital/creatures/<creature_id>/*.json` | 生物自定义（每个 mob 一个 behavior.json，详见 06/13）|
| `biocapital/status/*.png` | `config/biocapital/status/<effect_id>.png` | **状态 icon**（D13 决策；解包 RPG MVP.png 的美术资源放这里；详见 17） |

### 1.1.1 PostgreSQL 数据目录 vs SQL 迁移文件（**2026-06-20 D1 决策覆写**）

> **2026-06-20 用户原话**："**模块的这个编译的目录就是之后都要编译进 jar 文件的里面，为什么会有带日期的这个表单的这个数据库？这肯定不正常，至少带日期不正常**"
> "**不**是把它硬编码到模块里面"
> "**配置文件都在这里……就能够让用户修改的，能够查看的，能够实时更新的表单**"

> **D1 决策**：SQL 迁移文件**从源码树 `rust/migrations/` 移到 `run/config/biocapital/sql/` 运行时目录**。
> 用户**应能查看 / 修改 / 实时更新** SQL。带日期的演进文件**不正常**（如确需演进，可手动新增并重启）。

| 类别 | 路径 | 性质 | git? |
|---|---|---|---|
| **SQL 迁移文件** | `<minecraft_dir>/config/biocapital/sql/*.sql` | 运行时配置；用户可改 | ❌ **不**追踪（运行时配置） |
| **PG 数据目录** | `<minecraft_dir>/biocapital/pgdata/` | 运行时数据；PG 服务进程的 base/ pg_wal/ 物理文件 | ❌ **不**追踪（运行时生成） |
| **PG 初始化脚本** | `<minecraft_dir>/biocapital/init.sql`（可选） | 首次启动期 init + 异地备份配置 | ❌ 不追踪 |
| **异地备份** | `<用户指定>/biocapital-backup-YYYYMMDD.tar.gz` | 每日 03:00 自动打包 | ❌ 不追踪 |

> **旧行为**（2026-06-14 ~ 2026-06-19，**已废弃**）：`rust/migrations/*.sql` 硬编码在源码树，`MIGRATION_DIR` 是绝对路径（`rust/crates/biocapital-cli/src/main.rs:72`），配置项 `migration_dir_embedded = true` 误导性命名。**全部清掉**。

**Rust 启动期 init 流程**（`biocapital-cli start`，D1 改）：
```
1. 检测 <minecraft_dir>/biocapital/ 是否存在
   ├─ 不存在 → 创建 + 启动 PG
   └─ 存在 → 直接启动 PG

2. 启动 PG 进程（postgres -D <minecraft_dir>/biocapital/pgdata）

3. 等待端口开放（pg_isready）

4. 连接 PG + 跑 SQL 迁移文件
   └─ 来自 <minecraft_dir>/config/biocapital/sql/ 目录（运行时）
   └─ 启动期按文件名顺序执行（**不**带日期，**不**用 sqlx::migrate!）
   └─ 用户可手动加新文件 + 重启生效

5. 启动 gRPC server（50051）+ HTTP（8080）

6. 启动每日 03:00 异地备份 cron 任务
```

**异地备份**（每日 03:00）：
- `tar -czf <用户指定>/biocapital-backup-$(date +%Y%m%d).tar.gz <minecraft_dir>/biocapital/pgdata/`
- 保留 7 天；旧备份自动删除
- 备份目标路径在 `biocapital-server.toml` 的 `[Backup] target_dir` 配置

**关键不变量**（D1 重写）：
- ❌ **不**把 SQL 迁移文件**编译进 Rust 二进制**
- ❌ **不**把 SQL 迁移文件**进 git**
- ❌ **不**把 SQL 迁移文件**放在源码树**（应在 `config/biocapital/sql/` 运行时目录）
- ✅ SQL 迁移文件**用户可读 / 改 / 增**；重启 Rust 服务生效

### 1.2 TOML 校验

- 全部 `defineInRange` 必须有最小/最大边界；越界值回退默认值并日志 WARN。
- 颜色字段统一 ARGB hex（如 `#CCFF69B4`），不接受 RGB/HSL。
- 列表字段统一 `defineListAllowEmpty`，空列表与缺失等价。
- **状态 icon 路径**（D13）：统一 `<minecraft_dir>/config/biocapital/status/<effect_id>.png`；缺失 → 落到占位（magenta missing texture + WARN）

### 1.3 配置热重载

- Java 端：通过 `Config.load()` 暴露 reload API；服务器侧 `/biocapital config reload` 触发。
- Rust 端：通过 tokio signal 监听 `SIGHUP` 或文件 `notify` 触发 reload。
- **服务器配置同步**（D15/D16/D17）：见 §1.4

### 1.4 双目录配置 + 服务器覆盖（2026-06-20 D15/D16/D17 决策）

> **用户原话**："**服务器端下发的数据是覆盖性的，在连接到服务器时，所有服务器端数据生效**"
> "**玩家配置的是离线存档，或者自己开服务器时发放的配置文件**"
> "**正常配置文件是一份服务器需要下发到文件夹，这个文件夹是覆盖性的**"
> "**这样才不会服务器端修改同步到玩家，影响了玩家本地要开服务器或玩家本地单人存档玩家所设置的**"

**设计**：

| 目录 | 用途 | 何时使用 |
|---|---|---|
| `<minecraft_dir>/config/biocapital/*.toml` | 玩家**本地**配置 | **离线 / 单人 / 自开服**（玩家本地就是服务端）|
| `<minecraft_dir>/config/biocapital-online/<server_id>/*.toml` | **服务器下发**配置 | **在线**（玩家进服时拉取）|
| `<minecraft_dir>/config/biocapital/creatures/*.json` | 玩家本地生物行为 JSON | 离线 / 单人 |
| `<minecraft_dir>/config/biocapital-online/<server_id>/creatures/*.json` | 服务器下发生物行为 | 在线（周期性同步） |
| `<minecraft_dir>/config/biocapital/status/*.png` | 玩家本地状态 icon | 离线 / 单人 |
| `<minecraft_dir>/config/biocapital-online/<server_id>/status/*.png` | 服务器下发状态 icon | 在线（周期性同步） |

**D15 双目录**：两份独立文件；服务器下发**不污染**玩家本地（`config/biocapital/`）。

**D16 同步机制**：**进服时拉取** + **每 5 分钟 hash 检查**（仅当 hash 变化时重拉）。即"选项 B"：
- 进服时：Rust 推整套 config（`[Recovery]` / `[PlayerState]` / `[Bio]` / ...）到 `config/biocapital-online/<server_id>/`
- 每 5 min：Rust 算 config hash；如玩家端 hash 与服务端不一致 → 重推
- 玩家端**始终**用 `config/biocapital-online/<server_id>/` 下的版本（在线期间）
- 玩家断服 → 切回 `config/biocapital/`（本地）

**D17 覆盖范围**：**所有配置字段**都被服务器覆盖。包括：
- gameplay：`[Recovery] cat_grass_cost` / `[PlayerState] max_pleasure` / `[Bio] default_<mob_id>_effect_duration` / ...
- 本地化：`[Log] level` / `[Audit] output_path` / `[Debug] enabled`（**也**被覆盖；玩家不能保留本地调试开关）

**核心目的**：防止"服务器端修改同步到玩家本地影响单人/自开服"——你提的精确担忧。

**实现要点**（`doc/14-rust-services.md` § 详细）：
- 玩家进服时 Rust 算一次 `server_config.toml` + 推送到 `config/biocapital-online/<server_id>/`
- 玩家 MC 客户端启动时检测 `online/` 目录存在 → 用其配置；否则用 `config/biocapital/`
- 周期性 hash 同步由 Rust 主动 push

### 1.4.1 客户端进服前硬校验（D21 决策，2026-06-20 新增）

> **用户原话**："**只有两种情况需要拦下，剩下的只需要在服务器内有提示就可以**
> **第一种玩家没有同步server config**
> **第二种玩家的环境无法进入服务器无法兼容服务器的模组**"

**两种硬拦情况**（玩家**无法进服**）：

| # | 情况 | 检测方式 | 错误提示 |
|---|---|---|---|
| 1 | **玩家没同步 server config** | MC 客户端启动期检测 `config/biocapital-online/<current_server_id>/` 存在 + hash 一致；不满足 → 拦 | "**未同步服务器配置，请等待资源同步完成**" + 自动触发同步（不阻塞太久）|
| 2 | **玩家环境不兼容**（mod 不匹配/缺失）| MC 客户端对照 Rust server 推送的 mod_list（modid + version）；任一不匹配 → 拦 | "**服务器要求 mod X v1.0.0，当前 v0.9.0。请安装匹配版本**" |

**其他情况**仅 warning（**不**拦）：
- 配置项缺默认值 → 警告但允许进
- TG 白名单未生效 → 警告（但玩家连服时 Rust 会校验）
- cat_grass 余额不足 → 进服后 UI 提示
- 部位开发度低 → 进服后提示

**实现要点**：
- 玩家点"连接服务器"按钮时，MC 客户端先与 Rust server 握手
- 握手协议：mod_list + clientId + config hash
- Rust 返回 accept / reject + 原因
- 接受 → 正常进服流程
- 拒绝 → 显示错误界面 + "重试"按钮

### 1.5 周期性资源同步（D9 决策）

> **资源类型**（同步到玩家本地的非 .toml 资源）：
> - 生物行为 JSON（`creatures/<mob_id>/behavior.json`）
> - 状态 icon PNG（`status/<effect_id>.png`）
> - 未来：自定义贴图 / 模型 / 声音

**机制**：与 §1.4 config 同步共用通道。玩家进服时拉取，**每 5 分钟 hash 检查**后增量同步。

详见 `doc/13-bio-customization.md` + `doc/14-rust-services.md`。

---

## 2. 反作弊范围（核心经济/资产路径全覆盖）

> **范围**：所有会影响 cat grass 余额、部位开发度、核心舱产出、合约状态的事件，都必须经过 Rust 服务侧校验。

### 2.1 攻击面清单

| 攻击面 | 防御策略 |
|---|---|
| 银行账本双重记账 | PostgreSQL 事务 + UUIDv7 主键 + 乐观锁 |
| 转账透支/重复签名 | 服务端校验源账户余额、签名去重 |
| ATM 双面插入刷钱 | 同一 tick 内一个 ATM 只能接受一个端口输入；端口唯一性由 `slot.facing` 决定 |
| 部位开发度联机修改 | 仅服务端可写；客户端任何修改尝试被 `if (level.isClientSide) return` 拦截 |
| 核心舱 SU 产出速率伪造 | 计算公式在 Rust 侧；Java 端只读输出（`getGeneratedStress()` 由 Rust 通过 Sable JNI 注入） |
| PostgreSQL 参数注入 | 全部使用 `sqlx` prepared statement；禁止字符串拼接 |
| DG_LAB 协议并发事件订阅刷流 | WebSocket 连接 token 单玩家唯一；同 token 重复连接踢出旧连接 |
| 配置文件热加载攻击 | 重载期间读写锁；新配置必须经过签名（HMAC）才允许生效 |
| 玩家离线修改状态 | `PlayerStateAttachment` 写入触发 `setData()` → `setChanged()`；服务端是唯一权威 |

### 2.2 审计日志（强制）

| 字段 | 类型 | 说明 |
|---|---|---|
| `actor_uuid` | UUID | 操作者（玩家或管理员） |
| `actor_type` | enum | `PLAYER` / `ADMIN_CMD` / `RUST_SERVICE` / `HARDWARE_DGLAB` |
| `target_uuid` | UUID | 被操作者（玩家或账户） |
| `target_type` | enum | `PLAYER` / `BANK_ACCOUNT` / `CONTRACT` / `CORE_POD` |
| `op` | string | 操作类型（如 `bank.transfer`、`pod.produce`、`contract.sign`） |
| `before` | JSONB | 操作前状态快照 |
| `after` | JSONB | 操作后状态快照 |
| `tick_millis` | BIGINT | 服务端时间戳 |
| `request_id` | UUID | 幂等性去重 key |

审计表按月分区（`audit_2026_06`、`audit_2026_07` ...）；超过 100k 行的月份自动 ARCHIVE 到 `audit_archive/`。

### 2.3 设备锁定（资产防作弊）

- 每张 `bank_card` 在首次签发时绑定一个**设备 ID**（生成时使用 `device_lock` DataComponent）。
- 默认设备为「该卡片首次被使用的 `Level` 维度」+「首次扫描的客户端 IP hash」。
- 同一卡片在不同设备使用时，**只允许查询与展示**，**不允许转账/取款/存款**。
- 解绑必须通过 `invite_code`（UUIDv4）：原设备生成 invite → 新设备接收 invite → 服务端验证 → 解锁。
- 玩家丢失设备：管理员可通过 `/biocapital admin reset_device <player>` 重置（需权限 3）；此操作**强制审计**。

---

## 3. 服务器优化指标（仅描述）

> 参见 `00-overview.md` 第 5 节。具体预算在 `14-rust-services.md` 的「SLO」中统一协调。

每个业务模块的「性能影响」段落必须回答：

- 主线程 tick 影响（ms / tick）
- 异步任务频率（如每秒审计 flush、PostgreSQL 同步频率）
- 内存增长（按玩家数 O(n)？常数？）

---

## 4. 命名与注册规范

### 4.1 注册名（ModId）

- 全局唯一 modid：`create_biocapital`（不变）
- 所有注册资源 namespace 统一 `create_biocapital`
- 资源路径必须 snake_case

### 4.2 物品/方块/流体命名

- **注册名**：`create_biocapital:<snake_case_name>`（如 `core_pod`）
- **显示名（中）**：中文翻译键 `item.create_biocapital.<name>`
- **显示名（英）**：英文翻译键 `item.create_biocapital.<name>`

### 4.3 配方类型（Create 集成）

| 类型 | 注册名 | 说明 |
|---|---|---|
| `MixingRecipe` | `create:mixing` | 流体+流体混合 |
| `MechanicalMixer` 处理 | `create:mixing` | 同上，由机械搅拌机处理 |

---

## 5. 网络与同步

- **客户端 → 服务端**：gRPC（高频生产事件、银行转账、合约操作）
- **服务端 → 客户端**：gRPC stream（银行历史回放、合约状态变更）
- **Rust ↔ PostgreSQL**：sqlx prepared statement
- **Rust ↔ DG_LAB**：WebSocket（直接 Rust，不经 Java）
- **Rust ↔ Web UI**：HTTP REST + Server-Sent Events

### 5.1 协议版本

- gRPC schema 版本：`v1`（固定在 `proto/biocapital.proto` 的 `package biocapital.v1;`）
- 任何 breaking change 必须新建 `v2` 并保留 `v1` 至少两个 minor 版本

---

## 6. 测试与验证

### 6.1 单元测试

- Rust：`cargo test`（覆盖率目标 ≥80% 核心模块）
- Java：`gradlew test`（仅用于 Sable 桥接、Create 集成类）

### 6.2 集成测试

- 每个 gRPC endpoint 必须有 happy path + 3 种异常路径测试
- 每个 PostgreSQL migration 必须有 rollback 测试

### 6.3 端到端测试

- `/test/biocapital/e2e/` 下放端到端脚本（Rust 驱动 gRPC client + 真 PG 实例）

---

## 7. CHANGELOG 规范

- 每次模块修改必须在 `CHANGELOG.md` 中追加：
  - `## [模块号] - YYYY-MM-DD`
  - `### Added` / `### Changed` / `### Deprecated` / `### Removed` / `### Fixed` / `### Security`
- 与 99-integration-matrix.md 的依赖条目同步更新。

---

## 8. 不允许的事

- ❌ 在 Java 端维护任何经济账本或合约逻辑
- ❌ 客户端与服务端时间戳混用（必须统一为服务端 `tick_millis`）
- ❌ 任何对外部 IP/Port 的硬编码（必须在配置文件中可改）
- ❌ 任何对玩家 UUID 的字符串拼接（必须使用 `UUID.fromString()` + 异常处理）
- ❌ 任何对 `Math.random()` 的使用（经济逻辑必须使用安全随机）
- ❌ 任何对原版 `minecraft:lavabucket`、`minecraft:dirt` 等注册名的覆盖（必须新建生物/方块/流体）
