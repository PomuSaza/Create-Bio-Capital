---
module: 19-dev-process
status: canonical — dev workflow discipline
audience: all contributors
last_reviewed: 2026-06-20
depends_on: 00-overview, 01-cross-cutting-concerns, SYSTEM_PROMPT
---

# 开发流程纪律（MVP 优先 + 任务拆分）

> 本文件定义本项目的**开发流程工作纪律**。
> 这是 §0 元规则的具体执行约束，也是 SYSTEM_PROMPT §14 工作循环的细化。

---

## 1. 核心原则

> **"先做出一个可运行的最小版本作为框架，然后陆续的添加需要的功能"** — 2026-06-20 用户决策覆写

**任何**新功能、新模块、新交互的实现任务，**必须**先回答下列问题：

1. **"这个功能的最小可运行版本（MVP）是什么？"**
   - 砍掉所有"锦上添花"的特性
   - 只保留能**端到端跑通**的最少功能
   - 列出被砍掉的特性（**显式不写**进 MVP，等后续增）
2. **"MVP 之外的特性是否需要本任务内做？"**
   - 默认**不需要**——它们是后续 task
   - 如确需，**显式**列出并标注"为何不能拆分"
3. **"MVP 是否经过 §21 强制审计回路？"**
   - 不经审计的 MVP **不得**标 "完成"
   - 审计未通过的 MVP **不得**进下一 task

---

## 2. MVP 拆分示例

### 示例 1：自定义 buff/debuff 系统

| 层 | MVP（本任务）| 后续增 |
|---|---|---|
| 数据 | `Attachment<BiocapitalLivingEffects>` on `Player`，存 `Map<LivingEffectId, LivingEffectData>` | 复杂状态机、组合效果、互斥 |
| 渲染 | HUD 顶部新增一行，列出当前所有 `LivingEffect` 的图标 + 名称 | 动画、特效、粒子 |
| 事件 | `BiocapitalLivingEffectEvent.Added/Removed/Updated` 三个基础事件 | 自定义触发、被动效果链 |
| 服务端 | 1 个 gRPC endpoint `ApplyLivingEffect(player_uuid, effect_id, params)` | 批量应用、组合应用、撤销 |
| 客户端 | `ClientboundLivingEffectUpdatePacket` 接收服务端推送 | 客户端预测、批量更新 |
| 美术 | 1 个占位 `living_effect_unknown.png.txt` + 1 个真实 icon（发情）| 15+ effect icon 全套 |
| 行为 JSON | 1 个示例 `creatures/zombie/behavior.json`（被攻击获得发情 30s）| 全 mob 行为 + 玩家自定义 + 服务器分发 |
| 服务器分发 | **不**实现 | 周期性 hash 检查 + 自动同步 |

**MVP 任务数**：~6 个 gRPC + 数据层 + 1 个 example + 1 个 icon
**显式不做**：服务器分发、组合、动画、批量更新（**留后续 task**，但**显式列出**）

### 示例 2：Core Pod

| 层 | MVP | 后续增 |
|---|---|---|
| 方块 | 1×2×1 多方块，朝向 1 种 | 6 种朝向 + 可视化壳 |
| 应力 | 接受 Create KineticNetwork 输入，输出固定 SU | 应力可调、效率公式 |
| 流体输入 | 岩浆（minecraft:lava）桶装输入 | 流体管道 + 多流体 + 配比 |
| 流体输出 | 副产物以流体形式抽出（接 Mechanical Drain）| 多副产物、库存管理 |
| 物品输出 | 欲望碎屑以物品形式输出（接 Chute/Depot）| 智能仓库、批次号 |
| 应力公式 | 1 个固定 SU 常数（与 Create 6.0.10 电机对齐） | 动态公式（基于 part_dev 等）|
| 模型 | 1 个占位 + 1 个简化 model | 完整 3D 模型 + 动画 |
| 离线托管 | **不**实现 | 4 任务做 |

**MVP 任务数**：~4 个（方块 + 应力 + 流体 + 物品）
**显式不做**：6 朝向、应力可调、多副产物、模型、托管（**留后续 task**）

---

## 3. 任务追踪规范

### 3.1 TaskCreate 拆分

每个"功能"级任务**必须**拆分为：

```
[Task #N.0] 功能 X — MVP
[Task #N.1] 功能 X — 后续增 A
[Task #N.2] 功能 X — 后续增 B
...
```

`.0` 是**永远第一个**，且**必须**先完成；`.1+` 是**后续** task，可按需排期。

### 3.2 Task 描述模板

```
任务：<功能名> - MVP 子集

MVP 范围（必做）：
- <bullet 1>
- <bullet 2>
- ...

显式不做（留后续 task）：
- <bullet 1>
- <bullet 2>
- ...

审计回路：
- Step 1 EXECUTE: 实现 subagent
- Step 2 AUDIT: 独立 audit subagent
- Step 3a IMPROVE: 若必须改 ≠ 空，打回
- Step 3b COMMIT: 推送当前分支，不合并 main
```

### 3.3 反模式

