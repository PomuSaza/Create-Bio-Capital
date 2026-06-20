---
module: 07-environment
status: canonical
audience: all contributors
last_reviewed: 2026-06-14
depends_on: 00-overview, 02-player-state, 05-byproducts-fluids
---

# 去致死化环境（De-Fatalised Environment）

> 替代原始蓝图第 4 节「去致死化自然环境」段落。
> 现 Java 实现见 `src/main/java/mo/dystopia/biocapital/world/EnvironmentEffects.java`、`SwampMudBlock.java`。

---

## 1. 总体策略

### 1.1 蓝图原始目标

> 原版岩浆与沼泽泥地保持原有的注册名与材质贴图不变，但其实际效果被底层代码替换。

### 1.2 设计决策

- **保留注册名 + 贴图**（满足蓝图原始要求）
- **替换实际效果**（通过 `LivingTickEvent` 拦截 + 自定义流体/方块）
- 岩浆、沼泽泥地、沙子、岩浆块均替换为「催情热浪 / 触手泥潭 / 柔软沙」效果

### 1.3 与第 7 节「敌对生物」的配合

- 玩家在去致死化环境中**不死亡**，但持续积累 pleasure。
- pleasure 满额 → 触发战败状态（详见 06）。

---

## 2. 岩浆（Lava）

### 2.1 效果替换

| 原版效果 | 替换后 |
|---|---|
| 玩家着火 | 玩家不着火 |
| 玩家持续扣血 | 玩家隐性血量不扣 |
| 玩家死亡 | 玩家不死 |
| 物品掉落（死亡） | 无 |

### 2.2 实际行为

- 玩家在岩浆中：每秒 +10.0 pleasure
- 玩家在岩浆上方 1 块：每秒 +1.0 pleasure
- 玩家进入岩浆时：`LivingTickEvent` 触发 → 检查是否在岩浆/岩浆上方 → 修改 pleasure

### 2.3 实现

```java
@SubscribeEvent
public static void onLivingTick(LivingTickEvent event) {
    if (event.getEntity().level().isClientSide()) return;
    if (!(event.getEntity() instanceof Player player)) return;
    PlayerStateAttachment state = PlayerStateAttachment.get(player);
    BlockPos pos = player.blockPosition();
    Level level = player.level();

    // 检查岩浆
    if (level.getBlockState(pos).is(Blocks.LAVA)
        || level.getBlockState(pos.below()).is(Blocks.LAVA)) {
        state.addPleasure(0.5f);  // 10.0 / 秒
    }
}
```

> **关键**：通过 `LivingTickEvent` 而非原版 fire tick，因为原版 fire tick 会触发 damage。

---

## 3. 沼泽泥地（Swamp / Mud）

### 3.1 效果替换

| 原版效果 | 替换后 |
|---|---|
| 移动减速 | 保留减速（设计目的） |
| 持续扣血 | 玩家不死 |
| 玩家死亡 | 玩家不死 |

### 3.2 实际行为

- 玩家在沼泽泥地（自定义方块 `swamp_mud`）中：
  - 每秒 +5.0 pleasure
  - 每 5 秒 -0.5 hunger
  - 移动速度降低 50%

### 3.3 自定义方块

- 注册名：`create_biocapital:swamp_mud`
- 模型 / 贴图：**复用**原版沼泽泥地贴图（避免视觉差异）
- 行为：通过自定义 Block 类实现 `stepOn` / `entityInside`
- **不**覆盖原版 `minecraft:mud`（未来兼容）

### 3.4 世界生成

- 使用 Biome Modifier（NeoForge 1.21）：
  - `data/create_biocapital/worldgen/configured_feature/swamp_mud_patch.json`
  - `data/create_biocapital/worldgen/placed_feature/swamp_mud_patch.json`
  - `data/create_biocapital/neoforge/biome_modifier/swamp_mud.json`
- 仅在 `#minecraft:is_swamp` 标签的生物群系中替换

### 3.5 关键 JSON 注意点

- 1.21 移除了 `minecraft:patch`，使用 `minecraft:random_patch` 或**直接** `minecraft:simple_block`
- `random_patch` 的 inner codec 字段为 `placement` / `tries` / `xz_spread` / `y_spread`，**不是** `feature`
- 错误的 JSON 会在 `CreateWorld` 时崩溃（`IllegalStateException: Unbound values in registry`）

---

## 4. 沙子（Sand）

### 4.1 效果

- 玩家在沙子（任何高度）中：
  - 每 10 秒 +0.5 pleasure（缓慢）
  - 玩家不会陷入（保持原版碰撞箱）

### 4.2 实现

- 不替换方块注册名。
- 在 `LivingTickEvent` 中检查 `state.is(BlockTags.SAND)` → 修改 pleasure。

---

## 5. 岩浆块（Magma Block）

### 5.1 效果

- 玩家在岩浆块上：每秒 +3.0 pleasure
- 不触发原版 damage（与岩浆处理一致）

