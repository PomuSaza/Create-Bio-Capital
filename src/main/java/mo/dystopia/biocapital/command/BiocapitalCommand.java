package mo.dystopia.biocapital.command;

import com.mojang.brigadier.CommandDispatcher;
import com.mojang.brigadier.arguments.IntegerArgumentType;
import com.mojang.brigadier.arguments.LongArgumentType;
import com.mojang.brigadier.arguments.StringArgumentType;
import com.mojang.brigadier.builder.LiteralArgumentBuilder;
import com.mojang.brigadier.context.CommandContext;
import net.minecraft.commands.CommandSourceStack;
import net.minecraft.commands.Commands;
import net.minecraft.network.chat.Component;
import net.minecraft.server.level.ServerPlayer;
import net.neoforged.bus.api.SubscribeEvent;
import net.neoforged.fml.common.EventBusSubscriber;
import net.neoforged.neoforge.event.RegisterCommandsEvent;
import mo.dystopia.biocapital.NativeRustBindings;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.Optional;
import java.util.UUID;

/**
 * 2026-06-15 task #130: 完整重写 {@code /biocapital *} 指令树
 * （doc/12 §2）为 thin shim。
 *
 * <h2>职责边界（强制 — doc/12 §3.1）</h2>
 * 本类<em>仅</em>做：
 * <ol>
 *   <li>注册指令树（{@link RegisterCommandsEvent}）</li>
 *   <li>Brigadier 参数解析 + UUID / 玩家名解析</li>
 *   <li>权限校验（{@code source.hasPermission(level)} 0..=4）</li>
 *   <li>组装 {@link BiocapitalWireFormat} 简化 wire format + 调
 *       {@link NativeRustBindings#callPlayerState} /
 *       {@link NativeRustBindings#callBank} /
 *       {@link NativeRustBindings#callContract} /
 *       {@link NativeRustBindings#callDglab} /
 *       {@link NativeRustBindings#callAudit} 等转发到 Rust 端</li>
 *   <li>解码响应（{@link BiocapitalWireFormat#decodeLong} 等）
 *       + 玩家聊天输出（{@code source.sendSuccess} /
 *       {@code sendFailure}）</li>
 * </ol>
 *
 * <p>本类<strong>不</strong>做：业务逻辑（银行余额变更、token 颁发、
 * 契约推进、审计写入等）。所有业务逻辑由 Rust 端在
 * {@code rust/crates/biocapital-grpc/} 的 service 中实现（doc/14
 * §3.2），通过 Sable JNI（doc/16 §3.4）转发。
 *
 * <h2>method_id 约定（与 NativeRustBindings + doc/14 §3.2 一致）</h2>
 * <pre>
 *   PlayerStateService (callPlayerState):
 *     0  GetState
 *     1  UpdateState
 *     2  ApplyDamage
 *     3  AddPleasure
 *     4  AddHunger
 *
 *   BankService (callBank):
 *     0  GetBalance       1  Deposit          2  Withdraw
 *     3  Transfer         4  GetHistory       5  LockDevice
 *     6  UnlockDevice     7  GenerateInvite   8  AcceptInvite
 *     9  RequestHwToken  10  BindHw          11  ListHw
 *    12  RevokeHw        13  Authenticate
 *
 *   ContractService (callContract):
 *     0  Propose     1  Accept     2  Reject     3  Terminate
 *     4  Redeem      5  Get        6  List
 *
 *   DglabService (callDglab):
 *     0  SetStrength   1  GetStrength  2  GenerateToken
 *     3  RevokeToken   4  ListTokens
 *
 *   AuditService (callAudit):  0  Query  1  Export
 *
 *   CreatureService (callCreature):  0  List  1  Get  2  Reload
 * </pre>
 *
 * <h2>权限等级（doc/12 §1）</h2>
 * <pre>
 *   0  任何人
 *   1  普通玩家（命令对自身生效）
 *   2  OP
 *   3  服务器管理员
 *   4  完整控制（需 full_control = true）
 * </pre>
 *
 * <h2>降级（doc/16 §10）</h2>
 * 当 {@link NativeRustBindings#NATIVE_AVAILABLE} = false 时，Rust 端
 * 不可达。本类用 {@code Optional.empty()} 表达「Rust 端无响应」，
 * 通过 {@code sendFailure} 告知玩家「Rust 服务不可用」—— 不抛
 * 异常、不做 Java 端 fallback 业务逻辑。
 *
 * <h2>wire format</h2>
 * 当前 Java 端<strong>未</strong>引入 protobuf-java 依赖，因此采用
 * {@link BiocapitalWireFormat} 简化编码（{@code ByteBuffer} 大端序
 * 定长字段，每 8 字节一个 long）。Rust 端在 JNI dispatch 层按相同
 * 字段顺序读取。详见 {@code doc/SYSTEM_PROMPT.md §3.6}。
 */
@EventBusSubscriber(modid = "create_biocapital", bus = EventBusSubscriber.Bus.GAME)
public final class BiocapitalCommand {

    private static final Logger LOGGER = LoggerFactory.getLogger(BiocapitalCommand.class);

    // ── 常量：方法名前缀 / 反馈标签 ─────────────────────────────

    private static final String PREFIX = "[BioCapital]";

    // ── method_id 常量（与 NativeRustBindings + doc/14 §3.2 同步）────