- ❌ "这个功能先做了再说，MVP 太慢了" → 错；MVP 是**强制**的
- ❌ "MVP + 后续增" 一起做，无拆分 → 错；必须 TaskCreate 显式拆分
- ❌ "MVP 跑通就标完成" → 错；MVP 必须**经 §21 审计**才能标完成
- ❌ "显式不做" 不显式列出 → 错；必须在 task 描述里**显式**列出

---

## 4. 与 §21 审计回路的集成

| Phase | 任务 | 输出 |
|---|---|---|
| Step 1 EXECUTE | 实现 subagent 完成任务（MVP 子集）| 代码 + 自检输出 |
| Step 2 AUDIT | 独立 audit subagent 跑 `git diff` + 4 维度检查 | 审计报告（必须改 / 建议改 / 可不改）|
| Step 3a IMPROVE | 若必须改 ≠ 空 | 实现 subagent 重写 + 重审 |
| Step 3b COMMIT | 自主 commit 到当前分支 | commit hash + 推送 |

**MVP 完成度评估**（`must-fix = 0` 后）：

| 维度 | 必须满足 |
|---|---|
| 端到端跑通 | 是（cargo test / gradlew / e2e 至少 1 个）|
| 显式不做段已列 | 是（task 描述里**显式**列出）|
| 文档同步 | 是（doc/ 中相关段 + CHANGELOG）|
| 审计通过 | 是（独立 audit subagent 报告必须改 = 0）|
| Commit 到位 | 是（commit 在当前分支，**不**在 main）|

**任一不满足 → 不得标 MVP 完成，不得进下一 task**。

---

## 5. §20 实时文档同步

MVP 完成时**必须**同步更新：

1. `doc/<相关模块>.md` —— 该功能在文档中的描述与实际实现对齐
2. `doc/CHANGELOG.md` —— 追加 MVP 完成条目（含审计自审结论）
3. `doc/99-integration-matrix.md` —— 新增/修改联动条目
4. `README.md` —— 如完成度变化，更新「当前状态」段
5. `memory/MEMORY.md` —— 如有跨会话经验，写一条 memory

**MVP 是文档的最小验收点**——MVP 跑通 = 文档必须能"如实反映"。

---

## 6. §22 诚实交付的 MVP 视角

**MVP 完成 ≠ 完整完成**。文档中描述该功能时**必须**分清：

- ✅ **本 MVP 已做**（明确列出）
- ⚠️ **部分可用**（MVP 跑通但有已知缺口——必须列）
- ❌ **未做**（后续 task）
- 🚫 **阻塞**（等待外部依赖或用户决策）

**反例**（§22 反模式）：
- ❌ 标 "buff/debuff 系统完成" → 实际只做了 1 个 icon + 1 个 example
- ❌ 标 "Core Pod 完成" → 实际只做了方块 + 应力，**没**做流体输出
- ❌ 标 "Web UI 完成" → 实际只做了 health 端点，其他路由 500

**正确**：
- ✅ "buff/debuff 系统 MVP：1 个发情 effect + 1 个 icon + 1 个 example behavior JSON；后续增 5 个 task"
- ✅ "Core Pod MVP：1×2×1 + 应力输入 + 岩浆输入 + 1 个副产物；后续增 7 个 task"
- ✅ "Web UI MVP：health + 1 个玩家查询；后续增 17 个路由"

---

## 7. 与现有 18 模块的关系

| 模块 | 当前 MVP 状态 | 后续 task |
|---|---|---|
| 02 PlayerState | ⚠️ 部分可用（HP 隐藏 + 战败 UI 待实现）| 完整 HP 模型 + 战败触发 + 猫草恢复 |
| 04 CorePod | ⚠️ 部分可用（方块 + 应力待实现）| 完整多方块 + 流体 + 副产物 + 抽液机 |
| 06 HostileMobs | ⚠️ 部分可用（替换 + 行为待实现）| 完全改写 + 默认行为 + JSON 覆写 + 服务器分发 |
| 08 Bank | ⚠️ 部分可用 | 完整批次号追踪 + 设备锁定 + Web UI |
| 09 Contracts | ⚠️ 部分可用 | 完整合约生命周期 |
| 10 DG_LAB | ⚠️ 部分可用 | MC↔手机独立连接 + 强度算法 + 波形 |
| 14 RustServices | ⚠️ 部分可用 | 完整 gRPC + WS + 启动期 SQL 加载 |
| 15 WebUI | ⚠️ 部分可用（仅 health）| 18 个路由 |
| ... | ... | ... |

**项目总体完成度**（2026-06-20 估计）：**~70%**（按 00-overview.md §2.3）。

**MVP 优先策略**：
- 每个模块**先**实现 1 个 MVP 子集（端到端跑通 + 审计通过）
- MVP 完成度 = "模块在功能上最小可用"
- 后续 task 累加到模块的"完整完成度"

---

## 8. 引用

- `doc/SYSTEM_PROMPT.md` §14 工作循环（执行 → 审计 → 改进）
- `doc/SYSTEM_PROMPT.md` §20 实时文档与记忆纪律
- `doc/SYSTEM_PROMPT.md` §21 代码审计回路
- `doc/SYSTEM_PROMPT.md` §22 诚实交付
- `doc/SYSTEM_PROMPT.md` §23 自主 commit 与分支管理
- `doc/00-overview.md` §2.3 诚实完成度
- `doc/CHANGELOG.md` 完整变更历史
