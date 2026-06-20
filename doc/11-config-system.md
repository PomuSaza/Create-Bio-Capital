---
module: 11-config-system
status: canonical
audience: all contributors
last_reviewed: 2026-06-14
depends_on: 00-overview, 01-cross-cutting-concerns, 02-player-state, 08-bank
---

# 配置文件系统

> 详细规范见 `01-cross-cutting-concerns.md` 第 1 节。
> 本文件具体定义每个配置文件的字段。

---

## 1. Java 端 TOML — `config/create_biocapital.toml`

### 1.1 文件位置

`<minecraft_dir>/config/create_biocapital.toml`

### 1.2 字段（按节）

```toml
# ── HUD ────────────────────────────────────────────────
[HUD_Display]
showHUD = true
showPleasureBar = true
showHungerBar = true
showPercentages = true
debugHUD = false

[HUD_Layout]
hudX = 10
hudY = -40
barWidth = 80
barHeight = 16
barSpacing = 4

[HUD_Colors]
hungerColor = "#CCFFAA00"        # ARGB
pleasureColor = "#CCFF69B4"      # ARGB
backgroundColor = "#66333333"    # ARGB
textColor = "#FFFFFFFF"          # ARGB

# ── PlayerState ────────────────────────────────────────
[PlayerState]
defaultStatMax = 100.0
defaultHiddenHp = 20.0
spawnPleasure = 0.0
spawnHunger = 20.0
statMin = 0.0
copyOnDeath = true

[PlayerState.Sources]
sweetBerriesAdd = 5.0
glowBerriesSub = 5.0
mobAttackBase = 2.0
lavaPerTick = 0.5
charmPotionPerTick = 0.3

# ── BodyDevelopment ────────────────────────────────────
[BodyDevelopment]
maxDev = 100.0
feetMovementBonusPerPoint = 0.005
chestStressBonusPerPoint = 0.01
armsInteractionBonusPerPoint = 0.0005
mouthEfficiencyBonusPerPoint = 0.01
bellyMaxHungerBonusPerPoint = 0.5
genitalPleasureRateBonusPerPoint = 0.005

[BodyDevelopment.Sources]
sweetBerriesGenitalAdd = 0.5
highTideImmersionPerTickChest = 0.1
highTideImmersionPerTickBelly = 0.1
defeatEndGenitalAdd = 5.0
defeatEndBellyAdd = 5.0
runningPer1000BlockFeetAdd = 0.5
handCrankPer100ArmsAdd = 0.5
potionUsePerMouthAdd = 0.3
bingeEatingBellyAdd = 1.0

# ── CorePod ────────────────────────────────────────────
[CorePod]
generatedRpm = 16.0
generatedStress = 4.0
selfStress = 0.5
recipeIntervalTicks = 100
tankCapacityMb = 1000

[CorePod.Hosting]
maxEnduranceTicks = 24000
offlineDecayMultiplier = 2.0
debuffSlowness = 2
debuffMiningFatigue = 2
debuffWeakness = 1
debuffJump = -2

[CorePod.Inputs]
# 接受作为输入的流体列表（必须包含 create_biocapital:high_tide）
# 默认接受任何 create_biocapital:* 流体
allowedInputFluids = [
  "create_biocapital:high_tide",
  "create_biocapital:semen",
  "create_biocapital:charm_potion"
]

[CorePod.Outputs]
outputFluid = "create_biocapital:high_tide"
outputFluidAmount = 1              # mB / cycle
outputItem = "create_biocapital:desire_fragment"
outputItemCount = 1

# ── HostileMobReplacement ──────────────────────────────
[HostileMobReplacement]
enabled = true
removeSpawnEggs = true
whitelist = []
blacklist = ["minecraft:ender_dragon", "minecraft:wither"]

[HostileMobReplacement.Drops]
desireFragmentDropChance = 0.03

[HostileMobReplacement.SpawnEggs]
registerVariantSpawnEggs = false

# ── Environment ────────────────────────────────────────
[Environment]
lavaPleasurePerSecond = 10.0
lavaAbovePleasurePerSecond = 1.0
swampMudPleasurePerSecond = 5.0
swampMudHungerPerSecond = 0.1
swampMudMovementSlowdownPct = 0.5
sandPleasurePerSecond = 0.05
magmaBlockPleasurePerSecond = 3.0

# ── Bank ───────────────────────────────────────────────
[Bank]
maxBalance = 100000000
historySize = 16
deviceLockDefaultMode = "DIMENSION_AND_IP_HASH"
inviteCodeExpirySeconds = 600

[Bank.AtmInsertRateLimit]
# 防止同一 ATM 同一 tick 多次插入
perTickInsertLimit = 1

# ── Contracts ──────────────────────────────────────────
[Contracts]
dailyPayoutTimeTick = 24000       # 服务端 tick (每日 00:00)
maxActiveContractsPerPlayer = 5
defaultRedemptionCost = 10000

# ── DGLAB ──────────────────────────────────────────────
[DGLAB]
enabled = true
defaultWebSocketPort = 9700
heartbeatIntervalSeconds = 30
maxStrength = 200
pleasureToStrengthMultiplier = 2.0
offlineStandbyEnabled = true

# ── Web UI ─────────────────────────────────────────────
[WebUI]
enabled = true
defaultHttpPort = 8080
allowedOrigins = ["http://localhost:3000"]
sessionTimeoutMinutes = 30
adminSessionTimeoutMinutes = 60

# ── Logging ────────────────────────────────────────────
[Logging]
auditLogRetentionDays = 90
pgBackupAt = "03:00"
pgBackupRetentionDays = 7
```

