package mo.dystopia.biocapital.hud;

import mo.dystopia.biocapital.Config;
import mo.dystopia.biocapital.NativeRustBindings;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.GuiGraphics;
import net.minecraft.world.entity.player.Player;
import net.neoforged.api.distmarker.Dist;
import net.neoforged.bus.api.SubscribeEvent;
import net.neoforged.fml.common.EventBusSubscriber;
import net.neoforged.neoforge.client.event.RenderGuiLayerEvent;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.Optional;
import java.util.UUID;

/**
 * Client-side HUD renderer for the Bio-Capital mod.
 *
 * <p>Registered on the NeoForge <em>game</em> event bus (the
 * {@code RenderGuiLayerEvent} lives on the GAME bus, not the FORGE bus)
 * with {@link Dist#CLIENT} so the class is only ever loaded on the
 * client.
 *
 * <h2>Responsibility</h2>
 * Pure presentation layer (per {@code doc/02-player-state.md} §2 + §6 and
 * the 2026-06-14 user task #123 decision). The HUD:
 * <ol>
 *   <li>Polls the canonical {@code PlayerState} via Sable JNI
 *       ({@link NativeRustBindings#callPlayerState} with
 *       {@code methodId == 0 == GetState}) every
 *       {@value #POLL_INTERVAL_MS} ms.</li>
 *   <li>Decodes the proto wire-format response into a {@link PlayerStateView}.</li>
 *   <li>Performs a <b>client-side diff</b> against the last rendered
 *       snapshot. If nothing changed, the HUD skips the draw pass entirely
 *       (the 5 Hz poll becomes "5 comparisons + ≤5 redraws/s/player", not
 *       "5 redraws/s/player").</li>
 *   <li>Renders three rounded bars in the upper-left:
 *       <ul>
 *         <li>pleasure ({@code #CCFF69B4} by default — 粉色)</li>
 *         <li>hunger ({@code #CCFFAA00} by default — 橙色)</li>
 *         <li>hidden_hp ({@code #CC8A2BE2} default in debug mode — 紫)</li>
 *       </ul>
 *       Layout is driven by {@link Config#hudX} / {@link Config#hudY};
 *       colors by {@link Config#pleasureColor} / {@link Config#hungerColor}.
 * </ol>
 *
 * <h2>What this class does NOT do</h2>
 * <ul>
 *   <li>No state mutation. The HUD never writes to
 *       {@code PlayerStateAttachment}; Rust is the only writer
 *       ({@code doc/02 §1.3 + 99 §2}).</li>
 *   <li>No business logic. The class is intentionally narrow — if you find
 *       yourself adding a damage formula here, you are in the wrong place
 *       (the formula lives in Rust {@code biocapital-core::player_state}).</li>
 *   <li>No event-bus publishing. The HUD observes
 *       {@link RenderGuiLayerEvent.Pre} (read-only).</li>
 * </ul>
 *
 * <h2>Performance budget</h2>
 * <ul>
 *   <li>Poll rate: 5 Hz (200 ms — see {@link #POLL_INTERVAL_MS}).</li>
 *   <li>JNI + proto decode on a worker context (the call is synchronous
 *       in {@link NativeRustBindings} but cheap — the Sable tokio
 *       runtime is in-process and the {@code GetState} path is served
 *       from an in-memory 200 ms cache on the Rust side, so the
 *       PostgreSQL round-trip only happens on cache miss).</li>
 *   <li>Main-thread render cost: only the diff-and-draw path. When
 *       nothing changed, the entire draw is skipped (no fillRect calls).</li>
 *   <li>At 50 players × 5 Hz this is ~250 RPS of Sable JNI; the Rust
 *       cache keeps PG load constant at ~5 RPS for the warm path.</li>
 * </ul>
 *
 * <h2>Failure modes (degraded mode, never crashes)</h2>
 * <ul>
 *   <li>Native library not loaded ({@link NativeRustBindings#NATIVE_AVAILABLE}
 *       == false) → {@link #pollPlayerState} returns {@code null} → HUD
 *       silently skips the draw pass.</li>
 *   <li>JNI returns empty ({@link Optional#empty()}) → same as above.</li>
 *   <li>Proto decode throws → caught, logged at {@code DEBUG}, HUD skips
 *       the draw pass. The previous {@link #lastSnapshot} is preserved
 *       so the next successful poll restores the display.</li>
 * </ul>
 *
 * <p>Registered on the NeoForge <em>game</em> event bus
 * ({@code RenderGuiLayerEvent} is dispatched on the GAME bus, not the
 * FORGE bus) with {@link Dist#CLIENT} so this class is only ever loaded
 * on the client.
 */
