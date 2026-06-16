package mo.dystopia.biocapital.auth;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Base64;
import java.util.UUID;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * 2026-06-14 task #83 (doc/18-tg-whitelist.md §4.2): 跨平台硬件 ID
 * 采集（<strong>仅 Java 端职责</strong>）。
 *
 * <p>采集流程：
 * <ol>
 *   <li>读取系统盘 serial（Windows / Linux / macOS / VM 各自实现）</li>
 *   <li>读取 OS 版本 + 主板 UUID</li>
 *   <li>组合成 <code>"&lt;raw_serial&gt;|&lt;machine_uuid&gt;|&lt;os_version&gt;"</code></li>
 *   <li>SHA-256 + base32 编码（52 字符；18 §4.2）</li>
 *   <li>通过 Sable JNI 上报给 Rust，<strong>不再做任何业务判断</strong></li>
 * </ol>
 *
 * <p>采集失败抛 {@link HardwareIdUnavailableException} — 上层
 * {@link AuthHandler} 决定如何向玩家呈现。
 *
 * <p>授权：物理磁盘读取 / 进程提升 / 跨平台 command 执行 — 这些
 * 是 Java 端**唯一**的合法职责；任何业务逻辑都应去 Rust 端。
 *
 * <h2>Status</h2>
 * <p><strong>Stubs only</strong>: 每个平台的方法体仅返回占位
 * 字符串（"<code>UNKNOWN</code>"）。真实 OS 命令（{@code wmic} /
 * {@code ioreg} / {@code dmidecode}）的拼装与超时控制留待真实部署
 * 落地；本类不持有任何业务状态。
 */
public final class HardwareIdCollector {

    private static final Logger LOGGER = LoggerFactory.getLogger(HardwareIdCollector.class);

    /** Expected length of the base32-encoded SHA-256 hash. */
    public static final int HARDWARE_ID_HASH_LEN = 52;

    private HardwareIdCollector() {
        // utility class — no instances
    }

    /**
     * Main entry point. Returns a 52-char base32 string.
     *
     * @throws HardwareIdUnavailableException if the collector cannot
     *         read the OS-level serial on this host
     */
    public static String collectHardwareId() throws HardwareIdUnavailableException {
        String rawSerial = readSystemDiskSerial();
        String machineUuid = readMachineUuid();
        String osVersion = readOsVersion();
        return computeHardwareId(rawSerial, machineUuid, osVersion);
    }

    /**
     * Pure hash function — extracted so the gRPC layer can verify
     * the wire contract (52-char base32) without re-running the
     * OS probes.
     *
     * <p>Mirrors the doc/18 §4.2 pseudo-code:
     * <pre>
     * SHA-256 of "&lt;raw_serial&gt;|&lt;machine_uuid&gt;|&lt;os_version&gt;"
     * → base32 encode (no padding)
     * </pre>
     */
    public static String computeHardwareId(String rawSerial, String machineUuid, String osVersion) throws HardwareIdUnavailableException {
        String composite = rawSerial + "|" + machineUuid + "|" + osVersion;
        try {
            byte[] hash = MessageDigest.getInstance("SHA-256")
                .digest(composite.getBytes(StandardCharsets.UTF_8));
            // Java's Base64.getEncoder().withoutPadding() with
            // the BASE32 alphabet is not built-in; we hand-roll
            // a small RFC 4648 base32 encoder to keep the 52-char
            // wire contract from doc/18 §4.2.
            return base32EncodeNoPad(hash);
        } catch (NoSuchAlgorithmException e) {
            // SHA-256 is a JDK-required algorithm; this branch
            // is only reachable on a misconfigured JRE.
            throw new HardwareIdUnavailableException(
                "SHA-256 not available on this JRE", e);
        }
    }

