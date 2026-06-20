---
module: 06-hostile-mobs
status: canonical
audience: all contributors
last_reviewed: 2026-06-14
depends_on: 00-overview, 01-cross-cutting-concerns, 02-player-state, 13-bio-customization
---

# 敌对生物变体替换系统

> 替代原始蓝图第 4 节「怪物系统行为」段落。
> 现 Java 实现见 `src/main/java/mo/dystopia/biocapital/world/HostileMobHandler.java` 与 `MobDropsHandler.java`。

---

## 1. 总体策略

### 1.1 蓝图原始目标

> 敌对生物转化为具备色情特征的变体实体。

### 1.2 当前实现 vs 蓝图差距

- 现有 `HostileMobHandler` 仅**取消**生成（`event.setCanceled(true)`），不替换。
- 蓝图要求：**完全改写**原版生物（**不**替换注册名），**保留**原版生成位置 + 行为模式。

### 1.3 设计决策（**2026-06-20 D6/D9 覆写**）

> **用户原话**："**到底是替换原版已有的这些生物，原版有多少就改掉多少，完全的改掉**"
> "**一个是自定义模型，自定义声音这一块**"
> "**没有找到的话，它应该能够退回到原本的，原版的这些怪物的这个素材。但是行为模式是要写的**"
> "**还要让用户能够通过编辑json文件，然后去看有json文件，直接用用户编辑这个json文件，去复写掉咱们模组中预设的这一个怪物的这个行为**"

- **完全改写**原版生物，**不**注册新 EntityType（**不**替换异名）：保留原版 `EntityType` 注册名 + id，但**完全改写**行为 + 模型 + 声音
- **改写时机**：`EntityJoinLevelEvent`（**不**是 `MobSpawnEvent`）—— 这样可以保留生成位置
- **改写内容**：
  - 模型：自定义模型（资源包提供）；缺失 → 退回到**原版**模型（**不**留 magenta missing texture）
  - 声音：自定义声音（资源包提供）；缺失 → 退回到**原版**声音
  - 行为：自定义行为（**默认**有 mod 预设 + **用户/服主**可通过 JSON 覆写）
- **回退机制**：模型/声音缺失时**绝不报错**；fallback 到原版 + 仍应用 mod 自定义行为
- **玩家/服主自定义**：通过编辑 `config/biocapital/creatures/<mob_id>/behavior.json` 复写 mod 预设
- **服务器分发**：服务器可下发自定义资源（贴图/模型/声音/行为 JSON）到玩家本地（D9 决策 + D16 hash 检查）

> ❌ **不**注册新 EntityType（避免 "变体实体" 命名空间污染）
> ✅ **完全改写**原版 EntityType（行为 + 资源）

---

## 2. 原版生物完全改写（D6 决策）

### 2.1 改写范围

| 项 | 改写前 | 改写后 |
|---|---|---|
| EntityType 注册名 | `minecraft:zombie` | **保持不变**（`minecraft:zombie`）|
| EntityType 内部类 | `Zombie`（vanilla）| **保留**（继承 vanilla）|
| 模型 | `minecraft:zombie` 资源 | **优先** mod/服主自定义 → fallback 原版 |
| 声音 | `minecraft:zombie` 资源 | **优先** mod/服主自定义 → fallback 原版 |
| 行为（攻击效果）| vanilla attack | **mod 预设** + JSON 覆写 + living effect 触发 |
| 掉落 | vanilla loot | **mod 预设** + 保留 vanilla 物品 |

### 2.2 实现方式

```java
// 1. 在 EntityJoinLevelEvent 监听原版 EntityType
@SubscribeEvent
public static void onEntityJoin(EntityJoinLevelEvent event) {
    Entity entity = event.getEntity();
    if (!(entity instanceof Monster monster)) return;
    if (BiocapitalCreatureConfig.hasCustomConfig(monster.getType())) {
        // 2. 应用行为覆写（攻击、living effect 触发）
        BiocapitalCreatureConfig.applyBehavior(monster);
    }
}

// 3. 模型 / 声音通过 ResourceLocation 优先级加载
public ResourceLocation getCustomOrFallbackModel(EntityType<?> type) {
    String id = BuiltInRegistries.ENTITY_TYPE.getKey(type).getPath();
    ResourceLocation custom = ResourceLocation.fromNamespaceAndPath("create_biocapital", "models/entity/" + id);
    if (resourceExists(custom + ".json")) return custom;
    return EntityType.getKey(type);  // fallback to vanilla
}
```

### 2.3 改写列表（**MVP 必改**）

按"先做 MVP 框架"原则（`doc/19-dev-process.md` §1–§3），MVP 阶段**只改 3 个**原版生物：

- `minecraft:zombie` —— 默认行为："被攻击获得发情 30s"
- `minecraft:skeleton` —— 默认行为："被攻击获得淫纹 1 个（状态性，不解除）"
- `minecraft:spider` —— 默认行为："被攻击获得催情态 60s"

