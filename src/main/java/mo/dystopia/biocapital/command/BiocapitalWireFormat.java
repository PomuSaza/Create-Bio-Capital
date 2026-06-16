package mo.dystopia.biocapital.command;

import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.util.UUID;

/**
 * 2026-06-15 task #130: 简化 wire format 编码 helper。
 *
 * <p><strong>背景</strong>：当前 Java 端<strong>未</strong>引入
 * protobuf-java 依赖（{@code proto-gen} Gradle 任务尚未配置 —
 * doc/16 §3.5 仅描述了「Java 端共享 proto」目标但实际未生成 Java
 * stub），但 Rust 端 gRPC dispatch 期望
 * {@code prost::Message::decode} 兼容的字节流。
 *
 * <p><strong>过渡方案</strong>：在 proto-stub 落地之前，Java 端用
 * {@link java.nio.ByteBuffer} <strong>大端序定长</strong>编码
 * 字段（每 8 字节一个 {@code long}）。Rust 端在 JNI dispatch
 * 层按相同字段顺序读取。详见
 * {@code doc/SYSTEM_PROMPT.md §3.6} 与
 * {@code doc/16-sable-bridge.md §3.5} 末尾的「自定义 wire
 * format」约定。
 *
 * <p><strong>不</strong>使用 protobuf 编码（不写
 * varint / length-delimited），避免手写 wire format 的
 * typo 来源（task #130 子 agent 经验）。
 *
 * <p><strong>字段顺序约定</strong>：handler 编码时按
 * <em>字段声明顺序</em>拼装；Rust 端解码时按相同顺序读取
 * {@code ByteBuffer.getLong()}。两端均需在变更时同步。
 *
 * <p><strong>TODO</strong>：{@code proto-gen} 任务配置完毕后，
 * 所有 helper 替换为 protoc-gen-java 生成的 stub。
 */
public final class BiocapitalWireFormat {

    private BiocapitalWireFormat() {
        // utility class
    }

    // ── 单字段 helper（裸 UUID）────────────────────────────────

    /**
     * 16 字节大端序：UUID 高 8 字节 + 低 8 字节。
     *
     * <p>对应 Rust 端：
     * <pre>
     *   let mut buf = [0u8; 16];
     *   buf[0..8].copy_from_slice(&uuid.as_bytes()[0..8]);
     *   buf[8..16].copy_from_slice(&uuid.as_bytes()[8..16]);
     * </pre>
     */
    public static byte[] encodeUuid(UUID id) {
        final ByteBuffer buf = ByteBuffer.allocate(16);
        buf.putLong(id.getMostSignificantBits());
        buf.putLong(id.getLeastSignificantBits());
        return buf.array();
    }

    /**
     * 24 字节：UUID（16 B）+ int64（8 B）。
     */
    public static byte[] encodeUuidAndLong(UUID id, long value) {
        final ByteBuffer buf = ByteBuffer.allocate(24);
        buf.putLong(id.getMostSignificantBits());
        buf.putLong(id.getLeastSignificantBits());
        buf.putLong(value);
        return buf.array();
    }

    /**
     * 32 字节：UUID（16 B）+ UUID（16 B）。
     */
    public static byte[] encodeUuidUuid(UUID a, UUID b) {
        final ByteBuffer buf = ByteBuffer.allocate(32);
        buf.putLong(a.getMostSignificantBits());
        buf.putLong(a.getLeastSignificantBits());
        buf.putLong(b.getMostSignificantBits());
        buf.putLong(b.getLeastSignificantBits());
        return buf.array();
    }

    /**
     * 40 字节：UUID（16 B）+ UUID（16 B）+ int64（8 B）。
     */
    public static byte[] encodeUuidUuidLong(UUID a, UUID b, long value) {
        final ByteBuffer buf = ByteBuffer.allocate(40);
        buf.putLong(a.getMostSignificantBits());
        buf.putLong(a.getLeastSignificantBits());
        buf.putLong(b.getMostSignificantBits());
        buf.putLong(b.getLeastSignificantBits());
        buf.putLong(value);
        return buf.array();
    }

