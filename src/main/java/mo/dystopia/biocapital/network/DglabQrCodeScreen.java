package mo.dystopia.biocapital.network;

import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.client.gui.GuiGraphics;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.network.chat.Component;

/**
 * Client-side screen for the DG_LAB hardware pairing flow
 * (doc/10-hardware-dglab.md §1.3 + §3.4).
 *
 * <p>The Rust {@code biocapital-dglab} WebSocket server is the
 * authoritative source of the connection string
 * {@code ws://<host>:<port>/<sessionId>}; this screen fetches
 * the parameters via {@link NativeRustBindings#callDglab} and
 * renders the QR code for the player to scan with the DG_LAB
 * phone app.
 *
 * <p>Per doc/10 §11:
 * <ul>
 *   <li>This screen is a <b>display-only</b> sink; it does not
 *       speak the DG_LAB WebSocket protocol. All WS traffic
 *       stays Rust-to-Android.</li>
 *   <li>The QR is generated at <b>runtime</b> by
 *       {@link #renderQr}; the project does not ship a
 *       pre-rendered QR asset.</li>
 * </ul>
 *
 * <p>Anti-cheat: §7.1 single-connection invariant is enforced
 * Rust-side ({@code DglabWsServer.connected: Mutex<Option<...>>}),
 * not here.
 */
public class DglabQrCodeScreen extends Screen {

    /** Server-issued parameters, populated on open. */
    private final String host;
    private final int port;
    private final String sessionId;

    /**
     * Pixel size of one QR module. 8px makes a Version 3 QR
     * (29x29 modules) fit a 232x232-pixel panel.
     */
    private static final int MODULE_PX = 8;

    /**
     * Construct a screen with a pre-fetched connection string.
     * Callers should source the parameters from
     * {@code NativeRustBindings.callDglab(method_id = 1, ...)}
     * (a future addition to {@code NativeRustBindings} pending
     * task #14 follow-up — see 反问 §1 in the task #96 CHANGELOG
     * entry).
     */
    public DglabQrCodeScreen(String host, int port, String sessionId) {
        super(Component.literal("DG_LAB Connection"));
        this.host = host;
        this.port = port;
        this.sessionId = sessionId;
    }

    /** Compose the full {@code ws://...} URL the QR encodes. */
    public String qrPayload() {
        return "ws://" + host + ":" + port + "/" + sessionId;
    }

    @Override
    protected void init() {
        super.init();
        // No interactive widgets — this screen is informational.
    }

    @Override
    public void render(GuiGraphics gg, int mouseX, int mouseY, float partialTicks) {
        // 1. dim background
        this.renderBackground(gg, mouseX, mouseY, partialTicks);

        int cx = this.width / 2;
        int top = 40;

        // 2. title
        gg.drawCenteredString(
            this.font,
            "Scan with DG_LAB app",
            cx,
            top,
            0xFFFFFFFF
        );

        // 3. connection details
        String url = qrPayload();
        gg.drawCenteredString(
            this.font,
            url,
            cx,
            top + 16,
            0xFFCCCCCC
        );
        gg.drawCenteredString(
            this.font,
            "sessionId: " + sessionId,
            cx,
            top + 30,
            0xFFAAAAAA
        );

        // 4. QR panel
        int[][] matrix = buildQrMatrix(url);
        int size = matrix.length;
        int panel = size * MODULE_PX;
        int px = cx - panel / 2;
        int py = top + 50;
        // White quiet zone
        gg.fill(px - 12, py - 12, px + panel + 12, py + panel + 12, 0xFFFFFFFF);
        // Black modules
        for (int y = 0; y < size; y++) {
            for (int x = 0; x < size; x++) {
                if (matrix[y][x] == 1) {
                    gg.fill(
                        px + x * MODULE_PX,
                        py + y * MODULE_PX,
                        px + (x + 1) * MODULE_PX,
                        py + (y + 1) * MODULE_PX,
                        0xFF000000
                    );
                }
            }
        }

        // 5. footer hint
        gg.drawCenteredString(
            this.font,
            "ESC to close",
            cx,
            py + panel + 24,
            0xFF888888
        );
    }

    /**
     * Build a 1-bit matrix from the URL. This is a deliberately
     * minimal stub — see 反问 §2 in the task #96 CHANGELOG.
     * Real deployments should swap in a real QR encoder
     * (e.g. the well-known {@code qrcode-gen} library) at the
     * point the DG_LAB integration is wired into the build
     * (task #14 follow-up).
     */
    private static int[][] buildQrMatrix(String payload) {
        // Deterministic, well-bounded pseudo-matrix: render the
        // string's hash bits so the screen displays SOMETHING
        // and the rest of the world (tests, screenshots) can
        // visually tell the panel apart from a blank rectangle.
        // A real encoder must replace this before any production
        // pairing (see 反问 §2).
        int size = 29; // QR Version 3
        int[][] m = new int[size][size];
        long h = 1469598103934665603L;
        for (int i = 0; i < payload.length(); i++) {
            h ^= payload.charAt(i);
            h *= 1099511628211L;
        }
        // Position-detection patterns (top-left, top-right, bottom-left).
        drawFinder(m, 0, 0);
        drawFinder(m, size - 7, 0);
        drawFinder(m, 0, size - 7);
        // Data fill: SHA-3-style mix.
        for (int y = 0; y < size; y++) {
            for (int x = 0; x < size; x++) {
                if (isInFinder(x, y, 0, 0)
                    || isInFinder(x, y, size - 7, 0)
                    || isInFinder(x, y, 0, size - 7)) {
                    continue;
                }
                long mix = h ^ ((long) x * 31 + (long) y * 131);
                m[y][x] = ((mix >> ((x + y) & 63)) & 1L) == 1L ? 1 : 0;
            }
        }
        return m;
    }

    private static void drawFinder(int[][] m, int ox, int oy) {
        for (int y = 0; y < 7; y++) {
            for (int x = 0; x < 7; x++) {
                boolean on = (x == 0 || x == 6 || y == 0 || y == 6)
                    || (x >= 2 && x <= 4 && y >= 2 && y <= 4);
                m[oy + y][ox + x] = on ? 1 : 0;
            }
        }
    }

    private static boolean isInFinder(int x, int y, int ox, int oy) {
        return x >= ox && x < ox + 8 && y >= oy && y < oy + 8;
    }

    @Override
    public boolean isPauseScreen() {
        return false;
    }

    // PoseStack import retained to mirror the conventional
    // Mojang render signature should future QR encoder work
    // need direct buffer access.
    @SuppressWarnings("unused")
    private static void touchPoseStack(PoseStack ps) {
        // no-op
    }
}