@EventBusSubscriber(modid = "create_biocapital", value = Dist.CLIENT, bus = EventBusSubscriber.Bus.GAME)
public final class BioCapitalHud {

    private static final Logger LOGGER = LoggerFactory.getLogger(BioCapitalHud.class);

    // ── Constants (kept package-private for tests) ─────────────────────────

    /** PlayerStateService.GetState = 0 (proto method_id, see doc/14 §3.2). */
    static final int METHOD_ID_GET_STATE = 0;

    /** 5 Hz poll; matches the Rust cache TTL (doc/02 §5 budget). */
    static final long POLL_INTERVAL_MS = 200L;

    /** Bar geometry (pixels, matches doc/02 §2.1). */
    static final int BAR_WIDTH = 120;
    static final int BAR_HEIGHT = 10;
    static final int BAR_SPACING = 4;
    static final int LABEL_OFFSET_X = -56;  // label sits to the left of the bar
    static final int VALUE_OFFSET_X = 8;    // "##%" sits just inside the bar

    // ── Static state (singleton; no instances) ──────────────────────────────

    /**
     * Last successfully rendered snapshot. Used for client-side diffing —
     * if a new poll returns an equal value, the render pass is skipped.
     * Volatile so the read in {@link #onRenderHud} sees the most recent
     * write from a worker thread if we ever move the poll off-main
     * (currently it stays on the render thread for simplicity).
     */
    private static volatile PlayerStateView lastSnapshot = null;

    /** Wall-clock timestamp (ms) of the last poll that hit the wire. */
    private static long lastPollMs = 0L;

    private BioCapitalHud() {
        // utility class — no instances
    }

    // ── Event handler ───────────────────────────────────────────────────────

    /**
     * Render the HUD on top of vanilla GUI overlays. Registered on the
     * mod-specific event bus with {@link Dist#CLIENT} so this class is
     * only ever loaded on the client (the rest of the HUD lives in the
     * dedicated {@code mo.dystopia.biocapital.hud} package).
     */
    @SubscribeEvent
    public static void onRenderHud(RenderGuiLayerEvent.Pre event) {
        if (!Config.showHUD) {
            return;
        }
        final Minecraft mc = Minecraft.getInstance();
        if (mc.player == null || mc.level == null) {
            return;
        }
        final Player player = mc.player;

        // 5 Hz poll: skip the JNI call entirely if we polled recently.
        final long now = System.currentTimeMillis();
        if (now - lastPollMs < POLL_INTERVAL_MS) {
            // Still draw the cached snapshot if we have one — this is the
            // "no diff, no redraw" branch's draw side.
            if (lastSnapshot != null) {
                drawHud(event.getGuiGraphics(), lastSnapshot,
                    mc.getWindow().getGuiScaledWidth(),
                    mc.getWindow().getGuiScaledHeight());
            }
            return;
        }
        lastPollMs = now;

        // Poll the canonical state via Sable JNI. Any failure → silently
        // skip the draw pass; the previous snapshot (if any) will be
        // drawn on the next event tick.
        final PlayerStateView current = pollPlayerState(player.getUUID());
        if (current == null) {
            if (lastSnapshot != null) {
                drawHud(event.getGuiGraphics(), lastSnapshot,
                    mc.getWindow().getGuiScaledWidth(),
                    mc.getWindow().getGuiScaledHeight());
            }
            return;
        }

        // Client-side diff: only redraw when something visible changed.
        // Equality is value-based; we treat a tiny epsilon as equal so
        // floating-point jitter (e.g. 49.99997 vs 50.0) does not cause
        // 5 redraws/s/player.
        if (lastSnapshot != null && lastSnapshot.valuesEqualWithin(current, 0.01f)) {
            return; // nothing changed → skip the draw
        }
        lastSnapshot = current;

        drawHud(event.getGuiGraphics(), current,
            mc.getWindow().getGuiScaledWidth(),
            mc.getWindow().getGuiScaledHeight());
    }