    /**
     * 24 字节：UUID（16 B）+ int32（4 B）+ int32（4 B）。
     *
     * <p>用于 DG_LAB {@code SetStrengthRequest}
     * （player + channel + strength）。
     */
    public static byte[] encodeUuidTwoInts(UUID id, int first, int second) {
        final ByteBuffer buf = ByteBuffer.allocate(24);
        buf.putLong(id.getMostSignificantBits());
        buf.putLong(id.getLeastSignificantBits());
        buf.putInt(first);
        buf.putInt(second);
        return buf.array();
    }

    /**
     * 32 字节：UUID（16 B）+ int32（4 B）+ int32（4 B）+ int32（4 B）+ 4 B padding。
     *
     * <p>用于 {@code set_dglab_config(player, base, max, _pad)}。
     */
    public static byte[] encodeUuidThreeInts(UUID id, int first, int second, int third) {
        final ByteBuffer buf = ByteBuffer.allocate(32);
        buf.putLong(id.getMostSignificantBits());
        buf.putLong(id.getLeastSignificantBits());
        buf.putInt(first);
        buf.putInt(second);
        buf.putInt(third);
        buf.putInt(0); // padding for 8-byte alignment
        return buf.array();
    }

    /**
     * 24 字节：UUID（16 B）+ int32（4 B）+ int32（4 B）+ int32（4 B）+
     * 1 B part code + 3 B padding。
     *
     * <p>用于 {@code set_part_dev(player, part_code, value, _pad)}。
     * part_code ∈ {0..11}（12 BodyPart 枚举，doc/03 §1.1）。
     */
    public static byte[] encodeUuidIntString(UUID id, int partCode, String partName) {
        final byte[] nameBytes = partName == null
                ? new byte[0]
                : partName.getBytes(StandardCharsets.UTF_8);
        final ByteBuffer buf = ByteBuffer.allocate(16 + 4 + 4 + nameBytes.length);
        buf.putLong(id.getMostSignificantBits());
        buf.putLong(id.getLeastSignificantBits());
        buf.putInt(partCode);
        buf.putInt(nameBytes.length);
        buf.put(nameBytes);
        return buf.array();
    }

    // ── 简单字段 helper（无 UUID）──────────────────────────────

    /** 8 字节：单个 int64。 */
    public static byte[] encodeLong(long value) {
        return ByteBuffer.allocate(8).putLong(value).array();
    }

    /** 4 字节：单个 int32。 */
    public static byte[] encodeInt(int value) {
        return ByteBuffer.allocate(4).putInt(value).array();
    }

    /**
     * 字符串（UTF-8）+ 4 字节前缀长度。
     *
     * <p>注：当前<strong>未</strong>使用；保留供后续 string-only
     * handler 使用（如 token revoke）。
     */
    public static byte[] encodeString(String value) {
        final byte[] bytes = value == null ? new byte[0] : value.getBytes(StandardCharsets.UTF_8);
        final ByteBuffer buf = ByteBuffer.allocate(4 + bytes.length);
        buf.putInt(bytes.length);
        buf.put(bytes);
        return buf.array();
    }

    // ── 响应解码 helper ────────────────────────────────────────

    /**
     * 解码单个 int64（银行余额 / 阈值 / 数量）。
     *
     * @return long 值，{@code null} 表示解码失败（响应空 / 长度不足）
     */
    public static Long decodeLong(byte[] bytes) {
        if (bytes == null || bytes.length < 8) {
            return null;
        }
        return ByteBuffer.wrap(bytes).getLong();
    }

    /**
     * 解码单个 int32。
     */
    public static Integer decodeInt(byte[] bytes) {
        if (bytes == null || bytes.length < 4) {
            return null;
        }
        return ByteBuffer.wrap(bytes).getInt();
    }
}
