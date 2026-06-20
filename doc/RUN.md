# RUN — 如何跑通这个项目（2026-06-20 MVP-3）

> 给你一份**最小可运行**的 run 步骤 + **怎么核实**到底做到了什么程度。
> **不**含 Minecraft GUI 启动（需要 mod loader + 实际 Minecraft 客户端）。

---

## TL;DR（30 秒版）

```bash
# 0. 一行依赖检查
which docker psql cargo java && echo "all deps OK"

# 1. 启 PG（fresh）
pg_ctl -D /tmp/pgdata -l /tmp/pg.log start 2>&1 || (
  initdb -D /tmp/pgdata -U biocapital --auth=trust
  echo "listen_addresses = '127.0.0.1'" >> /tmp/pgdata/postgresql.conf
  echo "port = 5432" >> /tmp/pgdata/postgresql.conf
  echo "unix_socket_directories = '/tmp'" >> /tmp/pgdata/postgresql.conf
  pg_ctl -D /tmp/pgdata -l /tmp/pg.log start
)
PGUSER=biocapital createdb -h 127.0.0.1 -p 5432 biocapital

# 2. 编 Rust native lib + 跑 SQL 迁移
cd rust && cargo build -p biocapital-jni
mkdir -p ../build/natives/linux-x86_64
cp target/debug/libbiocapital_jni.so ../build/natives/linux-x86_64/
cd .. && BIOCAPITAL_SERVER_TOML=config/biocapital-server.toml \
  ./rust/target/debug/biocapital-cli migrate

# 3. 启 Rust server（另一个 terminal）
./rust/target/debug/biocapital-cli start

# 4. 跑 6/6 e2e 测试
ADMIN_TOKEN=76305866da86b16805b8b3572aa9e7912702f81b443a4e3d4e9401ab072f8232 \
  bash scripts/e2e.sh

# 5. 跑全套 Rust + Java 测试
cd rust && cargo test --workspace --exclude biocapital-pg
cd .. && ./gradlew test -x buildRustNatives
```

如果上面全部绿色通过 = **Rust 端端到端 OK**（HTTP / gRPC / SSE / PG / 6 个 e2e 6/6 PASS）。

---

## 详细步骤

### 0. 依赖检查

| 依赖 | 用途 | 版本要求 |
|---|---|---|
| `rustc` / `cargo` | 编 JNI lib + Rust server | 1.96+（rust-toolchain.toml） |
| `postgres` 18+ | PG 数据库 | 18.3 测试过；16/17 应可 |
| `psql` | seed 测试数据 + 验证 schema | 随 PG |
| `java` 17+ | 编译 mod | 17 (与 rust/docker/Dockerfile 一致) |
| `docker`（可选）| 编 cross-platform .so | 当前用 local cargo 绕过 |
| `gcc` / `cc` | cargo build 默认 linker | 系统默认 |

```bash
rustc --version    # rustc 1.96+
cargo --version
psql --version      # psql 18+
java -version       # openjdk 17+
```

### 1. 启 PostgreSQL

```bash
# 数据目录（dev 用 /tmp；生产用 <minecraft_dir>/biocapital/pgdata/）
mkdir -p /tmp/pgdata
chown -R $USER /tmp/pgdata

# 第一次：初始化
initdb -D /tmp/pgdata -U biocapital --auth=trust

# 监听配置（追加到 postgresql.conf）
echo "listen_addresses = '127.0.0.1'" >> /tmp/pgdata/postgresql.conf
echo "port = 5432" >> /tmp/pgdata/postgresql.conf
echo "unix_socket_directories = '/tmp'" >> /tmp/pgdata/postgresql.conf

# 启动
pg_ctl -D /tmp/pgdata -l /tmp/pg.log start

# 创建 biocapital DB（一次）
PGUSER=biocapital createdb -h 127.0.0.1 -p 5432 biocapital
```

### 2. 编 Rust native lib + SQL 迁移

```bash
# 2.1 编 JNI lib
cd rust
cargo build -p biocapital-jni

# 2.2 复制到 gradle 期望位置（Linux x86_64 演示）
mkdir -p ../build/natives/linux-x86_64
cp target/debug/libbiocapital_jni.so ../build/natives/linux-x86_64/

# 2.3 跑 SQL 迁移（D1 决策：runtime 目录）
cd ..
BIOCAPITAL_SERVER_TOML=config/biocapital-server.toml \
  ./rust/target/debug/biocapital-cli migrate
# 期望：16/16 SQL files ✓ 80ms
```

**config/biocapital-server.toml** 关键字段：
- `migration_dir = "rust/migrations/"`（dev 指向源码树）
- 生产请改为 `<minecraft_dir>/config/biocapital/sql/`

### 3. 启 Rust server

```bash
BIOCAPITAL_SERVER_TOML=config/biocapital-server.toml \
  ./rust/target/debug/biocapital-cli start
# 期望：Web UI listening 127.0.0.1:8080；gRPC server 127.0.0.1:50051；PG pool ready
```

### 4. 跑 6/6 e2e 测试（端到端）

```bash
ADMIN_TOKEN=76305866da86b16805b8b3572aa9e7912702f81b443a4e3d4e9401ab072f8232 \
  bash scripts/e2e.sh
# 期望：PASS: 6 / FAIL: 0
```

### 5. 跑全套测试套件

```bash
# Rust（含 e2e server 单元测试）
cd rust && cargo test --workspace --exclude biocapital-pg

# Java（wire format + JNI 集成）
cd .. && ./gradlew test -x buildRustNatives
```

---

## 怎么核实到底做到了什么程度（验证矩阵）

按"无 Minecraft / 有 Minecraft"两个环境区分。