### 1.3 校验规则

- 所有 `defineInRange` 字段越界 → 回退默认值 + 日志 WARN
- 颜色字段：`#RRGGBBAA` 或 `#RRGGBB`（ARGB 顺序：前两个为 alpha）
- 列表字段：空列表与缺失等价

### 1.4 热重载

- `Config.load()` 暴露 reload API
- `/biocapital config reload` 命令触发 reload（详见 12）
- 不需要重启服务器即可生效

---

## 2. Rust 服务端 TOML — `config/biocapital-server.toml`

### 2.1 文件位置

`<minecraft_dir>/config/biocapital-server.toml`

### 2.2 字段

```toml
# ── Server ─────────────────────────────────────────────
[Server]
bindAddress = "127.0.0.1"
grpcPort = 50051
httpPort = 8080
websocketPort = 9700
shutdownGracefulTimeoutSeconds = 30

# ── PostgreSQL ────────────────────────────────────────
[PostgreSQL]
connectionString = "postgresql://biocapital:biocapital@localhost:5432/biocapital"
maxConnections = 10
minConnections = 2
acquireTimeoutSeconds = 10
# 用于启动期 init / 心跳
initOnStartup = true
heartbeatIntervalSeconds = 60

# ── Backup ─────────────────────────────────────────────
[Backup]
enabled = true
backupTimeOfDay = "03:00"           # 24h format
backupRetentionDays = 7
backupDestination = "../biocapital-backups"   # 相对 minecraft_dir
compressionFormat = "gzip"

# ── DG_LAB ─────────────────────────────────────────────
[DGLAB]
enabled = true
heartbeatIntervalSeconds = 30
reconnectMaxAttempts = 3
reconnectIntervalSeconds = 5

# ── Logging ────────────────────────────────────────────
[Logging]
level = "info"
file = "../logs/biocapital-server.log"
fileRotation = "daily"
fileMaxSizeMb = 100
fileMaxFiles = 30

# ── Metrics ────────────────────────────────────────────
[Metrics]
prometheusEnabled = false
prometheusPort = 9090
```

### 2.3 校验规则

- `connectionString` 必须包含 `postgresql://`；格式错误则启动失败
- `backupTimeOfDay` 必须为 `HH:MM` 24h 格式
- `bindAddress` 不允许 `0.0.0.0` 除非显式 `allowPublicBind = true`

### 2.4 热重载

- `SIGHUP` 信号触发 reload
- 文件 `notify` watcher 触发 reload（任一即可）

---

## 3. 第三方挂接点 — `config/biocapital-hooks/*.json`

### 3.1 文件位置

`<minecraft_dir>/config/biocapital-hooks/*.json`

### 3.2 格式

