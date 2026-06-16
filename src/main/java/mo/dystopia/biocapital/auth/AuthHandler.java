package mo.dystopia.biocapital.auth;

import java.nio.charset.StandardCharsets;
import java.util.Optional;
import java.util.UUID;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import net.minecraft.network.chat.Component;
import net.minecraft.server.level.ServerPlayer;
import net.neoforged.bus.api.SubscribeEvent;
import net.neoforged.fml.common.EventBusSubscriber;
import net.neoforged.neoforge.common.NeoForge;
import net.neoforged.neoforge.event.entity.player.PlayerEvent;

import mo.dystopia.biocapital.NativeRustBindings;

/**
 * 2026-06-14 task #83 (doc/18-tg-whitelist.md §3.1): 玩家登录
 * 时调 Rust {@code Authenticate} 流程。
 *
 * <p><strong>职责边界（强制）</strong>：本类**仅**做
 * <ol>
 *   <li>采集 {@code hardware_id_hash}（调 {@link HardwareIdCollector}）</li>
 *   <li>把 <code>(player_uuid, player_username, hardware_id_hash)</code>
 *       三个字段组装成 {@code AuthenticateRequest} 的字节数组</li>
 *   <li>调 {@link NativeRustBindings#callBank(int, byte[])} 转发给 Rust</li>
 *   <li>根据 Rust 的 {@code AuthenticateResponse.allowed} 决定
 *       {@code disconnect(reason)} 或放行</li>
 * </ol>
 *
 * <p><strong>不</strong>做：白名单判定、硬件 token 颁发、状态机
 * 推进、审计写入。全部走 Rust 端（{@code BankService.Authenticate}
 * RPC + {@code HardwareTokenService}）。
 *
 * <p>本类保留 Sable JNI 风格：Rust 端不可达时（{@code NATIVE_AVAILABLE
 * = false}）走降级路径 — 直接放行玩家 + 写 WARN 日志，与
 * 11 §10 (16-sable-bridge §10) 的「degraded mode is by design」一致。
 *
 * <p>method_id 约定（BankService 调度表）:
 * <pre>
 *   0  GetBalance
 *   1  Deposit
 *   2  Withdraw
 *   3  Transfer
 *   4  GetHistory
 *   5  LockDevice
 *   6  UnlockDevice
 *   7  GenerateInviteCode
 *   8  AcceptInviteCode
 *   9  RequestHardwareToken     (18 §7)
 *  10  BindHardware              (18 §7)
 *  11  ListHardware              (18 §7)
 *  12  RevokeHardware            (18 §7)
 *  13  Authenticate              (18 §7)  ← 本类用这个
 * </pre>
 */
@EventBusSubscriber(modid = mo.dystopia.biocapital.BioCapital.MODID, bus = EventBusSubscriber.Bus.GAME)
public final class AuthHandler {

    private static final Logger LOGGER = LoggerFactory.getLogger(AuthHandler.class);
    /** BankService.Authenticate 的 method_id（doc/18 §7）。 */
    public static final int METHOD_ID_AUTHENTICATE = 13;

    private AuthHandler() {
        // utility class — no instances
    }

    @SubscribeEvent
    public static void onPlayerLoggedIn(PlayerEvent.PlayerLoggedInEvent event) {
        if (!(event.getEntity() instanceof ServerPlayer player)) {
            return;
        }
        // 仅服务端权威：客户端侧的 PlayerLoggedInEvent 不处理。
        if (player.level().isClientSide()) {
            return;
        }

        UUID playerUuid = player.getUUID();
        String username = player.getGameProfile().getName();

        String hardwareIdHash;
        try {
            hardwareIdHash = HardwareIdCollector.collectHardwareId();
        } catch (HardwareIdCollector.HardwareIdUnavailableException e) {
            LOGGER.warn("[BioCapital] hardware_id collection failed for {}: {}",
                username, e.getMessage());
            player.connection.disconnect(Component.literal(
                "[BioCapital] cannot collect hardware_id on this host: "
                    + e.getMessage()));
            return;
        }

        // 序列化 AuthenticateRequest。proto 字段顺序固定（player_uuid
        // 16 bytes BE + player_username 长度前缀 + hardware_id_hash
        // 长度前缀 + 字符串 UTF-8 bytes）。Rust 端用 prost 解码。
        byte[] request = encodeAuthRequest(playerUuid, username, hardwareIdHash);

        Optional<byte[]> respOpt =
            NativeRustBindings.callBank(METHOD_ID_AUTHENTICATE, request);
        if (respOpt.isEmpty()) {
            // 降级：Rust 不可达 — 11 §10 允许放行 + WARN。
            LOGGER.warn("[BioCapital] NativeRustBindings.callBank({}) returned empty for {}; "
                + "allowing login in degraded mode (see doc/16-sable-bridge §10)",
                METHOD_ID_AUTHENTICATE, username);
            return;
        }

        boolean allowed;
        String reason;
        try {
            AuthResponse parsed = decodeAuthResponse(respOpt.get());
            allowed = parsed.allowed;
            reason = parsed.reason;
        } catch (Exception e) {
            LOGGER.warn("[BioCapital] Authenticate response parse failed for {}: {}",
                username, e.toString());
            return;
        }

        if (!allowed) {
            LOGGER.info("[BioCapital] denying login for {} (uuid={}) reason={}",
                username, playerUuid, reason);
            player.connection.disconnect(Component.literal(
                "[BioCapital] " + reason));
        } else {
            LOGGER.debug("[BioCapital] allowing login for {} (uuid={}) reason={}",
                username, playerUuid, reason);
        }
    }