**后续增**（task #N.1+）：creeper / enderman / witch / 其他 vanilla 怪物。

### 2.4 默认行为 JSON（D6 + 后续增）

每个 mob 一个 JSON，**位置**：

| 状态 | 路径 |
|---|---|
| 玩家本地 / 单人 | `<minecraft_dir>/config/biocapital/creatures/<mob_id>/behavior.json` |
| 服务器下发（在线）| `<minecraft_dir>/config/biocapital-online/<server_id>/creatures/<mob_id>/behavior.json` |

**JSON schema**（**MVP 起步**）：

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
  ],
  "defense_effects": [
    {
      "trigger": "on_player_attack_self",
      "apply_living_effect": {
        "effect_id": "estrus",
        "duration_ticks": 600,
        "amplifier": 1
      }
    }
  ],
  "loot_override": {
    "additional_items": [
      { "item": "create_biocapital:desire_fragment", "count_min": 1, "count_max": 3, "chance": 0.5 }
    ]
  }
}
```

> **MVP 阶段**：仅 `attack_effects` + `defense_effects`；`loot_override` 留后续增。
> **优先级**：玩家本地 JSON > 服务器下发 JSON > mod 预设 JSON。

### 2.5 服务器资源分发（D9 决策）

> **D9 决策核心**：服务器可下发自定义资源（贴图/模型/声音/行为 JSON）到玩家本地；玩家进服时拉取，**每 5 分钟 hash 检查**后增量同步。

**同步机制**（D16 决策 B：进服 + 5 min hash 检查）：

```
1. 玩家进服时：
   - MC 客户端从 Rust 拉取服务器端的 resources_index（hash 列表）
   - 对比玩家本地 hash 列表
   - 缺失/变化 → 下载到 config/biocapital-online/<server_id>/
2. 每 5 分钟：
   - Rust 推 resources_index 变化
   - MC 客户端重新对比 + 增量下载
3. 玩家断服 → 玩家本地 config/biocapital/ 仍然有效
```

**资源类型**：
- 行为 JSON（`creatures/<mob_id>/behavior.json`）
- 状态 icon PNG（`status/<effect_id>.png`，D13 决策）
- 自定义模型 / 贴图 / 声音（后续增）

**禁止**：
- ❌ 服务器下发**不**污染玩家本地 `config/biocapital/`（D15 决策）
- ❌ 服务器下发**不**自动覆盖 mod 默认；只在玩家/服主主动选时生效

---

## 3. 替换逻辑（**已废弃**——D6 决策）

---

## 2. 变体实体注册

### 2.1 EntityType

```java
public static final DeferredRegister<EntityType<?>> ENTITY_TYPES =
    DeferredRegister.create(Registries.ENTITY_TYPE, MODID);

// 示例：原版 zombie 替换
public static final DeferredHolder<EntityType<?>, EntityType<VariantZombie>> VARIANT_ZOMBIE =
    ENTITY_TYPES.register("variant_zombie", () ->
        EntityType.Builder.<VariantZombie>of(VariantZombie::new, MobCategory.MONSTER)
            .sized(0.6F, 1.95F)  // 同原版 zombie
            .build("variant_zombie"));
