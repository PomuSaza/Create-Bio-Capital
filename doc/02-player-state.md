---
module: 02-player-state
status: canonical
audience: all contributors
last_reviewed: 2026-06-14
depends_on: 00-overview, 01-cross-cutting-concerns
---

# PlayerState — 玩家状态附件

> 替代原始蓝图第 2 节「核心数值与生物学模拟重构」。
> 实现细节见 `src/main/java/mo/dystopia/biocapital/state/PlayerStateAttachment.java`（现有 Java 实现，待 Rust 重写）。

---

## 1. 数据模型

### 1.1 Attachment Type（NeoForge 1.21）

> **2026-06-20 D2/D3 决策覆写**：原文档将"HP"建模为新池 `hidden_hp`。**完全错**。
> **实际**：HP = 原版 HP（vanilla），**条被隐藏**（不新建池）。`hidden_hp` **不存在**。
> "隐性战败" = **UI 遮罩层**（不是 buff）。

| 字段 | 类型 | 默认值 | 范围 | 含义 |
|---|---|---|---|---|
| `pleasure` | float | 0.0 | [0.0, 100.0] | **快感值**（HUD 显示）|
| `hunger` | float | 20.0 | [0.0, 100.0] | **饱食度 / 饥饿值**（HUD 显示）|
| `low_hp_hits` | int | 0 | [0, +∞) | HP=0 触发的次数（惩罚性记忆）|
| `part_dev` | `Map<BodyPart, float>` | 全 0.0 | 各部位 ≥0.0 | **部位开发度**（与状态独立；D4 决策）|
| `defeated` | bool | false | true/false | 当前是否处于战败状态 |
| `living_effects` | `Map<LivingEffectId, LivingEffectData>` | empty | - | **独立 living effect 系统**（D8 决策；不是 vanilla MobEffect）|

> **HP**：用 Minecraft 原版 `LivingEntity.getHealth()`；**不**新建 `hidden_hp` 池（**已删除**）。
> 修改默认值时必须同时在 `create_biocapital.toml` 的 `[PlayerState]` 节暴露（详见 11-config-system）。

### 1.2 持久化

- 通过 NeoForge Attachment `serialize(Codec)` 自动持久化到玩家 NBT。
- 玩家战败后（`PlayerEvent.Clone`）时按 `copyOnDeath=true` 自动复制。
- `low_hp_hits` 在战败时**不减半**，作为惩罚性记忆保留。

### 1.3 同步

- `AttachmentType.Builder#sync(StreamCodec)` 让客户端能渲染 HUD。
- `sendToPlayer(holder, to) => holder == to` —— 数据只发给宿主玩家自己。
- **战败状态**：D3 决策是 **UI 遮罩层**（不是 buff，不通过 MobEffect 同步）；走**独立通道** —— `ClientboundDefeatedStatePacket` / 服务端 `BiocapitalDefeatedStateEvent`。

### 1.4 战败状态字段（独立于 vanilla 同步）

- `defeated: bool` —— 当前是否处于战败
- `defeat_reason: DefeatReason` —— 战败原因（HP=0 / 特殊事件 / OP 强制）
- `defeat_started_at: long (tick_millis)` —— 战败起始服务端 tick（用于审计）
- `defeat_option_chosen: Option<DefeatOption>` —— 玩家是否已选求助选项；None = 未选

---

## 2. HUD 设计

### 2.1 视觉

- **废除**原版 HP 与 Hunger 的 GUI 渲染（通过 `GuiEvent` 拦截并 `event.setCanceled(true)`）。
- **新增**两条圆角条（与原版风格一致）：
  - 快感值条（粉色 `#CCFF69B4`）
  - 饥饿值条（橙色 `#CCFFAA00`）
- 两等高、等粗；左上角各显示「快感值」/「饱食值」文字标签；同一水平线显示百分比。
- 位置：参考现有 `Config.hudX / hudY`，默认 (10, -40)。
- **战败时**（D3）：整屏覆盖**绿色 UI 遮罩层** + **视角摇晃**（类似 nausea buff，但更严重），**不是**顶部 debuff 列表中的新项。

### 2.2 文字

- 中文：`create_biocapital.hud.pleasure` → 「快感值」
- 中文：`create_biocapital.hud.hunger` → 「饱食值」
- 英文：`create_biocapital.hud.pleasure` → "Pleasure"
- 英文：`create_biocapital.hud.hunger` → "Hunger"

### 2.3 Debug 模式

- `Config.debugHUD = true` 时额外显示 vanilla HP 与 low_hp_hits（仅自己可见）。
- 服务器管理员通过 `/biocapital stats <player>` 可看任意玩家全部数值（详见 12）。

### 2.4 战败 UI 遮罩层（D3 决策核心）

> **不是**注册成 buff；是**整屏覆盖层**。
> 参考 Minecraft 原版 nausea buff（绿色 + 视角摇晃）的实现，但**不是** MobEffectInstance。

