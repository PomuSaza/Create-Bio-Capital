---
module: 17-asset-placeholders
status: canonical
audience: all contributors
last_reviewed: 2026-06-14
depends_on: 00-overview, 01-cross-cutting-concerns
---

# 资源占位策略（Asset Placeholders）

> 用户原始要求：「遇到需要涉及材质模型等场景，生成一个替代的临时文件，例如一张图片那就是{你认为它应该有的标准命名}.{它应该有的格式}，而实际上是一个可以用文本编辑器打开的纯文本文档里面写了你对这个文件的要求」。
> 蓝图原始策略（已存在）：「对于任何图片/模型/音效，生成 `<expected_name>.<expected_format>.txt` 包含资产规范；用户提供真实资产；缺失资源不阻塞游戏」。

---

## 1. 占位文件命名

### 1.1 命名规则

`<expected_name>.<expected_format>.txt`

- `expected_name`：项目期望的真实资源名（如 `core_pod_side`）
- `expected_format`：项目期望的真实格式（如 `png` / `ogg` / `geo.json`）
- 后缀 `.txt`：表示这是占位文本描述（**不是**真实资源）

### 1.2 示例

| 期望资源 | 占位文件 |
|---|---|
| `core_pod_side.png` | `core_pod_side.png.txt` |
| `ambient.ogg` | `ambient.ogg.txt` |
| `variant_zombie.geo.json` | `variant_zombie.geo.json.txt` |
| `idle.animation.json` | `idle.animation.json.txt` |

---

## 2. 占位文件内容格式

### 2.1 头部

```
PLACEHOLDER FILE
================
```

### 2.2 必填字段

| 字段 | 格式 | 说明 |
|---|---|---|
| Expected | `<name>.<format>` | 期望的真实资源文件名 |
| Format | `<详细格式>` | PNG 8-bit RGBA / OGG Vorbis 44.1kHz / 等 |
| Size | `<width>×<height>` 或 `<duration>s` | 贴图尺寸 / 音频时长 |
| Subject | `<主题>` | 资源描述的内容 |
| Palette | `<色板>` | 主要颜色（hex） |
| Style | `<风格>` | 美术风格（如 anime-style / semi-realistic） |
| Notes | `<备注>` | 任何额外要求 |

### 2.3 示例：贴图占位

文件：`assets/create_biocapital/textures/block/core_pod_side.png.txt`

```
PLACEHOLDER FILE
================
Expected: core_pod_side.png
Format: PNG 8-bit RGBA
Size: 64×64 (or higher, must be power-of-2)
Subject: side panel of a sleek biomechanical pod — central pillar with
         glowing biomechanical glyphs, soft pink bioluminescent veins,
         a recessed slot for fluid injection visible at the bottom
Palette: primary #4A4A55 (cool gray), accent #FF69B4 (pleasure pink),
         glow #FFCCDD (soft pink), dark #1A1A20
Style: semi-realistic industrial, soft lighting, anime-inspired curves
Notes:
  - This file is REPLACED by the user with the actual PNG.
  - The mod will fall back to magenta-black if no PNG is provided.
  - UV mapping: standard 16×16 minecraft block (top/bottom/sides split).
```

### 2.4 示例：音频占位

文件：`config/biocapital/creatures/variant_zombie/sounds/ambient.ogg.txt`

```
PLACEHOLDER FILE
================
Expected: ambient.ogg
Format: OGG Vorbis, mono, 44100 Hz
Size: < 30 seconds (loopable)
Subject: soft, breathy moan/vocalization — gentle and slow, evokes
         the creature being in a state of arousal
Palette (audio): warm mid-low frequencies (200–800 Hz primary),
                 occasional higher vocalization (1–3 kHz)
Style: intimate, slow, lo-fi reverb
Notes:
  - This file is REPLACED by the user with the actual OGG.
  - The mod will play no sound if no OGG is provided (no fallback).
  - Loopable: file must be seamless.
```

### 2.5 示例：模型占位

文件：`config/biocapital/creatures/variant_zombie/geo/variant_zombie.geo.json.txt`

```
PLACEHOLDER FILE
================
Expected: variant_zombie.geo.json
Format: Geckolib 2.x geo JSON
Schema: see https://github.com/bernie-g/geckolib/blob/master/GeoModelFormat.md
Subject: humanoid female figure, full-body, articulated bones
         (head, body, arms ×2, legs ×2, optional tail/wings)
Required bones: head, body, arm_left, arm_right, leg_left, leg_right
Optional bones: tail, wing_left, wing_right, breast_left, breast_right
Size: visible box 2.0×2.5×1.0
Style: anime-inspired, slender proportions, detailed face
Notes:
  - This file is REPLACED by the user with the actual geo JSON.
  - The mod will fall back to the inherited vanilla model if no geo provided.
  - UV mapping convention: 0,0 at top-left, increasing right/down.
```