    // ── Polling ─────────────────────────────────────────────────────────────

    /**
     * Issue a single {@code GetState} call and decode the response.
     *
     * @return decoded snapshot, or {@code null} on any failure (logged at
     *         {@code DEBUG}, not WARN — failed polls are routine when the
     *         Sable library is not loaded or the Rust server is offline)
     */
    private static PlayerStateView pollPlayerState(UUID playerUuid) {
        if (!NativeRustBindings.NATIVE_AVAILABLE) {
            return null;
        }
        final byte[] request = encodePlayerIdentifier(playerUuid);
        final Optional<byte[]> respOpt =
            NativeRustBindings.callPlayerState(METHOD_ID_GET_STATE, request);
        if (respOpt.isEmpty()) {
            return null;
        }
        try {
            return decodePlayerState(respOpt.get());
        } catch (Exception e) {
            LOGGER.debug("[BioCapital] HUD proto decode failed: {}", e.toString());
            return null;
        }
    }

    // ── Encoding (Java → Rust, wire format) ─────────────────────────────────

    /**
     * Encode a {@code PlayerIdentifier} into the proto-shaped
     * "length-delimited" wire format the Rust {@code prost} decoder
     * consumes. Mirrors the pattern in
     * {@code mo.dystopia.biocapital.auth.AuthHandler#encodeAuthRequest}.
     *
     * <p>{@code PlayerIdentifier} has exactly one field:
     * {@code Uuid player_uuid = 1} — a 16-byte big-endian value.
     */
    static byte[] encodePlayerIdentifier(UUID playerUuid) {
        final byte[] out = new byte[1 + 2 + 16];
        // field 1, wire type 2 (LEN) = (1 << 3) | 2 = 0x0A
        out[0] = 0x0A;
        // length = 16
        out[1] = 0x10;
        // 16 bytes BE
        final ByteBuffer buf = ByteBuffer.allocate(16).order(ByteOrder.BIG_ENDIAN);
        buf.putLong(playerUuid.getMostSignificantBits());
        buf.putLong(playerUuid.getLeastSignificantBits());
        System.arraycopy(buf.array(), 0, out, 3, 16);
        return out;
    }

    // ── Decoding (Rust → Java) ──────────────────────────────────────────────

