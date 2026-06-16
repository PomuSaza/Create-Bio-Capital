package mo.dystopia.biocapital;

import com.electronwill.nightconfig.core.UnmodifiableConfig;
import com.electronwill.nightconfig.toml.TomlParser;
import com.mojang.logging.LogUtils;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.ResourceLocation;
import net.neoforged.fml.loading.FMLPaths;
import org.slf4j.Logger;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * BioCapital configuration loader — Java 端 stub.
 *
 * <p><b>Task #14 / doc/11-config-system.md §1：</b> 业务字段读取已下沉到 Rust 端。
 * 本类仅保留 <i>HUD 客户端展示</i> 必需的字段（HUD_Display / HUD_Layout / HUD_Colors），
 * 其余业务字段（PlayerState / CorePod / Bank / Contracts / DGLAB …）由 Rust 端
 * {@code biocapital-cli/src/config.rs} 与对应模块的 PG repository 持有权威值。
 *
 * <p>Java 端读取 create_biocapital.toml 仅用于：
 * <ul>
 *   <li>HUD 渲染（display / layout / colors）—— 客户端本地 UI
 *   <li>HostileMob 黑/白名单（Create 6.0.10 spawn egg 过滤）—— 客户端预筛
 * </ul>
 *
 * <p>字段语义：本文件保留 5 节；其余节按 doc/11-config-system.md §1 在 toml 内仍是合法字段，
 * 但 <b>不</b> 在本类中读取。如需新增 HUD 相关字段，先改 doc/11 §1.2 再改本类。
 */
public final class Config {

    private static final Logger LOGGER = LogUtils.getLogger();
    private static final String FILE_NAME = "create_biocapital.toml";

    // Section names (must match the TOML exactly).
    private static final String SECTION_HUD_DISPLAY = "HUD_Display";
    private static final String SECTION_HUD_LAYOUT  = "HUD_Layout";
    private static final String SECTION_HUD_COLORS  = "HUD_Colors";
    private static final String SECTION_HOSTILE     = "HostileMobRemoval";

    // ── HUD display ──
    public static boolean showHUD;
    public static boolean showPleasureBar;
    public static boolean showHungerBar;
    public static boolean showPercentages;
    public static boolean debugHUD;

    // ── HUD layout ──
    public static int hudX;
    public static int hudY;
    public static int barWidth;
    public static int barHeight;
    public static int barSpacing;

    // ── HUD colors (ARGB hex strings) ──
    public static String hungerColor;
    public static String pleasureColor;
    public static String backgroundColor;
    public static String textColor;

    // ── Hostile mob policy (客户端 spawn egg 过滤) ──
    public static boolean hostileRemovalEnabled;
    public static boolean removeSpawnEggs;
    public static Set<String> mobWhitelist;
    public static Set<String> mobBlacklist;

    // ── 已移除（task #14 / task #79 原则：业务字段下沉 Rust 端）──
    // ❌ PlayerState.*      —— 02-player-state.md（Rust PlayerStateService 权威）
    // ❌ BodyDevelopment.*  —— 03-body-development.md（Rust BodyPartService 权威）
    // ❌ CorePod.*          —— 04-core-pod.md（Rust CorePodService 权威）
    // ❌ Fluids / Effect.*  —— 05-byproducts-fluids.md（Rust fluid_effects PG 权威）
    // ❌ Environment.*      —— 07-environment.md（Rust EnvironmentService 权威）
    // ❌ Bank.*             —— 08-bank.md（Rust BankService 权威）
    // ❌ Contracts.*        —— 09-contracts.md（Rust ContractService 权威）
    // ❌ DGLAB.*            —— 10-hardware-dglab.md（Rust DglabService + player_dglab_config 权威）
    // ❌ Commands.*         —— 12-command-system.md（Rust 端查询指令限额）
    // ❌ WebUI.* / Sable.* / RustServices.* / PG.* / Logging.* / Monitoring.*
    //                       —— 14 / 15 / 16 模块（Rust 端持有）
    // ❌ Whitelist.*        —— 18-tg-whitelist.md（Rust 端 whitelist.toml + 内存 cache 权威）
    //
    // 任何业务字段读取需求 → 走 Sable JNI → Rust gRPC → 业务模块；不要在本类重新加业务字段。

    private Config() {}

    // ── Public helpers ──────────────────────────────────────────

    public static int parseHexColor(String hex) {
        try {
            String s = hex.startsWith("#") ? hex.substring(1) : hex;
            return Integer.parseUnsignedInt(s, 16);
        } catch (NumberFormatException e) {
            LOGGER.warn("Invalid color string: {}, defaulting to white", hex);
            return 0xFFFFFFFF;
        }
    }

    public static int hungerColorArgb()     { return parseHexColor(hungerColor); }
    public static int pleasureColorArgb()   { return parseHexColor(pleasureColor); }
    public static int backgroundColorArgb() { return parseHexColor(backgroundColor); }
    public static int textColorArgb()       { return parseHexColor(textColor); }

    private static boolean validateEntityId(final Object obj) {
        return obj instanceof String s && BuiltInRegistries.ENTITY_TYPE.containsKey(ResourceLocation.parse(s));
    }

    // ── Entry point ─────────────────────────────────────────────