- **触发**：HP=0（D4 决策：**立即触发**，不延迟 30s）
- **持续**：直到玩家选完救助选项
- **视觉**：
  - 整屏绿色 tint（α 0.6）
  - 视角轻微摇晃（random yaw drift ±5°）
  - 中心显示求助按钮（自定义 widget，**不**用 vanilla GUI 组件）
  - 按钮文字：「放弃物品回床」/「用猫草恢复」
- **背景音**：低频嗡鸣（可选）

---

## 3. 数值变化规则

### 3.1 快感值触发

| 来源 | 增量 |
|---|---|
| 食用甜浆果（`minecraft:sweet_berries`） | +5.0 |
| 食用发光浆果（`minecraft:glow_berries`） | -5.0 |
| 敌对生物攻击命中 | +2.0（按攻击力度缩放） |
| 岩浆/沼泽触觉 | +10.0 / tick（在流体中时） |
| 媚药水体浸泡 | +3.0 / tick |
| 战败恢复（猫草选项）| 清零 → 0（最低）|

> 以上数值均为初始默认值，可通过 `create_biocapital.toml` 的 `[PlayerState.Sources]` 节调整。

### 3.2 饥饿值触发

| 来源 | 增量 |
|---|---|
| 自然衰减 | -0.5 / tick |
| 食用普通食物 | 由食物 `nutrition` 决定 |
| 战败恢复（猫草选项）| 回满 → 100.0 |
| 腹部开发度 | 提高 maxHunger（详见 03-body-development）|

### 3.3 HP 触发（**vanilla HP，不是 hidden_hp**）

> **D2 决策**：HP = 原版 `LivingEntity.getHealth()`，**不**新建池。

| 来源 | 行为 |
|---|---|
| 原版任何伤害事件 | `livingEntity.hurt(damageSource, amount)` |
| 战败恢复（猫草选项）| HP 回满（vanilla `setHealth(getMaxHealth())`）|
| 战败恢复（放弃物品）| HP 回满 + 传送到 spawn point |

### 3.4 战败触发 + 恢复（D4 决策）

**触发**（D4：HP=0 **立即触发**，**不**延迟 30s）：

```
if (livingEntity.getHealth() <= 0.0F) {
    // 1. 阻止原版死亡
    livingEntity.setHealth(1.0F);  // 临时 HP=1 防止死亡触发
    // 2. 触发战败
    BiocapitalDefeatedStateEvent.fire(livingEntity, DefeatReason.HP_ZERO);
    // 3. 客户端显示 UI 遮罩层（D3）+ 求助按钮
}
```

**恢复选项**（D4：**两个选项并发弹出**，玩家点哪个走哪个）：

| 选项 | 行为 | 资源消耗 |
|---|---|---|
| **放弃物品回床** | 清除玩家身上所有物品 + 传送到 `respawnPos`（床/出生点）| 0 |
| **用猫草恢复** | HP 回满 + 清全部 **debuff** + 饱食度回满 + 快感值清零 | **N 单位猫草**（`[Recovery] cat_grass_cost`，**可配置**，默认 50）|

**关键约束**（D4/D5 用户原话）：
- 部位开发度**不**变（D5："**开发度不要给变啊**"）
- 战败后**不**走原版死亡路径，**不**生成死亡消息
- `low_hp_hits++`（每次战败 +1）
- 玩家选完选项 → `defeated=false` → UI 遮罩层消失

**反向 buff**（D5："**正向的不要管**"）：战败恢复时**不**清除正向 buff（如移动加速、抗性提升）；只清 debuff。

### 3.5 living effect 触发（D8 决策）

> **独立 living effect 系统**，不走 vanilla MobEffect（vanilla MobEffectInstance 有 duration timer，不适合"状态性"buff 如淫纹）。

详见 §7（新增）+ `doc/99-integration-matrix.md` §3 事件清单。

---

## 4. Rust 重写后的形态

### 4.1 Rust 中的存储位置

- **权威源**：PostgreSQL `player_state` 表（**不**含 `hidden_hp`，含 `defeated` / `defeat_*` 字段）。
- **Java 端缓存**：NeoForge Attachment（仅作为客户端渲染与离线时的本地暂存）。
- **同步路径**：
  - 服务端事件 → Rust gRPC `UpdatePlayerState(player_uuid, new_state)` → Rust 写 PG → Rust 通知 Java 客户端更新 Attachment。
  - 客户端 → Rust gRPC `RequestUpdatePlayerState(player_uuid, delta)` → Rust 校验 → 写 PG → 广播。

### 4.2 离线时的本地暂存

- 玩家离线时，Java 端 Attachment 仍持有最近一次同步值。
- 服务端从 PG 拉取最近状态用于离线时的反作弊校验。

### 4.3 战败状态的 Rust 端权威