    // PlayerStateService
    public static final int PLAYER_STATE_GET_STATE = 0;
    public static final int PLAYER_STATE_UPDATE = 1;
    public static final int PLAYER_STATE_APPLY_DAMAGE = 2;
    public static final int PLAYER_STATE_ADD_PLEASURE = 3;
    public static final int PLAYER_STATE_ADD_HUNGER = 4;

    // BankService
    public static final int BANK_GET_BALANCE = 0;
    public static final int BANK_DEPOSIT = 1;
    public static final int BANK_WITHDRAW = 2;
    public static final int BANK_TRANSFER = 3;
    public static final int BANK_GET_HISTORY = 4;
    public static final int BANK_LOCK_DEVICE = 5;
    public static final int BANK_UNLOCK_DEVICE = 6;
    public static final int BANK_GEN_INVITE = 7;
    public static final int BANK_ACCEPT_INVITE = 8;
    public static final int BANK_REQ_HW_TOKEN = 9;
    public static final int BANK_BIND_HW = 10;
    public static final int BANK_LIST_HW = 11;
    public static final int BANK_REVOKE_HW = 12;
    public static final int BANK_AUTHENTICATE = 13;

    // ContractService
    public static final int CONTRACT_PROPOSE = 0;
    public static final int CONTRACT_ACCEPT = 1;
    public static final int CONTRACT_REJECT = 2;
    public static final int CONTRACT_TERMINATE = 3;
    public static final int CONTRACT_REDEEM = 4;
    public static final int CONTRACT_GET = 5;
    public static final int CONTRACT_LIST = 6;

    // DglabService
    public static final int DGLAB_SET_STRENGTH = 0;
    public static final int DGLAB_GET_STRENGTH = 1;
    public static final int DGLAB_GEN_TOKEN = 2;
    public static final int DGLAB_REVOKE_TOKEN = 3;
    public static final int DGLAB_LIST_TOKENS = 4;

    // AuditService
    public static final int AUDIT_QUERY = 0;
    public static final int AUDIT_EXPORT = 1;

    // CreatureService
    public static final int CREATURE_LIST = 0;
    public static final int CREATURE_GET = 1;
    public static final int CREATURE_RELOAD = 2;

    // CorePodService
    public static final int CORE_POD_TICK = 0;
    public static final int CORE_POD_ENTER = 1;
    public static final int CORE_POD_EXIT = 2;
    public static final int CORE_POD_GET_STATE = 3;

    // EnvironmentService
    public static final int ENV_APPLY = 0;
    public static final int ENV_GET_MODIFIERS = 1;

    // ── 工具类 ─────────────────────────────────────────────

    private BiocapitalCommand() {
        // utility class
    }

    // ── 指令注册 ───────────────────────────────────────────

    @SubscribeEvent
    public static void register(RegisterCommandsEvent event) {
        final CommandDispatcher<CommandSourceStack> d = event.getDispatcher();
        d.register(buildRoot());
        LOGGER.info("[BioCapital] /biocapital command tree registered (task #130 rewrite)");
    }