```json
{
  "hook_id": "custom_event_listener",
  "version": "1.0.0",
  "events": ["PlayerStateChangeEvent", "BankTransactionEvent"],
  "actions": [
    {
      "type": "KUBEJS_SCRIPT",
      "script": "events.onPlayerStateChange(e => { /* ... */ })"
    },
    {
      "type": "HTTP_WEBHOOK",
      "url": "http://localhost:8000/webhook",
      "method": "POST",
      "timeout": 5000
    }
  ]
}
```

### 3.3 校验规则

- `hook_id` 必须全局唯一
- `events` 必须是 `00-overview.md` 第 6 节模块索引中列出事件
- 脚本（KubeJS）在 sandbox 中执行；禁止 `require('fs')` 等敏感 API

### 3.4 热加载

- 文件变化时自动重新解析
- 错误配置日志 ERROR 但不阻断其他 hook

---

## 4.5 双目录配置 + 服务器覆盖（2026-06-20 D15/D16/D17 决策）

> **D15 决策**：玩家本地与服务器下发是**两个独立目录**。服务器下发**不污染**玩家本地。

### 4.5.1 玩家本地（离线 / 单人 / 自开服）

```
<minecraft_dir>/config/
├── create_biocapital.toml           (Java 端)
├── biocapital-server.toml           (Rust 端)
├── biocapital-hooks/                (第三方 hook)
│   └── *.json
├── biocapital/
│   ├── creatures/<mob_id>/
│   │   ├── behavior.json            (D6 决策；详见 06 §2.4)
│   │   ├── textures/<mob_id>.png
│   │   ├── sounds/{ambient,hurt,death,step}.ogg
│   │   ├── geo/<mob_id>.geo.json
│   │   └── animations/<mob_id>.animation.json
│   └── status/                      (D13 决策；状态 icon)
│       └── <effect_id>.png
└── biocapital-online/                (服务器下发；**不**追踪 git)
    └── <server_id>/
        ├── biocapital-server.toml  (服务器覆盖版本)
        ├── biocapital/
        │   ├── creatures/<mob_id>/behavior.json
        │   └── status/<effect_id>.png
        └── ...
```

### 4.5.2 服务器下发（在线）

- **进服时拉取** + **每 5 分钟 hash 检查**（D16 决策 B）
- MC 客户端启动期检测 `online/<server_id>/` 目录存在 → 用其配置；否则用 `config/biocapital/`
- Rust 主动 push resources_index；客户端按需下载
- **所有配置**（包括本地化 log_level / audit_path）都被服务器覆盖（D17 决策）
- 玩家断服 → 切回 `config/biocapital/`（玩家本地）

### 4.5.3 `[Recovery]` 节（2026-06-20 D5 决策）

```toml
# 战败恢复所需猫草数量（可配置）
[Recovery]
cat_grass_cost = 50  # 默认 50；玩家/服主可调
```

详见 `doc/02-player-state.md` §3.4。

## 5. 文件系统布局

详见 `13-bio-customization.md`。

---

## 5. 文件系统布局

```
<minecraft_dir>/
├── config/
│   ├── create_biocapital.toml       (Java 端)
│   ├── biocapital-server.toml       (Rust 端)
│   ├── biocapital-hooks/             (第三方 hook)
│   │   ├── *.json
│   └── biocapital/
│       └── creatures/
│           └── <creature_id>/
│               ├── behavior.json  (D6)
│               ├── textures/*.png
│               ├── sounds/*.ogg
│               ├── geo/*.geo.json
│               └── animations/*.animation.json
├── saves/                            (Minecraft 原版)
└── biocapital/                       (本模组运行时数据，save 同级)
    ├── backups/                      (PG 异地备份)
    ├── db/                           (PG data dir)
    └── logs/                         (Rust 服务日志)
```

> **`<minecraft_dir>/biocapital/` 与 `saves/` 同级**，满足用户原始要求。

---

## 6. 不允许的事

- ❌ 任何硬编码端口（除默认值外，必须在 toml 中可改）
- ❌ 任何硬编码 PG 凭据（必须在 toml 中可改）
- ❌ 任何硬编码颜色 / 数值（必须在 toml 中可改）
- ❌ 任何硬编码文件路径常量（必须用 `FMLPaths.CONFIGDIR` / Rust 的 `dirs` crate）
- ❌ 任何对外部 IP 的硬编码（WebSocket / HTTP 服务器必须默认监听 127.0.0.1）