---

## 3. 资源缺失处理

### 3.1 贴图缺失

- 显示 minecraft 原生 missing-texture（magenta-black）
- 日志 WARN：`Missing texture: <path>`
- 不阻塞游戏

### 3.2 模型缺失

- 块/物品：fallback 到原版方块模型（如 furnace）
- 实体：fallback 到原版 mob 模型（如 zombie）
- 日志 WARN：`Missing model: <path>`
- 不阻塞游戏

### 3.3 音频缺失

- **不**播放任何声音（无 fallback）
- 日志 WARN：`Missing audio: <path>`
- 不阻塞游戏

### 3.4 动画缺失

- 模型不播放任何动画（仅静态姿势）
- 日志 WARN：`Missing animation: <path>`
- 不阻塞游戏

### 3.5 配置缺失

- creatures.json 缺失：creature **不注册**
- 日志 ERROR：`Missing creature config: <creature_id>`
- 不阻塞其他 creature 加载

---

## 4. 占位文件位置

### 4.1 项目内（被 git 追踪）

- `src/main/resources/assets/create_biocapital/textures/**/*.png.txt`
- `src/main/resources/assets/create_biocapital/models/**/*.json.txt`
- `src/main/resources/assets/create_biocapital/blockstates/**/*.json.txt`

### 4.2 用户自定义（运行时热加载）

- `config/biocapital/creatures/<creature_id>/textures/*.png.txt`（占位）
- `config/biocapital/creatures/<creature_id>/textures/*.png`（实际资源）

---

## 5. 项目模板

### 5.1 状态 icon 占位（D13 决策，2026-06-20 新增）

> **D13 决策**：解包 RPG MVP.png 获取的美术资源用作**状态 icon**，放在 `config/biocapital/status/<effect_id>.png`（运行时目录）。

**占位文件**（项目内 git 追踪）：

```
src/main/resources/assets/create_biocapital/textures/status/
├── estrus.png.txt                 (D13 占位)
├── rune_marked.png.txt            (D13 占位)
├── pleasure_overload.png.txt      (D13 占位)
└── unknown.png.txt                (兜底 icon)
```

**运行时路径**：

- 玩家本地：`config/biocapital/status/<effect_id>.png`（可由用户自定义）
- 服务器下发：`config/biocapital-online/<server_id>/status/<effect_id>.png`（D9 决策；周期性同步）

**回退链**（D13）：
1. 服务器下发 `config/biocapital-online/<server_id>/status/<effect_id>.png`（在线时）
2. 玩家本地 `config/biocapital/status/<effect_id>.png`（离线时）
3. mod jar 内置占位（fallback 永不报错；缺失 → magenta missing texture + WARN）

### 5.2 生物行为 JSON 占位（D6 决策，2026-06-20 新增）

> **D6 决策**：每个 mob 一个 `behavior.json`，放在 `config/biocapital/creatures/<mob_id>/behavior.json`。

**占位文件**（项目内 git 追踪）：

```
doc/assets/creatures/_template/
└── behavior.json.txt              (模板；玩家 / 服主复制后改)
```

**示例内容**（`behavior.json`）：

```json
{
  "mob_id": "minecraft:zombie",
  "attack_effects": [
    {
      "trigger": "on_attack_hit_player",
      "apply_living_effect": {
        "effect_id": "estrus",
        "duration_ticks": 600,
        "amplifier": 1
      }
    }
  ]
}
```

**详细 schema** 见 `doc/06-hostile-mobs.md` §2.4。

---

## 6. 不允许的事

### 5.1 模板位置

`doc/assets/creatures/_template/`

### 5.2 模板清单

| 文件 |
|---|
| `creatures.json.txt` |
| `textures/<creature_id>.png.txt` |
| `sounds/ambient.ogg.txt` |
| `sounds/hurt.ogg.txt` |
| `sounds/death.ogg.txt` |
| `sounds/step.ogg.txt` |
| `geo/<creature_id>.geo.json.txt` |
| `animations/<creature_id>.animation.json.txt` |
| `animations/<creature_id>.idle.animation.json.txt` |

> 详见 `doc/assets/creatures/_template/` 下的占位文件。

---

## 6. 不允许的事

- ❌ 在项目代码中硬编码任何二进制资源（必须用占位 + 描述）
- ❌ 在 .gitignore 中忽略 .png.txt / .ogg.txt / .geo.json.txt（这些是文档，必须提交）
- ❌ 任何「我自己生成 1×1 PNG 占位」的尝试（必须使用 .txt 占位）
- ❌ 任何真实美术内容（即使是 1×1 测试像素）
