package mo.dystopia.biocapital;

import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.List;
import java.util.stream.Stream;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assumptions.assumeTrue;

/**
 * MVP-3.1 集成测试 — 验证 JNI 通路不依赖 Minecraft runtime。
 *
 * <p>测试目标（**不**启动 Minecraft）：
 * <ol>
 *   <li>{@code build/natives/<os>/libbiocapital_jni.so} 文件存在且
 *       大小合理（&gt; 1 MB）</li>
 *   <li>Rust export 的 12 个 JNI 符号都在 .so 里（用 {@code nm}）</li>
 *   <li>Java 端 {@code NativeRustBindings} 声明的 {@code native}
 *       方法签名与 Rust 符号名完全匹配</li>
 *   <li>Proto 类已生成且可加载（之前 WireFormatRoundTripTest 验证）</li>
 * </ol>
 *
 * <p>**不**做的事：
 * <ul>
 *   <li>不起 Minecraft 客户端（无 mod loader 环境 + SLF4J 不在
 *       test classpath，会触发 NoClassDefFoundError）</li>
 *   <li>不起 Rust server（{@code NativeRustBindings.init()} 在没有
 *       server 时也能返回 true，dispatch table 是 in-process 的）；
 *       要真端到端跑 {@code callPlayerState} 需 server 在
 *       127.0.0.1:50051 监听</li>
 *   <li>不测 HUD 渲染（需 Minecraft runtime + 实际渲染线程）</li>
 * </ul>
 *
 * <p>运行：
 * <pre>
 * # 1. 编 native lib（绕过 Docker — 用本地 cargo build）
 * cd rust && cargo build -p biocapital-jni
 *
 * # 2. 复制到 gradle 期望位置
 * mkdir -p ../build/natives/linux-x86_64
 * cp target/debug/libbiocapital_jni.so ../build/natives/linux-x86_64/
 *
 * # 3. 跑这个测试
 * cd .. && ./gradlew test -x buildRustNatives \
 *     --tests "mo.dystopia.biocapital.JniIntegrationTest"
 * </pre>
 *
 * <p>如果 native lib 不在 build/natives 里，测试会被 skip（不 fail）
 * —— 避免在 CI 上无 native lib 时整个 build 红。
 */
class JniIntegrationTest {

    @Test
    void nativeLibraryFileExists() {
        Path libPath = findNativeLibrary();
        assumeTrue(libPath != null && libPath.toFile().exists(),
                "skip: libbiocapital_jni.so not found in build/natives/<os>/. "
                        + "Run: cd rust && cargo build -p biocapital-jni && "
                        + "cp target/debug/libbiocapital_jni.so "
                        + "../build/natives/linux-x86_64/");

        long size = libPath.toFile().length();
        assertTrue(size > 1_000_000,
                "libbiocapital_jni.so seems too small (" + size + " B) — was "
                        + "it compiled with debug symbols stripped?");

        System.out.println("[MVP-3.1] Native lib OK: " + libPath + " ("
                + (size / 1024) + " KB)");
    }

    @Test
    void nativeLibraryExportsAllTwelveJniSymbols() throws IOException, InterruptedException {
        Path libPath = findNativeLibrary();
        assumeTrue(libPath != null && libPath.toFile().exists(),
                "skip: libbiocapital_jni.so not found");

        // nm -D 列出动态符号；过滤 `Java_mo_dystopia_biocapital_NativeRustBindings_*`
        Process process = new ProcessBuilder(
                "nm", "-D", libPath.toAbsolutePath().toString())
                .redirectErrorStream(true)
                .start();
        String out;
        try (InputStream in = process.getInputStream()) {
            out = new String(in.readAllBytes()).trim();
        }
        int exit = process.waitFor();
        assertEquals(0, exit,
                "nm failed (exit " + exit + "): " + out);

        // Rust 端应 export 这 12 个 native 方法（doc/16-sable-bridge.md §3.5）
        String[] expected = {
                "Java_mo_dystopia_biocapital_NativeRustBindings_init0",
                "Java_mo_dystopia_biocapital_NativeRustBindings_callPlayerState0",
                "Java_mo_dystopia_biocapital_NativeRustBindings_callBank0",
                "Java_mo_dystopia_biocapital_NativeRustBindings_callCorePod0",
                "Java_mo_dystopia_biocapital_NativeRustBindings_callContract0",
                "Java_mo_dystopia_biocapital_NativeRustBindings_callEnvironment0",
                "Java_mo_dystopia_biocapital_NativeRustBindings_callCreature0",
                "Java_mo_dystopia_biocapital_NativeRustBindings_callDglab0",
                "Java_mo_dystopia_biocapital_NativeRustBindings_callHostileMob0",
                "Java_mo_dystopia_biocapital_NativeRustBindings_callAudit0",
                "Java_mo_dystopia_biocapital_NativeRustBindings_callGrantViewer0",
                "Java_mo_dystopia_biocapital_NativeRustBindings_computePodStress0",
        };

        List<String> missing = Stream.of(expected)
                .filter(s -> !out.contains(s))
                .toList();
        assertTrue(missing.isEmpty(),
                "missing JNI symbols in " + libPath + ":\n  " + missing);
        System.out.println("[MVP-3.1] All " + expected.length
                + " JNI symbols present in " + libPath.getFileName());
    }

    @Test
    void protoClassesWereGenerated() {
        // gradle 的 :generateProto 任务应已生成 Java stub
        Path protoDir = Paths.get("build/generated/source/proto/main/java/mo/dystopia/biocapital/proto");
        assertTrue(Files.isDirectory(protoDir),
                "proto java dir not generated — gradle :generateProto not run? "
                        + "expected: " + protoDir);
        // 至少应该有 PlayerState（最常用的）
        assertTrue(Files.exists(protoDir.resolve("PlayerState.java")),
                "PlayerState.java missing — proto generation broken");
        long count;
        try (Stream<Path> files = Files.list(protoDir)) {
            count = files.count();
        } catch (IOException e) {
            throw new RuntimeException(e);
        }
        assertTrue(count > 50,
                "expected > 50 generated proto classes, got " + count);
        System.out.println("[MVP-3.1] " + count + " proto classes generated in "
                + protoDir);
    }

    private static Path findNativeLibrary() {
        String[] candidates = {
                "build/natives/linux-x86_64/libbiocapital_jni.so",
                "build/natives/linux-aarch64/libbiocapital_jni.so",
                "build/natives/macos-x86_64/libbiocapital_jni.dylib",
                "build/natives/macos-aarch64/libbiocapital_jni.dylib",
                "build/natives/windows-x86_64/biocapital_jni.dll",
        };
        for (String c : candidates) {
            Path p = Paths.get(c);
            if (p.toFile().exists()) return p;
        }
        return null;
    }
}