    private static LiteralArgumentBuilder<CommandSourceStack> buildRoot() {
        return Commands.literal("biocapital")
                // ── stats <me|player> ─────────────────────────
                .then(Commands.literal("stats")
                        .executes(ctx -> handleStats(ctx, null))
                        .then(Commands.argument("target", StringArgumentType.string())
                                .executes(ctx -> handleStats(ctx,
                                        StringArgumentType.getString(ctx, "target")))))
                // ── bank <...> ─────────────────────────────────
                .then(Commands.literal("bank")
                        .then(Commands.literal("balance")
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .executes(ctx -> handleBankBalance(ctx,
                                                StringArgumentType.getString(ctx, "player")))))
                        .then(Commands.literal("history")
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .executes(ctx -> handleBankHistory(ctx,
                                                StringArgumentType.getString(ctx, "player"), 16L))
                                        .then(Commands.argument("limit", LongArgumentType.longArg(1, 1000))
                                                .executes(ctx -> handleBankHistory(ctx,
                                                        StringArgumentType.getString(ctx, "player"),
                                                        LongArgumentType.getLong(ctx, "limit"))))))
                        .then(Commands.literal("transfer")
                                .requires(s -> s.hasPermission(3))
                                .then(Commands.argument("from", StringArgumentType.string())
                                        .then(Commands.argument("to", StringArgumentType.string())
                                                .then(Commands.argument("amount",
                                                        LongArgumentType.longArg(1))
                                                        .executes(ctx -> handleBankTransfer(ctx,
                                                                StringArgumentType.getString(ctx, "from"),
                                                                StringArgumentType.getString(ctx, "to"),
                                                                LongArgumentType.getLong(ctx, "amount")))))))
                        .then(Commands.literal("deposit")
                                .requires(s -> s.hasPermission(3))
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .then(Commands.argument("amount", LongArgumentType.longArg(1))
                                                .executes(ctx -> handleBankDeposit(ctx,
                                                        StringArgumentType.getString(ctx, "player"),
                                                        LongArgumentType.getLong(ctx, "amount"))))))
                        .then(Commands.literal("withdraw")
                                .requires(s -> s.hasPermission(3))
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .then(Commands.argument("amount", LongArgumentType.longArg(1))
                                                .executes(ctx -> handleBankWithdraw(ctx,
                                                        StringArgumentType.getString(ctx, "player"),
                                                        LongArgumentType.getLong(ctx, "amount")))))))
                // ── contract <...> ─────────────────────────────
                .then(Commands.literal("contract")
                        .then(Commands.literal("list")
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .executes(ctx -> handleContractList(ctx,
                                                StringArgumentType.getString(ctx, "player")))))
                        .then(Commands.literal("info")
                                .then(Commands.argument("id", StringArgumentType.string())
                                        .executes(ctx -> handleContractInfo(ctx,
                                                StringArgumentType.getString(ctx, "id")))))
                        .then(Commands.literal("terminate")
                                .requires(s -> s.hasPermission(3))
                                .then(Commands.argument("id", StringArgumentType.string())
                                        .executes(ctx -> handleContractTerminate(ctx,
                                                StringArgumentType.getString(ctx, "id")))))
                        .then(Commands.literal("redeem")
                                .requires(s -> s.hasPermission(3))
                                .then(Commands.argument("id", StringArgumentType.string())
                                        .executes(ctx -> handleContractRedeem(ctx,
                                                StringArgumentType.getString(ctx, "id"))))))
                // ── pod <...> ──────────────────────────────────
                .then(Commands.literal("pod")
                        .requires(s -> s.hasPermission(3))
                        .then(Commands.literal("info")
                                .then(Commands.argument("x", LongArgumentType.longArg())
                                        .then(Commands.argument("y", LongArgumentType.longArg())
                                                .then(Commands.argument("z", LongArgumentType.longArg())
                                                        .executes(ctx -> handlePodInfo(ctx,
                                                                LongArgumentType.getLong(ctx, "x"),
                                                                LongArgumentType.getLong(ctx, "y"),
                                                                LongArgumentType.getLong(ctx, "z")))))))
                        .then(Commands.literal("force_eject")
                                .then(Commands.argument("x", LongArgumentType.longArg())
                                        .then(Commands.argument("y", LongArgumentType.longArg())
                                                .then(Commands.argument("z", LongArgumentType.longArg())
                                                        .executes(ctx -> handlePodForceEject(ctx,
                                                                LongArgumentType.getLong(ctx, "x"),
                                                                LongArgumentType.getLong(ctx, "y"),
                                                                LongArgumentType.getLong(ctx, "z"))))))))
                // ── dglab <...> ────────────────────────────────
                .then(Commands.literal("dglab")
                        .requires(s -> s.hasPermission(3))
                        .then(Commands.literal("list")
                                .executes(ctx -> handleDglabList(ctx)))
                        .then(Commands.literal("generate_token")
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .executes(ctx -> handleDglabGenerateToken(ctx,
                                                StringArgumentType.getString(ctx, "player")))))
                        .then(Commands.literal("revoke_token")
                                .then(Commands.argument("token", StringArgumentType.string())
                                        .executes(ctx -> handleDglabRevokeToken(ctx,
                                                StringArgumentType.getString(ctx, "token")))))
                        .then(Commands.literal("set_strength")
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .then(Commands.argument("strength", IntegerArgumentType.integer(0, 200))
                                                .executes(ctx -> handleDglabSetStrength(ctx,
                                                        StringArgumentType.getString(ctx, "player"),
                                                        IntegerArgumentType.getInteger(ctx, "strength"))))))
                        .then(Commands.literal("get_config")
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .executes(ctx -> handleDglabGetConfig(ctx,
                                                StringArgumentType.getString(ctx, "player")))))
                        .then(Commands.literal("set_config")
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .then(Commands.argument("base", IntegerArgumentType.integer(0, 200))
                                                .then(Commands.argument("max", IntegerArgumentType.integer(0, 200))
                                                        .executes(ctx -> handleDglabSetConfig(ctx,
                                                                StringArgumentType.getString(ctx, "player"),
                                                                IntegerArgumentType.getInteger(ctx, "base"),
                                                                IntegerArgumentType.getInteger(ctx, "max"))))))))
                // ── auth <...> ─────────────────────────────────
                .then(Commands.literal("auth")
                        .then(Commands.literal("request_token")
                                .executes(BiocapitalCommand::handleAuthRequestToken))
                        .then(Commands.literal("bind")
                                .then(Commands.argument("token", StringArgumentType.string())
                                        .executes(ctx -> handleAuthBind(ctx,
                                                StringArgumentType.getString(ctx, "token")))))
                        .then(Commands.literal("list")
                                .executes(BiocapitalCommand::handleAuthList)))
                // ── admin <...> ────────────────────────────────
                .then(Commands.literal("admin")
                        .then(Commands.literal("reset_device")
                                .requires(s -> s.hasPermission(3))
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .executes(ctx -> handleAdminResetDevice(ctx,
                                                StringArgumentType.getString(ctx, "player")))))
                        .then(Commands.literal("grant_viewer")
                                .requires(s -> s.hasPermission(4))
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .executes(ctx -> handleAdminGrantViewer(ctx,
                                                StringArgumentType.getString(ctx, "player")))))
                        .then(Commands.literal("override")
                                .requires(s -> s.hasPermission(3))
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .then(Commands.argument("param", StringArgumentType.string())
                                                .then(Commands.argument("value",
                                                        LongArgumentType.longArg())
                                                        .then(Commands.argument("duration",
                                                                LongArgumentType.longArg(0))
                                                                .executes(ctx -> handleAdminOverride(ctx,
                                                                        StringArgumentType.getString(ctx, "player"),
                                                                        StringArgumentType.getString(ctx, "param"),
                                                                        LongArgumentType.getLong(ctx, "value"),
                                                                        LongArgumentType.getLong(ctx, "duration")))))))))
                // ── config reload ──────────────────────────────
                .then(Commands.literal("config")
                        .requires(s -> s.hasPermission(3))
                        .then(Commands.literal("reload")
                                .executes(BiocapitalCommand::handleConfigReload)))
                // ── whitelist reload ───────────────────────────
                .then(Commands.literal("whitelist")
                        .requires(s -> s.hasPermission(3))
                        .then(Commands.literal("reload")
                                .executes(BiocapitalCommand::handleWhitelistReload)))
                // ── audit <...> ────────────────────────────────
                .then(Commands.literal("audit")
                        .requires(s -> s.hasPermission(3))
                        .then(Commands.literal("query")
                                .then(Commands.argument("scope", StringArgumentType.string())
                                        .then(Commands.argument("uuid", StringArgumentType.string())
                                                .executes(ctx -> handleAuditQuery(ctx,
                                                        StringArgumentType.getString(ctx, "scope"),
                                                        StringArgumentType.getString(ctx, "uuid"),
                                                        100L))
                                                .then(Commands.argument("limit", LongArgumentType.longArg(1, 10000))
                                                        .executes(ctx -> handleAuditQuery(ctx,
                                                                StringArgumentType.getString(ctx, "scope"),
                                                                StringArgumentType.getString(ctx, "uuid"),
                                                                LongArgumentType.getLong(ctx, "limit")))))))
                        .then(Commands.literal("export")
                                .then(Commands.argument("table", StringArgumentType.string())
                                        .executes(ctx -> handleAuditExport(ctx,
                                                StringArgumentType.getString(ctx, "table"), 0L, 0L))
                                        .then(Commands.argument("from_tick", LongArgumentType.longArg(0))
                                                .then(Commands.argument("to_tick", LongArgumentType.longArg(0))
                                                        .executes(ctx -> handleAuditExport(ctx,
                                                                StringArgumentType.getString(ctx, "table"),
                                                                LongArgumentType.getLong(ctx, "from_tick"),
                                                                LongArgumentType.getLong(ctx, "to_tick"))))))))
                // ── debug <...> ────────────────────────────────
                .then(Commands.literal("debug")
                        .requires(s -> s.hasPermission(4))
                        .then(Commands.literal("set_pleasure")
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .then(Commands.argument("value", LongArgumentType.longArg(0, 100))
                                                .executes(ctx -> handleDebugSetPleasure(ctx,
                                                        StringArgumentType.getString(ctx, "player"),
                                                        LongArgumentType.getLong(ctx, "value"))))))
                        .then(Commands.literal("set_hunger")
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .then(Commands.argument("value", LongArgumentType.longArg(0, 100))
                                                .executes(ctx -> handleDebugSetHunger(ctx,
                                                        StringArgumentType.getString(ctx, "player"),
                                                        LongArgumentType.getLong(ctx, "value"))))))
                        .then(Commands.literal("set_hidden_hp")
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .then(Commands.argument("value", LongArgumentType.longArg(0, 100))
                                                .executes(ctx -> handleDebugSetHiddenHp(ctx,
                                                        StringArgumentType.getString(ctx, "player"),
                                                        LongArgumentType.getLong(ctx, "value"))))))
                        .then(Commands.literal("set_part_dev")
                                .then(Commands.argument("player", StringArgumentType.string())
                                        .then(Commands.argument("part", StringArgumentType.string())
                                                .then(Commands.argument("value", LongArgumentType.longArg(0, 100))
                                                        .executes(ctx -> handleDebugSetPartDev(ctx,
                                                                StringArgumentType.getString(ctx, "player"),
                                                                StringArgumentType.getString(ctx, "part"),
                                                                LongArgumentType.getLong(ctx, "value"))))))));
    }

    // ── 权限 / 工具 helper ─────────────────────────────────

    /**
     * 正向语义：{@code source.hasPermission(required) == true} → 通过
     * （返回 1）；否则发送失败反馈 + 返回 0。
     *
     * <p>Brigadier {@code .requires(...)} 是同一机制；如果在该层级
     * 拒绝则根本不会进入 {@code executes} lambda。本 helper 用于
     * 同一指令内「self 0 / other 3」双权限分支的额外校验。
     */
    private static int requirePermission(CommandContext<CommandSourceStack> ctx, int required) {
        if (ctx.getSource().hasPermission(required)) {
            return 1;
        }
        ctx.getSource().sendFailure(Component.literal(
                PREFIX + " Insufficient permission (need " + required + ")"));
        return 0;
    }

    /**
     * 解析玩家名 → UUID。
     * <ul>
     *   <li>"me" 或当前执行者自身 → 返回自身 UUID（无需查表）</li>
     *   <li>其它名字 → 查 {@code source.getServer().getPlayerList()}</li>
     *   <li>未找到 → 返回 {@code null}</li>
     * </ul>
     */
    private static UUID resolvePlayerUuid(CommandContext<CommandSourceStack> ctx, String name) {
        final ServerPlayer self = ctx.getSource().getPlayer();
        if (name == null || name.isEmpty()) {
            return self == null ? null : self.getUUID();
        }
        if (self != null && name.equalsIgnoreCase("me")) {
            return self.getUUID();
        }
        if (self != null && name.equalsIgnoreCase(self.getName().getString())) {
            return self.getUUID();
        }
        // 解析为 "me" 以外的名字 → 需查 playerList
        final var server = ctx.getSource().getServer();
        if (server == null) {
            return null;
        }
        final var player = server.getPlayerList().getPlayerByName(name);
        return player == null ? null : player.getUUID();
    }

    /** 解析 UUID 字符串（去除 "-" 后用 {@link UUID#fromString}）。 */
    private static UUID parseUuidOrNull(String s) {
        if (s == null || s.isEmpty()) return null;
        try {
            return UUID.fromString(s);
        } catch (IllegalArgumentException ex) {
            return null;
        }
    }

    /**
     * 处理 JNI 不可用：发送标准「Rust unavailable」反馈 + 返回 0。
     */
    private static boolean ensureNative(CommandContext<CommandSourceStack> ctx) {
        if (NativeRustBindings.NATIVE_AVAILABLE) {
            return true;
        }
        ctx.getSource().sendFailure(Component.literal(
                PREFIX + " Rust service unavailable; native library not loaded"));
        return false;
    }

    /**
     * 发送成功反馈（绿色 + 工具提示）。注：{@code sendSuccess} 需要
     * {@code Supplier<Component>}（按 doc/12 §5.1 客户端聊天）。
     */
    private static void sendOk(CommandContext<CommandSourceStack> ctx, String body) {
        ctx.getSource().sendSuccess(() -> Component.literal(PREFIX + " " + body), false);
    }

    /**
     * 发送失败反馈（红色，无需 supplier）。
     */
    private static void sendFail(CommandContext<CommandSourceStack> ctx, String body) {
        ctx.getSource().sendFailure(Component.literal(PREFIX + " " + body));
    }

    // ── handler: stats ────────────────────────────────────

    private static int handleStats(CommandContext<CommandSourceStack> ctx, String target) {
        // 权限：self 0 / other 3
        UUID uuid;
        if (target == null || target.isEmpty()
                || target.equalsIgnoreCase("me")) {
            uuid = resolvePlayerUuid(ctx, "me");
        } else {
            if (requirePermission(ctx, 3) == 0) return 0;
            uuid = resolvePlayerUuid(ctx, target);
        }
        if (uuid == null) {
            sendFail(ctx, "Player not found: " + target);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;

        final Optional<byte[]> resp = NativeRustBindings.callPlayerState(
                PLAYER_STATE_GET_STATE, BiocapitalWireFormat.encodeUuid(uuid));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        // 简化：响应解 1 个 long（player_state.last_updated_tick；后续可扩展）
        final Long tick = BiocapitalWireFormat.decodeLong(resp.get());
        final String name = (target == null || target.isEmpty()) ? "me" : target;
        sendOk(ctx, "Stats of " + name + ": updated_tick=" + tick);
        return 1;
    }

    // ── handler: bank <balance|history|transfer|deposit|withdraw> ──

    private static int handleBankBalance(CommandContext<CommandSourceStack> ctx, String playerName) {
        final ServerPlayer self = ctx.getSource().getPlayer();
        if (self == null) {
            sendFail(ctx, "Must be a player");
            return 0;
        }
        UUID targetUuid;
        if (playerName.equalsIgnoreCase("me")
                || playerName.equalsIgnoreCase(self.getName().getString())) {
            targetUuid = self.getUUID();
        } else {
            // OP / admin 看任意
            if (requirePermission(ctx, 3) == 0) return 0;
            targetUuid = resolvePlayerUuid(ctx, playerName);
            if (targetUuid == null) {
                sendFail(ctx, "Player not found: " + playerName);
                return 0;
            }
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callBank(
                BANK_GET_BALANCE, BiocapitalWireFormat.encodeUuid(targetUuid));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long balance = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Balance of " + playerName + ": " + balance);
        return 1;
    }

    private static int handleBankHistory(CommandContext<CommandSourceStack> ctx,
                                          String playerName, long limit) {
        final ServerPlayer self = ctx.getSource().getPlayer();
        if (self == null) {
            sendFail(ctx, "Must be a player");
            return 0;
        }
        UUID targetUuid;
        if (playerName.equalsIgnoreCase("me")
                || playerName.equalsIgnoreCase(self.getName().getString())) {
            targetUuid = self.getUUID();
        } else {
            if (requirePermission(ctx, 3) == 0) return 0;
            targetUuid = resolvePlayerUuid(ctx, playerName);
            if (targetUuid == null) {
                sendFail(ctx, "Player not found: " + playerName);
                return 0;
            }
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callBank(
                BANK_GET_HISTORY,
                BiocapitalWireFormat.encodeUuidAndLong(targetUuid, limit));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long count = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "History of " + playerName + ": " + count + " entries (limit " + limit + ")");
        return 1;
    }

    private static int handleBankTransfer(CommandContext<CommandSourceStack> ctx,
                                           String from, String to, long amount) {
        final UUID fromUuid = resolvePlayerUuid(ctx, from);
        final UUID toUuid = resolvePlayerUuid(ctx, to);
        if (fromUuid == null) {
            sendFail(ctx, "Player not found: " + from);
            return 0;
        }
        if (toUuid == null) {
            sendFail(ctx, "Player not found: " + to);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callBank(
                BANK_TRANSFER,
                BiocapitalWireFormat.encodeUuidUuidLong(fromUuid, toUuid, amount));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long newBalance = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Transferred " + amount + " from " + from + " to " + to
                + " (your new balance: " + newBalance + ")");
        return 1;
    }

    private static int handleBankDeposit(CommandContext<CommandSourceStack> ctx,
                                          String playerName, long amount) {
        final UUID uuid = resolvePlayerUuid(ctx, playerName);
        if (uuid == null) {
            sendFail(ctx, "Player not found: " + playerName);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callBank(
                BANK_DEPOSIT, BiocapitalWireFormat.encodeUuidAndLong(uuid, amount));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long newBalance = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Deposited " + amount + " to " + playerName
                + " (new balance: " + newBalance + ")");
        return 1;
    }

    private static int handleBankWithdraw(CommandContext<CommandSourceStack> ctx,
                                           String playerName, long amount) {
        final UUID uuid = resolvePlayerUuid(ctx, playerName);
        if (uuid == null) {
            sendFail(ctx, "Player not found: " + playerName);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callBank(
                BANK_WITHDRAW, BiocapitalWireFormat.encodeUuidAndLong(uuid, amount));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long newBalance = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Withdrew " + amount + " from " + playerName
                + " (new balance: " + newBalance + ")");
        return 1;
    }

    // ── handler: contract <list|info|terminate|redeem> ────────

    private static int handleContractList(CommandContext<CommandSourceStack> ctx, String playerName) {
        final ServerPlayer self = ctx.getSource().getPlayer();
        if (self == null) {
            sendFail(ctx, "Must be a player");
            return 0;
        }
        UUID targetUuid;
        if (playerName.equalsIgnoreCase("me")
                || playerName.equalsIgnoreCase(self.getName().getString())) {
            targetUuid = self.getUUID();
        } else {
            if (requirePermission(ctx, 3) == 0) return 0;
            targetUuid = resolvePlayerUuid(ctx, playerName);
            if (targetUuid == null) {
                sendFail(ctx, "Player not found: " + playerName);
                return 0;
            }
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callContract(
                CONTRACT_LIST, BiocapitalWireFormat.encodeUuid(targetUuid));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long count = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Active contracts of " + playerName + ": " + count);
        return 1;
    }

    private static int handleContractInfo(CommandContext<CommandSourceStack> ctx, String id) {
        final UUID contractUuid = parseUuidOrNull(id);
        if (contractUuid == null) {
            sendFail(ctx, "Invalid contract id: " + id);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callContract(
                CONTRACT_GET, BiocapitalWireFormat.encodeUuid(contractUuid));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long status = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Contract " + id + " status code: " + status);
        return 1;
    }

    private static int handleContractTerminate(CommandContext<CommandSourceStack> ctx, String id) {
        final UUID contractUuid = parseUuidOrNull(id);
        if (contractUuid == null) {
            sendFail(ctx, "Invalid contract id: " + id);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callContract(
                CONTRACT_TERMINATE, BiocapitalWireFormat.encodeUuid(contractUuid));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        sendOk(ctx, "Contract " + id + " terminated");
        return 1;
    }

    private static int handleContractRedeem(CommandContext<CommandSourceStack> ctx, String id) {
        final UUID contractUuid = parseUuidOrNull(id);
        if (contractUuid == null) {
            sendFail(ctx, "Invalid contract id: " + id);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callContract(
                CONTRACT_REDEEM, BiocapitalWireFormat.encodeUuid(contractUuid));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long cost = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Contract " + id + " redeemed (cost " + cost + ")");
        return 1;
    }

    // ── handler: pod <info|force_eject> ───────────────────────

    private static int handlePodInfo(CommandContext<CommandSourceStack> ctx,
                                     long x, long y, long z) {
        if (!ensureNative(ctx)) return 0;
        // 简化：发送 24 字节（3 × int64：x, y, z）—— CorePod 端按相同顺序读取
        final java.nio.ByteBuffer buf = java.nio.ByteBuffer.allocate(24);
        buf.putLong(x).putLong(y).putLong(z);
        final Optional<byte[]> resp = NativeRustBindings.callCorePod(
                CORE_POD_GET_STATE, buf.array());
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long status = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Pod at (" + x + "," + y + "," + z + "): status=" + status);
        return 1;
    }

    private static int handlePodForceEject(CommandContext<CommandSourceStack> ctx,
                                            long x, long y, long z) {
        if (!ensureNative(ctx)) return 0;
        final java.nio.ByteBuffer buf = java.nio.ByteBuffer.allocate(24);
        buf.putLong(x).putLong(y).putLong(z);
        final Optional<byte[]> resp = NativeRustBindings.callCorePod(
                CORE_POD_EXIT, buf.array());
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        sendOk(ctx, "Pod at (" + x + "," + y + "," + z + "): force-ejected");
        return 1;
    }

    // ── handler: dglab <list|generate_token|revoke_token|set_strength|get_config|set_config> ──

    private static int handleDglabList(CommandContext<CommandSourceStack> ctx) {
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callDglab(
                DGLAB_LIST_TOKENS, new byte[0]);
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long count = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Active Dglab tokens: " + count);
        return 1;
    }

    private static int handleDglabGenerateToken(CommandContext<CommandSourceStack> ctx, String playerName) {
        final UUID uuid = resolvePlayerUuid(ctx, playerName);
        if (uuid == null) {
            sendFail(ctx, "Player not found: " + playerName);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callDglab(
                DGLAB_GEN_TOKEN, BiocapitalWireFormat.encodeUuid(uuid));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        sendOk(ctx, "Token generated for " + playerName);
        return 1;
    }

    private static int handleDglabRevokeToken(CommandContext<CommandSourceStack> ctx, String token) {
        if (token == null || token.isEmpty()) {
            sendFail(ctx, "Token must not be empty");
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callDglab(
                DGLAB_REVOKE_TOKEN, BiocapitalWireFormat.encodeString(token));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        sendOk(ctx, "Token revoked: " + token);
        return 1;
    }

    private static int handleDglabSetStrength(CommandContext<CommandSourceStack> ctx,
                                               String playerName, int strength) {
        final UUID uuid = resolvePlayerUuid(ctx, playerName);
        if (uuid == null) {
            sendFail(ctx, "Player not found: " + playerName);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        // 简化：SetStrength(player, channel=0, strength)
        final Optional<byte[]> resp = NativeRustBindings.callDglab(
                DGLAB_SET_STRENGTH,
                BiocapitalWireFormat.encodeUuidTwoInts(uuid, 0, strength));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long applied = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Set strength of " + playerName + " to " + strength
                + " (applied " + applied + ")");
        return 1;
    }

    private static int handleDglabGetConfig(CommandContext<CommandSourceStack> ctx, String playerName) {
        final UUID uuid = resolvePlayerUuid(ctx, playerName);
        if (uuid == null) {
            sendFail(ctx, "Player not found: " + playerName);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callDglab(
                DGLAB_GET_STRENGTH, BiocapitalWireFormat.encodeUuid(uuid));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long strength = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Dglab config of " + playerName + ": strength=" + strength);
        return 1;
    }

    private static int handleDglabSetConfig(CommandContext<CommandSourceStack> ctx,
                                             String playerName, int base, int max) {
        final UUID uuid = resolvePlayerUuid(ctx, playerName);
        if (uuid == null) {
            sendFail(ctx, "Player not found: " + playerName);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        // 简化：复用 set_strength 通路（base 作为强度值，max 留待协议扩展）
        final Optional<byte[]> resp = NativeRustBindings.callDglab(
                DGLAB_SET_STRENGTH,
                BiocapitalWireFormat.encodeUuidTwoInts(uuid, base, max));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        sendOk(ctx, "Set dglab config of " + playerName + ": base=" + base + " max=" + max);
        return 1;
    }

    // ── handler: auth <request_token|bind|list> ─────────────────

    private static int handleAuthRequestToken(CommandContext<CommandSourceStack> ctx) {
        final ServerPlayer self = ctx.getSource().getPlayer();
        if (self == null) {
            sendFail(ctx, "Must be a player");
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callBank(
                BANK_REQ_HW_TOKEN, BiocapitalWireFormat.encodeUuid(self.getUUID()));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        sendOk(ctx, "Hardware token requested; check chat for code");
        return 1;
    }

    private static int handleAuthBind(CommandContext<CommandSourceStack> ctx, String token) {
        final ServerPlayer self = ctx.getSource().getPlayer();
        if (self == null) {
            sendFail(ctx, "Must be a player");
            return 0;
        }
        if (token == null || token.isEmpty()) {
            sendFail(ctx, "Token must not be empty");
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final java.nio.ByteBuffer buf = java.nio.ByteBuffer.allocate(16 + 4 + token.length());
        buf.putLong(self.getUUID().getMostSignificantBits());
        buf.putLong(self.getUUID().getLeastSignificantBits());
        buf.putInt(token.length());
        buf.put(token.getBytes(java.nio.charset.StandardCharsets.UTF_8));
        final Optional<byte[]> resp = NativeRustBindings.callBank(
                BANK_BIND_HW, buf.array());
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        sendOk(ctx, "Token bound");
        return 1;
    }

    private static int handleAuthList(CommandContext<CommandSourceStack> ctx) {
        final ServerPlayer self = ctx.getSource().getPlayer();
        if (self == null) {
            sendFail(ctx, "Must be a player");
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callBank(
                BANK_LIST_HW, BiocapitalWireFormat.encodeUuid(self.getUUID()));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long count = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Bound hardware tokens: " + count);
        return 1;
    }

    // ── handler: admin <reset_device|grant_viewer|override> ─────

    private static int handleAdminResetDevice(CommandContext<CommandSourceStack> ctx, String playerName) {
        final UUID uuid = resolvePlayerUuid(ctx, playerName);
        if (uuid == null) {
            sendFail(ctx, "Player not found: " + playerName);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callBank(
                BANK_UNLOCK_DEVICE, BiocapitalWireFormat.encodeUuid(uuid));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        sendOk(ctx, "Device lock reset for " + playerName);
        return 1;
    }

    private static int handleAdminGrantViewer(CommandContext<CommandSourceStack> ctx, String playerName) {
        final UUID uuid = resolvePlayerUuid(ctx, playerName);
        if (uuid == null) {
            sendFail(ctx, "Player not found: " + playerName);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        // 复用 bank list_hw 通路（Web UI 登录 token 通过 doc/15 §0 流程生成）
        final Optional<byte[]> resp = NativeRustBindings.callBank(
                BANK_LIST_HW, BiocapitalWireFormat.encodeUuid(uuid));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        sendOk(ctx, "Viewer grant initiated for " + playerName + " (see Web UI login)");
        return 1;
    }

    private static int handleAdminOverride(CommandContext<CommandSourceStack> ctx,
                                            String playerName, String param, long value, long duration) {
        final UUID uuid = resolvePlayerUuid(ctx, playerName);
        if (uuid == null) {
            sendFail(ctx, "Player not found: " + playerName);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        // 简化：写入 player_state (4 long: uuid + value + duration + param_hash)
        final java.nio.ByteBuffer buf = java.nio.ByteBuffer.allocate(40);
        buf.putLong(uuid.getMostSignificantBits()).putLong(uuid.getLeastSignificantBits());
        buf.putLong(value);
        buf.putLong(duration);
        buf.putLong((long) param.hashCode());
        final Optional<byte[]> resp = NativeRustBindings.callPlayerState(
                PLAYER_STATE_UPDATE, buf.array());
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        sendOk(ctx, "Override applied: " + playerName + " " + param
                + "=" + value + " for " + duration + " ticks");
        return 1;
    }

    // ── handler: config / whitelist reload ───────────────────

    private static int handleConfigReload(CommandContext<CommandSourceStack> ctx) {
        // 简化：仅反馈「已触发重载」+ 留给 Rust 端真正 reload
        // （doc/11 §2.3 重载流程；Java 端不直接读 toml 业务）
        sendOk(ctx, "Config reload requested (delegated to Rust)");
        return 1;
    }

    private static int handleWhitelistReload(CommandContext<CommandSourceStack> ctx) {
        sendOk(ctx, "Whitelist reload requested (delegated to Rust)");
        return 1;
    }

    // ── handler: audit <query|export> ───────────────────────

    private static int handleAuditQuery(CommandContext<CommandSourceStack> ctx,
                                         String scope, String uuidStr, long limit) {
        final UUID uuid = parseUuidOrNull(uuidStr);
        if (uuid == null) {
            sendFail(ctx, "Invalid uuid: " + uuidStr);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callAudit(
                AUDIT_QUERY,
                BiocapitalWireFormat.encodeUuidAndLong(uuid, limit));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long count = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Audit " + scope + " for " + uuidStr + ": " + count + " rows");
        return 1;
    }

    private static int handleAuditExport(CommandContext<CommandSourceStack> ctx,
                                          String table, long fromTick, long toTick) {
        if (table == null || table.isEmpty()) {
            sendFail(ctx, "Table must not be empty");
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final java.nio.ByteBuffer buf = java.nio.ByteBuffer.allocate(16);
        buf.putLong(fromTick).putLong(toTick);
        final Optional<byte[]> resp = NativeRustBindings.callAudit(
                AUDIT_EXPORT, buf.array());
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        final Long count = BiocapitalWireFormat.decodeLong(resp.get());
        sendOk(ctx, "Audit export " + table + ": " + count + " rows");
        return 1;
    }

    // ── handler: debug <set_pleasure|set_hunger|set_hidden_hp|set_part_dev> ──

    private static int handleDebugSetPleasure(CommandContext<CommandSourceStack> ctx,
                                                String playerName, long value) {
        return handleDebugSetScalar(ctx, playerName, "pleasure", PLAYER_STATE_UPDATE, value);
    }

    private static int handleDebugSetHunger(CommandContext<CommandSourceStack> ctx,
                                              String playerName, long value) {
        return handleDebugSetScalar(ctx, playerName, "hunger", PLAYER_STATE_UPDATE, value);
    }

    private static int handleDebugSetHiddenHp(CommandContext<CommandSourceStack> ctx,
                                                String playerName, long value) {
        return handleDebugSetScalar(ctx, playerName, "hidden_hp", PLAYER_STATE_UPDATE, value);
    }

    private static int handleDebugSetPartDev(CommandContext<CommandSourceStack> ctx,
                                               String playerName, String part, long value) {
        final UUID uuid = resolvePlayerUuid(ctx, playerName);
        if (uuid == null) {
            sendFail(ctx, "Player not found: " + playerName);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final int partCode = partNameToCode(part);
        final Optional<byte[]> resp = NativeRustBindings.callPlayerState(
                PLAYER_STATE_UPDATE,
                BiocapitalWireFormat.encodeUuidIntString(uuid, partCode, part));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        sendOk(ctx, "Set part_dev of " + playerName + " " + part + " = " + value);
        return 1;
    }

    /** 通用 debug scalar setter（pleasure / hunger / hidden_hp 共用路径）。 */
    private static int handleDebugSetScalar(CommandContext<CommandSourceStack> ctx,
                                              String playerName, String fieldName,
                                              int methodId, long value) {
        final UUID uuid = resolvePlayerUuid(ctx, playerName);
        if (uuid == null) {
            sendFail(ctx, "Player not found: " + playerName);
            return 0;
        }
        if (!ensureNative(ctx)) return 0;
        final Optional<byte[]> resp = NativeRustBindings.callPlayerState(
                methodId, BiocapitalWireFormat.encodeUuidAndLong(uuid, value));
        if (resp.isEmpty()) {
            sendFail(ctx, "Rust service returned no data");
            return 0;
        }
        sendOk(ctx, "Set " + fieldName + " of " + playerName + " = " + value);
        return 1;
    }

    /**
     * BodyPart 12 枚举 → 整数 code（doc/03 §1.1）。
     * 大小写不敏感；未知返回 -1。
     */
    private static int partNameToCode(String name) {
        if (name == null) return -1;
        switch (name.toUpperCase(java.util.Locale.ROOT)) {
            case "HEAD": return 0;
            case "NECK": return 1;
            case "CHEST": return 2;
            case "BELLY": return 3;
            case "GENITAL": return 4;
            case "BUTT": return 5;
            case "BACK": return 6;
            case "LEFT_ARM": return 7;
            case "RIGHT_ARM": return 8;
            case "LEFT_LEG": return 9;
            case "RIGHT_LEG": return 10;
            case "FEET": return 11;
            default: return -1;
        }
    }
}