    /**
     * 启动期加载 + 热重载入口（11 §1.4 / 01 §1.3）。
     * 服务器侧通过 {@code /biocapital config reload} 触发（12 §X 指令）。
     */
    public static void load() {
        Path configPath = FMLPaths.CONFIGDIR.get().resolve(FILE_NAME);

        if (!Files.exists(configPath)) {
            LOGGER.warn("Config {} not found, using built-in defaults", configPath);
            applyDefaults();
            return;
        }

        UnmodifiableConfig root;
        try {
            String content = Files.readString(configPath, StandardCharsets.UTF_8);
            root = new TomlParser().parse(content);
        } catch (Exception e) {
            LOGGER.error("Failed to parse {}, using built-in defaults", configPath, e);
            applyDefaults();
            return;
        }

        // ── HUD ──
        UnmodifiableConfig hudDisplay = childOf(root, SECTION_HUD_DISPLAY);
        showHUD         = getBool(hudDisplay, "showHUD", true);
        showPleasureBar = getBool(hudDisplay, "showPleasureBar", true);
        showHungerBar   = getBool(hudDisplay, "showHungerBar", true);
        showPercentages = getBool(hudDisplay, "showPercentages", true);
        debugHUD        = getBool(hudDisplay, "debugHUD", false);

        UnmodifiableConfig hudLayout = childOf(root, SECTION_HUD_LAYOUT);
        hudX       = getInt(hudLayout, "hudX",       10, 0,    1000);
        hudY       = getInt(hudLayout, "hudY",      -40, -1000, 0);
        barWidth   = getInt(hudLayout, "barWidth",   80, 30,   300);
        barHeight  = getInt(hudLayout, "barHeight",  16, 6,    50);
        barSpacing = getInt(hudLayout, "barSpacing",  4, 0,    30);

        UnmodifiableConfig hudColors = childOf(root, SECTION_HUD_COLORS);
        hungerColor     = getString(hudColors, "hungerColor",     "#CCFFAA00");
        pleasureColor   = getString(hudColors, "pleasureColor",   "#CCFF69B4");
        backgroundColor = getString(hudColors, "backgroundColor", "#66333333");
        textColor       = getString(hudColors, "textColor",       "#FFFFFFFF");

        // ── HostileMobRemoval（客户端 spawn egg 预筛）──
        UnmodifiableConfig hostile = childOf(root, SECTION_HOSTILE);
        hostileRemovalEnabled = getBool(hostile, "enabled",         true);
        removeSpawnEggs       = getBool(hostile, "removeSpawnEggs", true);
        mobWhitelist    = new HashSet<>(getList(hostile, "whitelist"));
        mobBlacklist    = new HashSet<>(getList(hostile, "blacklist"));

        mobWhitelist.removeIf(id -> !validateEntityId(id));
        mobBlacklist.removeIf(id -> !validateEntityId(id));

        LOGGER.info("[BioCapital] Config loaded: {}", configPath);
    }

    // ── Internal helpers ────────────────────────────────────────

    private static void applyDefaults() {
        showHUD = true;
        showPleasureBar = true;
        showHungerBar = true;
        showPercentages = true;
        debugHUD = false;

        hudX = 10;
        hudY = -40;
        barWidth = 80;
        barHeight = 16;
        barSpacing = 4;

        hungerColor = "#CCFFAA00";
        pleasureColor = "#CCFF69B4";
        backgroundColor = "#66333333";
        textColor = "#FFFFFFFF";

        hostileRemovalEnabled = true;
        removeSpawnEggs = true;
        mobWhitelist = new HashSet<>();
        mobBlacklist = new HashSet<>();
    }

    private static UnmodifiableConfig childOf(UnmodifiableConfig parent, String key) {
        if (parent == null) return null;
        Object child = parent.get(key);
        return child instanceof UnmodifiableConfig u ? u : null;
    }

    private static boolean getBool(UnmodifiableConfig section, String key, boolean defaultValue) {
        if (section == null) return defaultValue;
        Object val = section.get(key);
        if (val instanceof Boolean b) return b;
        if (val != null) LOGGER.warn("Config {}.{} expected boolean, using default {}", section, key, defaultValue);
        return defaultValue;
    }

    private static int getInt(UnmodifiableConfig section, String key, int defaultValue, int min, int max) {
        if (section == null) return defaultValue;
        Object val = section.get(key);
        if (val instanceof Number n) {
            int i = n.intValue();
            if (i < min || i > max) {
                LOGGER.warn("Config {}.{}={} out of range [{}, {}], using default {}", section, key, i, min, max, defaultValue);
                return defaultValue;
            }
            return i;
        }
        if (val != null) LOGGER.warn("Config {}.{} expected int, using default {}", section, key, defaultValue);
        return defaultValue;
    }

    private static String getString(UnmodifiableConfig section, String key, String defaultValue) {
        if (section == null) return defaultValue;
        Object val = section.get(key);
        if (val instanceof String s) return s;
        if (val != null) LOGGER.warn("Config {}.{} expected string, using default {}", section, key, defaultValue);
        return defaultValue;
    }

    @SuppressWarnings("unchecked")
    private static List<String> getList(UnmodifiableConfig section, String key) {
        if (section == null) return Collections.emptyList();
        Object val = section.get(key);
        if (val instanceof List<?> list) {
            return (List<String>) list.stream()
                    .filter(e -> e instanceof String)
                    .map(e -> (String) e)
                    .toList();
        }
        return Collections.emptyList();
    }
}