### ✅ 无 Minecraft 也能验证（已实测 100%）

| 验证项 | 命令 | 期望 |
|---|---|---|
| Rust 编译 | `cd rust && cargo check --workspace` | Finished `dev` profile |
| Rust 测试 | `cd rust && cargo test --workspace --exclude biocapital-pg` | **280+ tests pass**（含 13 config + 2 sql_loader）|
| SQL 迁移幂等 | `biocapital-cli migrate` 跑 3 次 | 全部 `✓`；NOTICE `already partitioned, skipping` |
| Gradle 编译 | `./gradlew compileJava -x buildRustNatives` | BUILD SUCCESSFUL |
| Java 测试 | `./gradlew test -x buildRustNatives` | **10/10 tests pass**（7 wire format + 3 JNI integration）|
| Mod JAR 生成 | `./gradlew jar -x buildRustNatives` | `build/libs/create_biocapital-1.0-SNAPSHOT.jar`（~1 MB）|
| JNI 符号匹配 | `./gradlew test -x buildRustNatives --tests JniIntegrationTest` | "All 12 JNI symbols present" |
| Rust server 启动 | `biocapital-cli start` | Web UI + gRPC + PG pool ready |
| **端到端 HTTP API** | `bash scripts/e2e.sh` | **6/6 PASS** |
| Mod metadata | `unzip -l build/libs/create_biocapital-1.0-SNAPSHOT.jar` | 383 files；含 `@Mod(BioCapital.MODID)` 入口 |

### ⚠️ 需要 Minecraft 客户端才能验证（**未实测**）

| 验证项 | 怎么验证 | 当前状态 |
|---|---|---|
| Mod 加载（mod loader 启动）| 启动 Minecraft 1.21.1 + NeoForge + Create 6.0.10 + 装 mod JAR | **未实测**（需要真实 MC 客户端）|
| HUD 渲染（pleasure / hunger 条）| 进游戏后看左上 | 代码在 `PlayerStateAttachment.java`；渲染器**已删除**（D3 决策：HUD 模块待重建）|
| 战败 UI（粉色遮罩 + 求助按钮）| HP=0 时 | 代码未实现（MVP-2 任务 #39）|
| JNI 真实调用（Java → Rust → PG）| mod 运行起来调 NativeRustBindings.callPlayerState | **符号匹配已验证**；实际调用需 MC runtime |
| 实体 / 方块 / 物品注册 | 进游戏看 mod 列表 | mod JAR 含 383 个 class 文件；注册逻辑在 `*Init.java`（gradle 编译过 = 编译期 OK）|
| 银行 / 契约 / 资源同步实际跑 | 进游戏触发事件 | Web UI 端到端 OK；Java mod 端未集成触发器 |

### ❌ 当前完全未实现（标 MVP-0/1 后逐步补）

| 功能 | 状态 | 关联 |
|---|---|---|
| MVP-2 cat grass 战败恢复（D4/D5/D20）| 待实现 | task #39 |
| D8 living effect 系统 | doc only（§7 数据模型）；代码未写 | task #38 #39 |
| D9 资源周期同步 | doc only | 后续 |
| D15/D17 双目录 config 覆盖 | doc only | 后续 |
| D21 客户端硬校验 | doc only | 后续 |
| D26 奴隶契约 Web UI 创建 | doc only（Rust 端 ready）| 后续 |
| Web UI Dashboard (D18/D22) | doc only（`doc/20-web-ui-dashboard.md` 给 web UI Agent 的独立文档）| 后续 |

---

## 已知 run 限制

1. **Docker 不通 / buildRustNatives 失败** → 用 `cargo build -p biocapital-jni` 绕过（已在本 RUN.md 演示）
2. **PG 端口冲突** → 改 `biocapital-server.toml` 的 `[Server.PostgreSQL] port` 字段
3. **Gradle 配置缓存问题** → `./gradlew --stop` + `./gradlew clean` + 重试
4. **没有 Minecraft runtime** → MVP-2/3 中需要 mod loader 才能验；目前所有验证基于 Rust server + Java 单元测试

---

## 如果你要从 GitHub 拉新代码跑一遍

```bash
# 1. clone + 进入
git clone https://github.com/PomuSaza/Create-Bio-Capital.git
cd Create-Bio-Capital
git switch dev-raw0    # MVP-0 / 0.1 / 3 commits 都在这里

# 2. 一行依赖（如缺 cargo / postgres / java 17，先装）
# Fedora: sudo dnf install rust cargo postgresql-server java-17-openjdk gcc
# Ubuntu: sudo apt install rustc cargo postgresql-18 openjdk-17-jdk gcc
# macOS:  brew install rust postgresql@18 openjdk@17

# 3. 启 PG（如步骤 1.）

# 4. 跑全部"无 MC"验证
./gradlew test -x buildRustNatives
cd rust && cargo test --workspace --exclude biocapital-pg

# 5. 启 server + 跑 e2e
cd .. && ./rust/target/debug/biocapital-cli start &
ADMIN_TOKEN=76305866da86b16805b8b3572aa9e7912702f81b443a4e3d4e9401ab072f8232 \
  bash scripts/e2e.sh
```

---

## 引用

- `doc/CHANGELOG.md` 最新条目 — MVP-0/0.1/3.0/3.1 的诚实记录
- `doc/00-overview.md` §2.3 — 项目完成度（~75%）
- `doc/SYSTEM_PROMPT.md` §14 工作循环 + §21 审计回路
- `scripts/e2e.sh` — 6/6 PASS 的脚本（commit `4f5f075` 入仓）
- `wiki/JavaAudit.md` — Java 端审计（2026-06-18）