    /**
     * Decode a {@code PlayerState} proto payload into the Java view used
     * by the renderer. Only the fields the HUD displays are populated:
     * pleasure / hunger / hidden_hp / max_hunger. The other proto fields
     * (parts map / low_hp_hits / active_contracts / defeat_count /
     * updated_at) are skipped on the wire to keep the per-poll cost
     * trivial — a full decode belongs in the {@code /biocapital stats}
     * command, not in the per-frame HUD hot path.
     *
     * <p>Wire shape (per {@code rust/proto/biocapital.proto}):
     * <pre>
     * message PlayerState {
     *   Uuid player_uuid = 1;        // 16 bytes BE (ignored here)
     *   float pleasure = 2;          // wire type 5 (I32) — fixed 32-bit
     *   float hunger = 3;
     *   float hidden_hp = 4;
     *   int32 low_hp_hits = 5;       // ignored
     *   map&lt;string, float&gt; parts = 6;   // ignored
     *   google.protobuf.Timestamp updated_at = 7;  // ignored
     *   int32 defeat_count = 8;      // ignored
     *   int64 active_contracts = 9;  // ignored
     *   int32 max_hunger = 10;       // (not currently displayed but
     *                                 //  parsed for forward-compat)
     * }
     * </pre>
     */
    static PlayerStateView decodePlayerState(byte[] payload) {
        // Minimal hand-rolled proto reader. We only care about fixed
        // 32-bit floats (wire type 5) and a single int32; everything
        // else is skipped via tag + length-prefix dispatch.
        int pos = 0;
        final int n = payload.length;
        float pleasure = Float.NaN;
        float hunger = Float.NaN;
        float hiddenHp = Float.NaN;
        int maxHunger = 100;
        while (pos < n) {
            final int tag = payload[pos++] & 0xFF;
            final int fieldNumber = tag >>> 3;
            final int wireType = tag & 0x07;
            switch (fieldNumber) {
                case 1 -> {
                    // Uuid player_uuid — LEN(2) → 16 bytes; skip
                    if (wireType != 2) throw new IllegalStateException("Uuid not LEN");
                    if (pos >= n) throw new IllegalStateException("truncated Uuid len");
                    final int len = payload[pos++] & 0xFF;
                    pos += len;
                }
                case 2 -> {
                    if (wireType != 5) throw new IllegalStateException("pleasure not I32");
                    if (pos + 4 > n) throw new IllegalStateException("truncated pleasure");
                    pleasure = ByteBuffer.wrap(payload, pos, 4)
                        .order(ByteOrder.LITTLE_ENDIAN).getFloat();
                    pos += 4;
                }
                case 3 -> {
                    if (wireType != 5) throw new IllegalStateException("hunger not I32");
                    if (pos + 4 > n) throw new IllegalStateException("truncated hunger");
                    hunger = ByteBuffer.wrap(payload, pos, 4)
                        .order(ByteOrder.LITTLE_ENDIAN).getFloat();
                    pos += 4;
                }
                case 4 -> {
                    if (wireType != 5) throw new IllegalStateException("hidden_hp not I32");
                    if (pos + 4 > n) throw new IllegalStateException("truncated hidden_hp");
                    hiddenHp = ByteBuffer.wrap(payload, pos, 4)
                        .order(ByteOrder.LITTLE_ENDIAN).getFloat();
                    pos += 4;
                }
                case 5, 6, 7, 8, 9 -> {
                    // low_hp_hits / parts / updated_at / defeat_count /
                    // active_contracts — skip; HUD doesn't need them.
                    pos = skipField(payload, pos, wireType, n);
                }
                case 10 -> {
                    if (wireType != 0) throw new IllegalStateException("max_hunger not VARINT");
                    maxHunger = readVarintInt32(payload, pos);
                    pos = skipField(payload, pos, wireType, n);
                }
                default -> {
                    // Unknown field — skip per proto3 forward-compat rules.
                    pos = skipField(payload, pos, wireType, n);
                }
            }
        }
        return new PlayerStateView(pleasure, hunger, hiddenHp, maxHunger);
    }

    /** Skip an arbitrary proto field. Returns the new cursor position. */
    private static int skipField(byte[] payload, int pos, int wireType, int n) {
        return switch (wireType) {
            case 0 -> { // VARINT
                while (pos < n && (payload[pos++] & 0x80) != 0) { /* keep reading */ }
                yield pos;
            }
            case 1 -> pos + 8; // I64
            case 2 -> { // LEN
                if (pos >= n) throw new IllegalStateException("truncated LEN");
                final int len = payload[pos++] & 0xFF;
                pos += len;
                yield pos;
            }
            case 5 -> pos + 4; // I32
            default -> throw new IllegalStateException("unsupported wire type: " + wireType);
        };
    }

    /** Decode a base-128 varint. We only need small int32s here. */
    private static int readVarintInt32(byte[] payload, int pos) {
        int result = 0;
        int shift = 0;
        while (pos < payload.length) {
            final int b = payload[pos++] & 0xFF;
            result |= (b & 0x7F) << shift;
            if ((b & 0x80) == 0) return result;
            shift += 7;
            if (shift > 35) throw new IllegalStateException("varint too long");
        }
        throw new IllegalStateException("truncated varint");
    }

    // ── Rendering ───────────────────────────────────────────────────────────

