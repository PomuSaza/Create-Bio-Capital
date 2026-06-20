---
module: 13-bio-customization
status: canonical
audience: all contributors
last_reviewed: 2026-06-14
depends_on: 00-overview, 01-cross-cutting-concerns, 06-hostile-mobs
---

# 生物自定义系统（Bio-Customization）

> 用户原始要求：「生物部分需要能够在配置文件的文件夹下有一个目录及配置文件，能够直接替换掉原有的材质来自定义生物的样貌所播放的音频及动画，并写好要求及占位文件」。

---

## 1. 总览

### 1.1 设计目标

- 服务器管理员可**直接替换**生物的模型/贴图/动画/音频，**不**需要重新编译模组。
- 占位文件由项目提供，**实际资源由用户提供**。
- 用户修改后，**热加载**生效（不需要重启服务器）。
- **2026-06-20 D6 决策**：生物**不**注册新 EntityType（**不**替换异名），而是**完全改写**原版 EntityType。详见 `doc/06-hostile-mobs.md` §1.3。
- **2026-06-20 D9 决策**：服务器可下发自定义资源到玩家本地（**周期性** hash 检查 + 同步）。详见 `doc/06-hostile-mobs.md` §2.5 + `doc/01-cross-cutting-concerns.md` §1.4。

### 1.2 文件位置（**D6/D15 双目录**）

> **D15 决策**：玩家本地与服务器下发是**两个独立目录**。服务器下发**不污染**玩家本地。

**玩家本地**（离线 / 单人 / 自开服）：

```
<minecraft_dir>/config/biocapital/creatures/<creature_id>/
├── behavior.json              # 行为 JSON（D6 决策；详见 06 §2.4）
├── textures/
│   └── <creature_id>.png        # 贴图（占位）
├── sounds/
│   ├── ambient.ogg
│   ├── hurt.ogg
│   ├── death.ogg
│   └── step.ogg
├── geo/
│   └── <creature_id>.geo.json
└── animations/
    ├── <creature_id>.animation.json
    └── <creature_id>.idle.animation.json
```

**服务器下发**（在线）：

```
<minecraft_dir>/config/biocapital-online/<server_id>/creatures/<creature_id>/
├── behavior.json
├── textures/<creature_id>.png
├── sounds/...
├── geo/...
└── animations/...
```

> 玩家进服时 Rust 推送；每 5 min hash 检查后增量同步（D16 决策）。
> 玩家断服 → 切回玩家本地。

---

## 2. creatures.json 格式

### 2.1 字段

```json
{
  "creature_id": "variant_zombie",
  "display_name": {
    "zh_cn": "变体僵尸",
    "en_us": "Variant Zombie"
  },
  "model_source": "minecraft:zombie",     // 继承哪个原版 mob
  "creature_type": "MONSTER",
  "geckolib_format_version": 2,
  "audio": {
    "ambient": "sounds/ambient.ogg",
    "hurt": "sounds/hurt.ogg",
    "death": "sounds/death.ogg",
    "step": "sounds/step.ogg",
    "volume": 1.0,
    "pitch": 1.0
  },
  "textures": {
    "main": "textures/variant_zombie.png",
    "overlay": null
  },
  "model": {
    "geo": "geo/variant_zombie.geo.json",
    "animations": [
      "animations/variant_zombie.animation.json"
    ],
    "idle_animation": "animations/variant_zombie.idle.animation.json",
    "scale": 1.0
  },
  "stats_override": {
    "max_health": 20.0,
    "attack_damage": 3.0,
    "movement_speed": 0.23
  },
  "drops_override": null,
  "replaces": "minecraft:zombie",
  "tags": ["monster", "undead", "biocapital_variant"]
}
```

### 2.2 字段说明

| 字段 | 类型 | 说明 |
|---|---|---|
| `creature_id` | string | 唯一标识（snake_case） |
| `display_name` | object | 中英文显示名 |
| `model_source` | string | 继承哪个原版 mob 的 AI/HP/攻击 |
| `creature_type` | enum | `MONSTER` / `PASSIVE` / `BOSS` |
| `geckolib_format_version` | int | 当前固定 2 |
| `audio.*` | object | 音频路径 + 音量 + 音高 |
| `textures.*` | object | 贴图路径（main + 可选 overlay） |
| `model.*` | object | Geckolib 模型 + 动画路径 |
| `stats_override` | object | 可覆盖 HP/攻击/速度 |
| `drops_override` | object | 可覆盖掉落表 |
| `replaces` | string | 替换哪个原版 mob |
| `tags` | array | 标签（用于过滤） |

---

## 3. 资源格式规范

### 3.1 贴图（PNG）

- 分辨率：64×64（标准）、128×128（高清）、256×256（超高清）
- 格式：PNG，RGBA
- 命名：`<creature_id>.png`（小写 + 下划线）
- 大小限制：< 1 MB

### 3.2 音频（OGG）

- 格式：OGG Vorbis
- 采样率：44100 Hz
- 声道：单声道
- 时长：< 30 秒（环境音可循环）
- 大小限制：< 500 KB / 文件

### 3.3 模型（Geckolib 2.x `.geo.json`）