    /**
     * Encode an {@code AuthenticateRequest} into proto-shaped
     * bytes. Field order matches the proto definition:
     *
     * <ol>
     *   <li>field 1, type 2 (LEN): Uuid player_uuid (16 bytes BE)</li>
     *   <li>field 2, type 2 (LEN): player_username UTF-8</li>
     *   <li>field 3, type 2 (LEN): hardware_id_hash UTF-8 (52 chars)</li>
     * </ol>
     *
     * <p>The wire format is the canonical protobuf
     * "length-delimited" encoding — see
     * <a href="https://protobuf.dev/programming-guides/encoding/">
     *   Protocol Buffers Encoding Guide
     * </a>.
     */
    public static byte[] encodeAuthRequest(UUID playerUuid, String username, String hardwareIdHash) {
        java.io.ByteArrayOutputStream out = new java.io.ByteArrayOutputStream();
        try {
            // field 1, wire type 2 = (1 << 3) | 2 = 10
            out.write(0x0A);
            byte[] uuidBytes = new byte[16];
            long msb = playerUuid.getMostSignificantBits();
            long lsb = playerUuid.getLeastSignificantBits();
            for (int i = 0; i < 8; i++) {
                uuidBytes[i] = (byte) ((msb >> (56 - i * 8)) & 0xFF);
                uuidBytes[8 + i] = (byte) ((lsb >> (56 - i * 8)) & 0xFF);
            }
            writeLenDelim(out, uuidBytes);

            // field 2, wire type 2
            out.write(0x12);
            writeLenDelim(out, username.getBytes(StandardCharsets.UTF_8));

            // field 3, wire type 2
            out.write(0x1A);
            writeLenDelim(out, hardwareIdHash.getBytes(StandardCharsets.UTF_8));
        } catch (java.io.IOException e) {
            // ByteArrayOutputStream never throws; the catch is
            // for source-level sanity.
            throw new RuntimeException(e);
        }
        return out.toByteArray();
    }

    /** Length-delimited record: varint length followed by the bytes. */
    private static void writeLenDelim(java.io.ByteArrayOutputStream out, byte[] data)
        throws java.io.IOException {
        writeVarint(out, data.length);
        out.write(data);
    }

    /** Unsigned base-128 varint encoder (canonical protobuf). */
    private static void writeVarint(java.io.ByteArrayOutputStream out, int value)
        throws java.io.IOException {
        while ((value & ~0x7F) != 0) {
            out.write((value & 0x7F) | 0x80);
            value >>>= 7;
        }
        out.write(value & 0x7F);
    }

    /**
     * Decode an {@code AuthenticateResponse} payload. Field order:
     * <ol>
     *   <li>field 1, wire type 0 (VARINT): bool allowed</li>
     *   <li>field 2, wire type 2 (LEN): string reason</li>
     * </ol>
     */
    public static AuthResponse decodeAuthResponse(byte[] payload) {
        if (payload == null || payload.length == 0) {
            return new AuthResponse(false, "");
        }
        java.io.ByteArrayInputStream in = new java.io.ByteArrayInputStream(payload);
        boolean allowed = false;
        String reason = "";
        try {
            while (in.available() > 0) {
                int tag = in.read();
                int fieldNumber = tag >>> 3;
                int wireType = tag & 0x07;
                if (fieldNumber == 1 && wireType == 0) {
                    // varint
                    int v = readVarint(in);
                    allowed = v != 0;
                } else if (fieldNumber == 2 && wireType == 2) {
                    int len = readVarint(in);
                    byte[] bytes = in.readNBytes(len);
                    reason = new String(bytes, StandardCharsets.UTF_8);
                } else {
                    // skip unknown field
                    skipField(in, wireType);
                }
            }
        } catch (java.io.IOException e) {
            throw new RuntimeException("AuthResponse decode failed", e);
        }
        return new AuthResponse(allowed, reason);
    }

    private static int readVarint(java.io.InputStream in) throws java.io.IOException {
        int result = 0;
        int shift = 0;
        int b;
        do {
            b = in.read();
            if (b < 0) throw new java.io.EOFException("varint truncated");
            result |= (b & 0x7F) << shift;
            shift += 7;
        } while ((b & 0x80) != 0);
        return result;
    }

    private static void skipField(java.io.InputStream in, int wireType)
        throws java.io.IOException {
        switch (wireType) {
            case 0 -> readVarint(in);  // varint
            case 1 -> in.readNBytes(8);  // 64-bit
            case 2 -> {  // length-delimited
                int len = readVarint(in);
                in.readNBytes(len);
            }
            case 5 -> in.readNBytes(4);  // 32-bit
            default -> throw new java.io.IOException("unknown wire type: " + wireType);
        }
    }

    /** Simple value object — kept package-private to mirror Rust `AuthenticateResponse`. */
    public record AuthResponse(boolean allowed, String reason) {}
}