---

## 6. 战败状态接入（D2/D3/D4 决策覆写）

> **2026-06-20 用户原话**："**隐性战败它跟 buff 就没有关系，它是显示在用户界面上的一层类似于遮罩**"
> "**就是这个 HP 加上隐性血量**"  ← 这条用户是说：**HP = 原版 HP**（隐藏条），**没有**新建池
> "**显示一个求助的一个按钮，要么说放弃现在的这个物品，然后回到一个安全的地点**"

- 当玩家在去致死化环境（岩浆 / 沼泽 / 沙 / 岩浆块）中 **HP 降至 0** 时（D4 决策：**立即触发**，**不**延迟 30s）：
  - **临时 HP=1** 阻止原版死亡路径
  - 触发 `BiocapitalDefeatedStateEvent`（D3 决策：**不是 buff**，是**独立状态**）
  - 客户端显示**整屏绿色 UI 遮罩层** + **视角摇晃**（类似 nausea 实现，但**不是** MobEffectInstance）
  - 弹出**求助按钮**（自定义 widget），**两个选项并发**：
    - **放弃物品回床**：清除玩家身上所有物品 + 传送到 `respawnPos`（床/出生点）
    - **用猫草恢复**：消耗 N 单位猫草（`[Recovery] cat_grass_cost`，**可配置**，默认 50）；HP 回满 + 清全部 debuff + 饱食度回满 + 快感值清零
- **部位开发度不变**（D5："**开发度不要给变啊**"）
- **正向 buff 不动**（D5："**正向的不要管**"）
- 战败次数 `low_hp_hits++`，永不归零
- 玩家选完选项 → `defeated=false` → UI 遮罩层消失

**与原版战败状态的关键区别**：
- ❌ 旧版："隐性战败 = buff"（注册成 `MobEffectInstance`）
- ✅ 现行版："隐性战败 = 独立 UI 遮罩层状态"（**不**走 vanilla `MobEffect`）

详见 `doc/02-player-state.md` §1.4 + §2.4 + §3.4。

---

## 7. 物品掉落保护

### 7.1 原版死亡物品掉落

- 由于玩家不会因环境死亡，原版 death drops **不触发**。
- 玩家可以通过 `/biocapital admin revive <player>` 强制清空 debuff（权限 3）。

### 7.2 战败状态期间物品

- 战败状态期间，物品栏保护（玩家不能打开箱子、不能交易）。
- 5 秒后战败状态结束，恢复正常。

---

## 8. Rust 重写后的形态

### 8.1 服务端权威

- 环境触发的 pleasure 增量在 Rust 侧计算。
- Java 端 `LivingTickEvent` → gRPC `ApplyEnvironmentEffect` → Rust 计算 → 写 PG。

### 8.2 gRPC 接口

```protobuf
service EnvironmentService {
  rpc ApplyEnvironmentEffect(EnvironmentEffectRequest) returns (EnvironmentEffectResponse);
  rpc GetEnvironmentModifiers(BlockPos) returns (EnvironmentModifiers);
}
```

### 8.3 PostgreSQL 表

```sql
CREATE TABLE environment_effects (
  effect_id UUID PRIMARY KEY,
  environment VARCHAR(16) NOT NULL,  -- LAVA / SWAMP_MUD / SAND / MAGMA_BLOCK
  entity_uuid UUID NOT NULL,         -- 受影响实体（玩家或生物）
  intensity FLOAT NOT NULL DEFAULT 1.0,
  duration_ticks BIGINT NOT NULL DEFAULT 0,  -- 0 = permanent（2026-06-14 user decision）
  tick BIGINT NOT NULL,              -- 写入时的 tick
  pleasure_delta FLOAT,
  hunger_delta FLOAT,
  triggered_defeat BOOLEAN NOT NULL DEFAULT FALSE,
  PRIMARY KEY (effect_id)
);
CREATE INDEX idx_env_effects_entity ON environment_effects (entity_uuid, tick DESC);
CREATE INDEX idx_env_effects_env    ON environment_effects (environment, tick DESC);
```

> **2026-06-14 user decision**：新增 `intensity` (FLOAT) 与 `duration_ticks` (BIGINT) 列，对应 proto `EnvironmentEffectRequest` 字段；`duration_ticks = 0` 表示永久效果（如岩浆、沼泽）。

---

## 9. 性能影响

- 主线程 tick：每个玩家每 tick 检查 4 种环境（O(1)）；总计 ≤0.5 ms
- 异步任务：Rust 累积效果，每 10 秒 flush PG
- 内存：每玩家每环境条目 64 字节

---

## 10. 联动点

- `EnvironmentEffectEvent` —— 玩家被环境触发时
- `DefeatStateEnterEvent` / `DefeatStateExitEvent`
- KubeJS：`events.onEnvironmentEffect(event => { event.player, event.environment })`