```json
{
  "format_version": "1.12.0",
  "minecraft_version": "1.17.0",
  "visible_box_width": 2.0,
  "visible_box_height": 2.0,
  "visible_box_length": 2.0,
  "model_name": "variant_zombie",
  "model_identifier": "biocapital:variant_zombie",
  "textures": ["variant_zombie"],
  "elements": [
    {
      "name": "head",
      "box": { "origin": [-4, 24, -4], "size": [8, 8, 8] },
      "faces": {
        "north": { "uv": [0, 0], "texture": "#head" }
      }
    }
  ],
  "bones": [
    {
      "name": "root",
      "pivot": [0, 0, 0],
      "cubes": [
        { "origin": [-4, 0, -4], "size": [8, 24, 8], "uv": [0, 0] }
      ]
    }
  ]
}
```

### 3.4 动画（Geckolib 2.x `.animation.json`）

```json
{
  "format_version": "1.8.0",
  "animations": {
    "idle": {
      "loop": true,
      "animation_length": 1.0,
      "bones": {
        "head": {
          "rotation": ["math.sin(query.anim_time*180*2)/8", 0, 0]
        }
      }
    },
    "walk": {
      "loop": true,
      "animation_length": 0.5,
      "bones": {
        "leg_left": {
          "rotation": ["math.sin(query.anim_time*360*2)/4", 0, 0]
        }
      }
    }
  }
}
```

---

## 4. 占位文件策略

### 4.1 占位文件命名

每个期望资源对应一个 `<expected_name>.<expected_format>.txt` 文件：

- `textures/variant_zombie.png.txt` —— 描述期望的 PNG 内容
- `sounds/ambient.ogg.txt` —— 描述期望的 OGG 内容
- `geo/variant_zombie.geo.json.txt` —— 描述期望的 Geckolib geo 内容

### 4.2 占位文件内容（示例）

`textures/variant_zombie.png.txt`：
```
PLACEHOLDER FILE
================
Expected: PNG image, 64x64, RGBA
Subject: humanoid female figure with pale skin
Palette: skin tones (#F5D5C0 to #D4A89A), hair (light brown #B8896D),
         clothing (dark gray #3F3F3F)
Style: anime-style, full-body visible
Notes: All minecraft model UV mapping conventions apply.
       This file is REPLACED by the user with the actual PNG.
       The mod will fall back to magenta-black if no PNG is provided.
```

### 4.3 资源缺失处理

- 如果用户**未**提供实际资源（仅留 `.txt` 占位）：
  - Java 端读取占位 → 加载 magenta-black fallback
  - 日志 WARN：`<creature_id> missing texture: ...`
  - 不阻塞游戏运行
- 详细策略见 `17-asset-placeholders.md`

---

## 5. 热加载

### 5.1 文件监听

- Java 端通过 `WatchService` 监听 `config/biocapital/creatures/` 目录
- 文件创建 / 修改 / 删除 → 重新加载对应 creature

### 5.2 资源路径解析

- `creatures.json` 中路径相对于 `config/biocapital/creatures/<creature_id>/`
- 绝对路径**不允许**
- `../` 路径**不允许**（安全）

### 5.3 错误处理

- JSON 解析失败：保留上一次成功配置 + 日志 ERROR
- 缺失资源：magenta-black fallback + 日志 WARN
- 模型格式错误：游戏崩溃（不可恢复）；保留上一次成功配置直到用户修复

---

## 6. Rust 重写后的形态

### 6.1 服务端权威

- creatures.json 由 Rust 端解析 + 缓存
- Java 端仅读取（gRPC `GetCreatureConfig`）

### 6.2 gRPC 接口

```protobuf
service CreatureService {
  rpc ListCreatures(ListRequest) returns (CreatureListResponse);
  rpc GetCreature(CreatureRequest) returns (CreatureConfig);
  rpc ReloadCreatures(Empty) returns (ReloadResponse);
}
```

### 6.3 PostgreSQL 表

```sql
CREATE TABLE creature_configs (
  creature_id VARCHAR(64) PRIMARY KEY,
  config JSONB NOT NULL,
  loaded_tick BIGINT NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT true
);
```

---

## 7. 默认生物清单（占位）

| creature_id | replaces |
|---|---|
| `variant_zombie` | `minecraft:zombie` |
| `variant_skeleton` | `minecraft:skeleton` |
| `variant_creeper` | `minecraft:creeper` |
| `variant_spider` | `minecraft:spider` |
| `variant_enderman` | `minecraft:enderman` |
| `variant_witch` | `minecraft:witch` |
| `variant_pillager` | `minecraft:pillager` |
| `variant_zombified_piglin` | `minecraft:zombified_piglin` |
| `variant_hoglin` | `minecraft:hoglin` |
| `variant_piglin_brute` | `minecraft:piglin_brute` |

> 全部提供占位文件于 `assets/creatures/_template/`（模板由本仓库提供）。

---

## 8. 性能影响

- 主线程 tick：每个变体实体渲染 0.2 ms（一次性）
- 异步任务：creatures.json 热加载（tokio task）
- 内存：每个 creature config 约 4 KB

---

## 9. 联动点

- `CreatureConfigReloadedEvent` —— 配置文件热加载完成
- `CreatureMissingAssetEvent` —— 缺失资源时
- KubeJS：`events.onCreatureConfigReload(event => { event.creatureId })`

---

## 10. 验收标准

- [ ] creatures.json schema 正确
- [ ] Geckolib 模型 + 动画可加载
- [ ] 热加载生效（修改文件后立即生效）
- [ ] 资源缺失 → magenta-black fallback，不崩溃
- [ ] 路径遍历攻击防御（`../` 拒绝）
- [ ] PostgreSQL 表 + 索引齐全
- [ ] gRPC 接口齐全