```

### 2.2 变体实体类

```java
public class VariantZombie extends Zombie {
    // 继承原版 zombie 的所有攻击/AI/掉落逻辑
    // 仅覆写：getModel()、getAmbientSound()、getDeathSound()
}
```

### 2.3 替换映射表

| 原版 | 变体 |
|---|---|
| `minecraft:zombie` | `create_biocapital:variant_zombie` |
| `minecraft:skeleton` | `create_biocapital:variant_skeleton` |
| `minecraft:creeper` | `create_biocapital:variant_creeper` |
| `minecraft:spider` | `create_biocapital:variant_spider` |
| `minecraft:enderman` | `create_biocapital:variant_enderman` |
| ... | ... |

> 全替换列表由 `create_biocapital.toml` 的 `[HostileMobReplacement]` 节控制：

```toml
[HostileMobReplacement]
enabled = true
remove_spawn_eggs = true
whitelist = ["minecraft:zombie", "minecraft:skeleton"]
blacklist = ["minecraft:ender_dragon"]  # 不替换末影龙
```

---

## 3. 替换逻辑

### 3.1 触发位置

`@SubscribeEvent` 监听 `EntityJoinLevelEvent`：

```java
@SubscribeEvent
public static void onEntityJoin(EntityJoinLevelEvent event) {
    if (event.getLevel().isClientSide()) return;
    Entity entity = event.getEntity();
    if (!(entity instanceof Monster monster)) return;
    if (!Config.hostileRemovalEnabled) return;
    if (Config.mobBlacklist.contains(EntityType.getKey(monster.getType()).toString())) return;
    if (!Config.mobWhitelist.isEmpty()
        && !Config.mobWhitelist.contains(EntityType.getKey(monster.getType()).toString())) return;

    // 取消原版生成
    event.setCanceled(true);

    // 在原位置生成变体
    VariantEntity variant = VariantRegistry.createVariant(monster.getType(), event.getLevel());
    variant.moveTo(monster.getX(), monster.getY(), monster.getZ(),
                   monster.getYRot(), monster.getXRot());
    event.getLevel().addFreshEntity(variant);
}
```

### 3.2 行为继承

- 变体实体**继承原版的 AI、目标选择、攻击模式**。
- 变体实体的**伤害值、HP、移动速度、掉落物**完全等同原版。
- 变体实体的**模型/动画/音频**来自 13-bio-customization 的 `creatures.json`。

---

## 4. 攻击效果

### 4.1 隐性血量损失

- 变体攻击玩家 → 原版 `hurtServer` 路径触发。
- 在 `LivingHurtEvent` 中拦截：实际伤害转换为「隐性血量」扣除 + pleasure 增量。
- 玩家 `applyHiddenDamage(incoming)` + `addPleasure(incoming × 0.4)`。

### 4.2 pleasure 增量

- 每个变体攻击命中：+2.0 pleasure（基础值，可配置）
- 高速攻击（如 spider）：+3.0
- 远程攻击（如 skeleton 弓箭）：+1.5

### 4.3 战败状态触发

- 当玩家 `pleasure >= 100.0` 时，触发**隐性战败状态**（详见 02-player-state 第 3.1 节）。
- 战败状态期间，玩家被「捕获」在该位置 5 秒（**不**传送），期间持续 + pleasure。
- 战败结束后 `pleasure = 0`，low_hp_hits 不变，但 GENITAL 部位开发度 +5.0（详见 03）。

---

## 5. 掉落物

### 5.1 默认掉落

- 完全等同原版（如 zombie 掉落 rotten_flesh、偶尔掉落装备）。
- **不**删除原版掉落物。

### 5.2 欲望碎片

- 击败任何变体敌对生物：3% 概率掉落 1 个 `desire_fragment`（蓝图原始设定）。
- 概率可在 `create_biocapital.toml` 的 `[HostileMobReplacement.Drops]` 节调整。

---

## 6. 客户端渲染

### 6.1 模型来源

- 每个变体实体对应 `assets/create_biocapital/geo/<creature_id>.geo.json`（Geckolib 模型）
- 动画：`assets/create_biocapital/animations/<creature_id>.animation.json`
- 贴图：`assets/create_biocapital/textures/entity/<creature_id>.png`

### 6.2 占位策略

- 与 13-bio-customization 一致：占位 .txt 文件描述期望资源，实际资源由用户提供。
- 缺失贴图：游戏日志 WARN + magenta-black fallback。

---

## 7. Spawn Egg 处理

### 7.1 原版 spawn egg 移除

- 默认 `Config.removeSpawnEggs = true`：原版敌对生物的 spawn egg 在创造栏中**不显示**。
- 黑名单 / 白名单中的生物 spawn egg 始终不显示。

### 7.2 变体 spawn egg（可选）

- 每个变体敌对生物可注册一个对应 spawn egg：`create_biocapital:variant_zombie_spawn_egg`。
- 默认不注册，由 `create_biocapital.toml` 的 `[HostileMobReplacement.SpawnEggs]` 节控制。

---

## 8. Rust 重写后的形态

### 8.1 服务端权威

- 变体实体的攻击伤害 → 隐性血量 + pleasure 转换公式在 Rust 侧。
- Java 端 `LivingHurtEvent` 拦截后调用 gRPC `ApplyHostileDamage`。

### 8.2 gRPC 接口

```protobuf
service HostileMobService {
  rpc ApplyHostileDamage(DamageRequest) returns (DamageResponse);
  rpc GetDropChance(CreatureIdRequest) returns (DropChanceResponse);
}
```

### 8.3 PostgreSQL 表

```sql
CREATE TABLE mob_replacements (
  original_type VARCHAR(64) PRIMARY KEY,
  variant_type VARCHAR(64) NOT NULL,
  enabled BOOLEAN NOT NULL,
  drop_chance_desire_fragment FLOAT NOT NULL DEFAULT 0.03
);
```

---

## 9. 性能影响

- 主线程 tick：每个敌对生物生成时 0.1 ms（一次性）
- 异步任务：Rust 侧累积战败状态，每 30 秒 flush PG
- 内存：每变体实体约 256 字节

---

## 10. 联动点

- `MobReplacedEvent` —— 原版 mob 被替换时
- `HostileAttackEvent` —— 变体 mob 攻击玩家时
- `DefeatStateEnterEvent` / `DefeatStateExitEvent` —— 战败状态进入/退出
- KubeJS：`events.onHostileAttack(event => { event.player, event.creature })`