    /**
     * RFC 4648 base32 (no padding). Output length is
     * {@code ceil(input_len * 8 / 5)} = 52 for a 32-byte SHA-256
     * input.
     */
    static String base32EncodeNoPad(byte[] data) {
        final String alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        StringBuilder sb = new StringBuilder();
        int buffer = 0;
        int bitsLeft = 0;
        for (byte b : data) {
            buffer = (buffer << 8) | (b & 0xFF);
            bitsLeft += 8;
            while (bitsLeft >= 5) {
                int idx = (buffer >> (bitsLeft - 5)) & 0x1F;
                sb.append(alphabet.charAt(idx));
                bitsLeft -= 5;
            }
        }
        if (bitsLeft > 0) {
            int idx = (buffer << (5 - bitsLeft)) & 0x1F;
            sb.append(alphabet.charAt(idx));
        }
        return sb.toString();
    }

    // ── OS probes (stubs — see doc/18 §4.1) ─────────────────────────────────

    /**
     * Windows: <code>wmic diskdrive get serialnumber</code> for the
     * C: drive. PowerShell: <code>Get-PhysicalDisk | Select
     * SerialNumber</code>.
     */
    static String readSystemDiskSerialWindows() {
        // Stub: real implementation shells out to wmic / PowerShell.
        // Per doc/18 §4.1 the on-disk serial is "不可伪造，除非重装系统".
        return "UNKNOWN_WINDOWS_DISK_SERIAL";
    }

    /**
     * Linux: <code>/sys/class/block/sda/device/serial</code> or
     * <code>/dev/disk/by-id/ata-*</code>.
     */
    static String readSystemDiskSerialLinux() {
        // Stub: real implementation reads from /sys/class/block or
        // /dev/disk/by-id, picking the device backing the root
        // partition.
        return "UNKNOWN_LINUX_DISK_SERIAL";
    }

    /**
     * macOS: <code>ioreg -rd1 -c IOPMStorAVController | grep Serial</code>
     * or <code>system_profiler SPStorageDataType</code>.
     */
    static String readSystemDiskSerialMacos() {
        // Stub: real implementation runs ioreg / system_profiler.
        return "UNKNOWN_MACOS_DISK_SERIAL";
    }

    /**
     * VM: <code>dmidecode -s system-serial-number</code>.
     */
    static String readSystemDiskSerialVm() {
        // Stub: real implementation runs dmidecode (requires
        // root / sudo). VM-scope detection is the gRPC layer's
        // concern; this method is a best-effort fallback.
        return "UNKNOWN_VM_SYSTEM_SERIAL";
    }

    /** Dispatches to the right platform probe. */
    static String readSystemDiskSerial() throws HardwareIdUnavailableException {
        String os = System.getProperty("os.name", "").toLowerCase();
        if (os.contains("win")) {
            return readSystemDiskSerialWindows();
        } else if (os.contains("mac") || os.contains("darwin")) {
            return readSystemDiskSerialMacos();
        } else if (os.contains("nix") || os.contains("nux") || os.contains("aix")) {
            return readSystemDiskSerialLinux();
        } else {
            throw new HardwareIdUnavailableException(
                "unsupported OS: " + os);
        }
    }

    /**
     * Reads a stable machine UUID. On Linux this is
     * {@code /var/lib/dbus/machine-id} or {@code /etc/machine-id};
     * on Windows it is read from the registry; on macOS it is the
     * IOPlatformUUID.
     *
     * <p>Stub.
     */
    static String readMachineUuid() {
        // Stub: real implementation dispatches per OS.
        return "UNKNOWN_MACHINE_UUID";
    }

    /**
     * Returns a stable OS version string for the hash input. The
     * exact format is unconstrained by doc/18 — the hash is over
     * the raw bytes, so any stable string is fine.
     *
     * <p>Stub returns {@code os.name + os.version}.
     */
    static String readOsVersion() {
        return System.getProperty("os.name", "unknown")
            + " " + System.getProperty("os.version", "unknown");
    }

    /**
     * Thrown when the collector cannot read the OS-level serial.
     * The Java {@link AuthHandler} turns this into a "需要硬件
     * token，但本机无法采集" prompt to the player.
     */
    public static class HardwareIdUnavailableException extends Exception {
        public HardwareIdUnavailableException(String message) {
            super(message);
        }
        public HardwareIdUnavailableException(String message, Throwable cause) {
            super(message, cause);
        }
    }
}