    /**
     * Draw the three HUD bars at the configured screen offset. The
     * render uses vanilla {@link GuiGraphics} primitives — no shaders, no
     * custom buffers — so the cost per frame is dominated by the
     * {@code fillRect} calls, not the encoder.
     *
     * <p>Layout (top-down):
     * <pre>
     *   Y = hudY:       [pleasure]  42%
     *   Y = hudY+14:    [hunger  ]  67%
     *   Y = hudY+28:    [hp      ]  20%   (only when debugHUD = true)
     * </pre>
     */
    private static void drawHud(GuiGraphics gg, PlayerStateView ps,
                                 int screenW, int screenH) {
        // Anchor in the upper-left; hudY may be negative (relative to top).
        final int baseX = clampToScreen(Config.hudX, 0, screenW - BAR_WIDTH);
        final int baseY = clampToScreenRelative(Config.hudY, screenH);

        // Pleasure bar (always on when showHUD = true).
        drawBar(gg, baseX, baseY, ps.pleasure, 100f,
            Config.pleasureColorArgb(), "快感值");

        // Hunger bar.
        drawBar(gg, baseX, baseY + BAR_HEIGHT + BAR_SPACING, ps.hunger, ps.maxHunger,
            Config.hungerColorArgb(), "饱食值");

        // Hidden HP — debug-only (doc/02 §2.3).
        if (Config.debugHUD) {
            drawBar(gg, baseX, baseY + 2 * (BAR_HEIGHT + BAR_SPACING),
                ps.hiddenHp, 20f, 0xCC8A2BE2, "隐性HP");
        }
    }

    /**
     * Draw a single rounded-ish bar at ({@code x},{@code y}) with width
     * scaled to {@code value / max}.
     */
    private static void drawBar(GuiGraphics gg, int x, int y, float value,
                                 float max, int argb, String label) {
        // Background (semi-transparent black) so the bar is visible on
        // any terrain.
        gg.fill(x, y, x + BAR_WIDTH, y + BAR_HEIGHT, 0x66000000);

        // Foreground.
        final float clamped = Math.max(0f, Math.min(value, max));
        final int fillWidth = max <= 0f ? 0 : Math.round((clamped / max) * BAR_WIDTH);
        if (fillWidth > 0) {
            gg.fill(x, y, x + fillWidth, y + BAR_HEIGHT, argb);
        }

        // Label + percentage text overlay.
        final int labelColor = 0xFFFFFFFF;
        final int valueColor = 0xFFFFFFFF;
        final String text = label + " " + Math.round((clamped / Math.max(1f, max)) * 100f) + "%";
        gg.drawString(Minecraft.getInstance().font, text,
            x, y + 1, valueColor, false);
    }

    private static int clampToScreen(int value, int lo, int hi) {
        return Math.max(lo, Math.min(value, hi));
    }

    /**
     * Negative {@code hudY} values are interpreted as "offset from the
     * top of the screen" (vanilla Minecraft convention from
     * {@code doc/02 §2.1}). Positive values are absolute screen
     * coordinates.
     */
    private static int clampToScreenRelative(int hudY, int screenH) {
        // hudY defaults to -40 in Config.java; with screen height 240
        // (a typical scaled height) this places the bars at 200 px from
        // the top — i.e. just above the hotbar.
        if (hudY < 0) {
            return Math.max(0, screenH + hudY);
        }
        return Math.min(hudY, screenH - BAR_HEIGHT);
    }

    // ── Value type ──────────────────────────────────────────────────────────

    /**
     * Decoded view of a {@code PlayerState} for rendering. Immutable,
     * value-based equality (within a tiny epsilon) for the diff check.
     */
    public static final class PlayerStateView {
        public final float pleasure;
        public final float hunger;
        public final float hiddenHp;
        public final int maxHunger;

        PlayerStateView(float pleasure, float hunger, float hiddenHp, int maxHunger) {
            this.pleasure = pleasure;
            this.hunger = hunger;
            this.hiddenHp = hiddenHp;
            this.maxHunger = maxHunger;
        }

        /**
         * Returns {@code true} iff every visible field differs from
         * {@code other} by less than {@code epsilon}. Used by the render
         * loop to skip the draw pass when nothing has changed since the
         * last paint.
         */
        public boolean valuesEqualWithin(PlayerStateView other, float epsilon) {
            if (other == null) return false;
            return Math.abs(pleasure - other.pleasure) <= epsilon
                && Math.abs(hunger - other.hunger) <= epsilon
                && Math.abs(hiddenHp - other.hiddenHp) <= epsilon;
        }
    }
}