- 服务端是**唯一**可设置 `defeated=true` 的实体
- Java 端只接收 `BiocapitalDefeatedStateEvent` 通知，**不**主动设置
- 客户端的"求助选项选择"通过 `RequestDefeatResolve(option, params)` gRPC 发回服务端，服务端验证 → 写 PG → 广播

---

## 5. 性能影响

- 主线程 tick：仅 HUD 渲染（≤1 ms / tick）
- 异步任务：心跳同步至 Rust 每 5 秒一次
- 内存：每玩家约 256 字节

---

## 6. 联动点

- `NeoForge.EVENT_BUS.addListener(BiocapitalDefeatedStateEvent.class)` —— 战败状态变化时触发
- `NeoForge.EVENT_BUS.addListener(BiocapitalLivingEffectEvent.class)` —— living effect 变化（D8 决策）
- KubeJS 绑定：`events.onDefeatedStateChange(event => { ... })` + `events.onLivingEffectChange(event => { ... })`
- JSON hook：在 `config/biocapital-hooks/player-state.json` 中声明外部监听器

---

## 7. Living Effect 系统（D8 决策，2026-06-20 新增）

> **不走 vanilla `MobEffect`**。原因：vanilla `MobEffectInstance` 有 **duration timer**，到时间自动移除，不适合"状态性" buff（如淫纹 — 写入后逻辑上未解除就不能解除）。
> 走 NeoForge 1.21 **`Attachment` 系统**（详见 `Documentation-main/docs/datastorage/attachments.md`）。

### 7.1 数据模型

```java
// player.getData(BIOCAPITAL_LIVING_EFFECTS) -> BiocapitalLivingEffects
public class BiocapitalLivingEffects {
    Map<LivingEffectId, LivingEffectData> effects;  // 按 effect id 索引
}

public class LivingEffectData {
    LivingEffectId id;                    // e.g. "estrus", "rune_marked"
    int amplifier;                        // 强度（0+）
    long applied_at_tick;                 // 服务端 tick_millis
    String source_actor_uuid;             // 谁施加的（mob / player / 事件）
    boolean removable;                    // 是否可移除（淫纹 = false）
    Map<String, Object> custom_state;     // 状态性数据（淫纹等级、印记数等）
}
```

### 7.2 注册

```java
public static final DeferredRegister<AttachmentType<?>> ATTACHMENT_TYPES =
    DeferredRegister.create(NeoForgeRegistries.ATTACHMENT_TYPES, MOD_ID);

public static final Supplier<AttachmentType<BiocapitalLivingEffects>> LIVING_EFFECTS =
    ATTACHMENT_TYPES.register("living_effects",
        () -> AttachmentType.builder(BiocapitalLivingEffects::new)
            .serialize(BiocapitalLivingEffects.CODEC)  // 持久化
            .copyOnDeath()                              // 战败后保留
            .sync(BiocapitalLivingEffects.STREAM_CODEC) // 同步到客户端
            .build()
    );
```

### 7.3 事件（D8 决策核心）

| 事件 | 触发时机 | 用途 |
|---|---|---|
| `BiocapitalLivingEffectEvent.Added` | 添加新 effect | 触发被动技能 / 渲染更新 |
| `BiocapitalLivingEffectEvent.Removed` | 移除 effect | 清理被动技能 / 解除状态 |
| `BiocapitalLivingEffectEvent.Updated` | 更新 effect（amplifier / custom_state）| 实时状态变化 |

> **不走 vanilla `MobEffectEvent`** —— vanilla event 是 duration-based，不适合"状态性"。

### 7.4 渲染

- 在 HUD 顶部新增**单独一行**（**不**用 vanilla buff 列表），列出当前所有 effect 的图标 + 名称 + amplifier
- 图标来自 `config/biocapital/status/<effect_id>.png`（D13 决策）
- 状态性 effect（如淫纹）不显示时间（**没有**时间参数）

### 7.5 默认 Effect（D8 决策起步）

| Effect ID | 类型 | 触发 | 数据 | removable |
|---|---|---|---|---|
| `estrus` | 临时 | 被指定 mob 攻击 | `amplifier`, `duration_ticks`（虽然可设长，但**有** duration）| true |
| `rune_marked` | **状态性** | 特定 mob 接触 | `mark_count`, `mark_locations` | **false** |
| `pleasure_overload` | 临时 | 快感值 > 90 | `intensity` | true |

> 仅 3 个示例；后续 task 按需增；服务端可下发自定义 effect 定义。

### 7.6 服务端 gRPC

```
service PlayerStateService {
    rpc ApplyLivingEffect(ApplyLivingEffectRequest) returns (ApplyLivingEffectResponse);
    rpc RemoveLivingEffect(RemoveLivingEffectRequest) returns (RemoveLivingEffectResponse);
    rpc ListLivingEffects(ListLivingEffectsRequest) returns (ListLivingEffectsResponse);
}
```
