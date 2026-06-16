package mo.dystopia.biocapital;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.Optional;

/**
 * JNI bridge to the native (Rust) implementation of Bio-Capital's server-side
 * logic.  Mirrors {@code doc/16-sable-bridge.md} §3.5 and the JNI entry
 * points in {@code rust/crates/biocapital-jni/src/lib.rs}.
 *
 * <h2>Loading strategy</h2>
 * <ul>
 *   <li>The native library is named {@code biocapital_jni} on every platform
 *       (Linux: {@code libbiocapital_jni.so}; macOS: {@code .dylib};
 *       Windows: {@code .dll}).  JVM resolves the platform suffix.</li>
 *   <li>Loading is best-effort: a missing library (e.g. a developer running
 *       the game without a built {@code build/natives/<os>/} directory)
 *       leaves {@link #NATIVE_AVAILABLE} {@code false} and every public
 *       method returns {@link Optional#empty()}.  The mod never crashes
 *       on a missing native — degraded mode is by design per
 *       {@code doc/16-sable-bridge.md} §10 (acceptance criteria).</li>
 * </ul>
 *
 * <h2>Threading</h2>
 * All methods are safe to call from any thread.  Each call dispatches a
 * synchronous request to the Rust side via the in-process tokio runtime
 * (initialised by {@link #init()}).
 *
 * <h2>Stubs</h2>
 * Methods whose backing service is not yet implemented (see
 * {@code doc/99-integration-matrix.md} §11.2) always return
 * {@link Optional#empty()}.  Once the corresponding service lands, the
 * Rust side removes the stub and the Java side begins receiving
 * non-empty responses automatically — no Java change required.
 */
public final class NativeRustBindings {

    private static final Logger LOGGER = LoggerFactory.getLogger(NativeRustBindings.class);
    private static final String LIB_NAME = "biocapital_jni";

    /** Set to {@code true} iff {@link #init()} successfully loaded the .so/.dylib/.dll. */
    public static final boolean NATIVE_AVAILABLE;

    static {
        boolean ok;
        try {
            System.loadLibrary(LIB_NAME);
            ok = true;
            LOGGER.info("[BioCapital] Loaded native library '{}' (JNI bridge ready)", LIB_NAME);
        } catch (UnsatisfiedLinkError | SecurityException e) {
            ok = false;
            LOGGER.warn(
                    "[BioCapital] Native library '{}' not loaded ({}). " +
                            "Falling back to pure-Java / pure-Rust-gRPC paths; " +
                            "see doc/16-sable-bridge.md §10 for the build procedure.",
                    LIB_NAME, e.getMessage());
        }
        NATIVE_AVAILABLE = ok;
    }

    private NativeRustBindings() {
        // utility class — no instances
    }

    // ------------------------------------------------------------------
    // Lifecycle
    // ------------------------------------------------------------------

    /**
     * Bootstraps the native side: tokio runtime, in-process gRPC client,
     * dispatch table.  Idempotent.  Returns {@code true} on success,
     * {@code false} if the library was never loaded.
     *
     * <p>Called once from {@link BioCapital#BioCapital} (the mod
     * constructor).  May also be called from tests or from a server-only
     * entrypoint.
     */
    public static boolean init() {
        if (!NATIVE_AVAILABLE) {
            return false;
        }
        return init0();
    }

    private static native boolean init0();

    // ------------------------------------------------------------------
    // Per-service dispatch (doc/16 §3.5 + rust lib.rs)
    // ------------------------------------------------------------------

    /**
     * PlayerStateService dispatch.  See {@code PlayerStateService} in
     * {@code doc/14-rust-services.md} §3.2.
     *
     * <p>Hot path consumers (task #123, 2026-06-14):
     * <ul>
     *   <li>{@code methodId == 0 (GetState)} — called by
     *       {@code mo.dystopia.biocapital.hud.BioCapitalHud} every
     *       200 ms (5 Hz) per online player. The Rust side serves these
     *       reads from an in-memory 200 ms cache
     *       ({@code biocapital_core::PlayerStateCache}) so steady-state
     *       PostgreSQL load is constant at ~5 read/s/player regardless
     *       of the online count. See
     *       {@code doc/CHANGELOG.md} "[02-player-state] - 2026-06-14 晚".</li>
     *   <li>{@code methodId ∈ {1, 2, 3, 4, 5}} — write paths; the
     *       call site must follow up with a {@code GetState} refresh
     *       for the HUD if the result is not already broadcast.</li>
     * </ul>
     *
     * @param methodId     0=GetState, 1=UpdateState, 2=ApplyDamage,
     *                     3=AddPleasure, 4=AddHunger, 5=AddFluidEffect
     *                     (see {@code rust/proto/biocapital.proto}
     *                     {@code PlayerStateService})
     * @param requestBytes protobuf-encoded request message
     * @return protobuf-encoded response, or empty if native unavailable
     *         or the call failed
     */
    public static Optional<byte[]> callPlayerState(int methodId, byte[] requestBytes) {
        if (!NATIVE_AVAILABLE) return Optional.empty();
        try {
            return Optional.ofNullable(callPlayerState0(methodId, requestBytes));
        } catch (UnsatisfiedLinkError e) {
            LOGGER.warn("callPlayerState unimplemented on native side: {}", e.getMessage());
            return Optional.empty();
        }
    }

    private static native byte[] callPlayerState0(int methodId, byte[] requestBytes);

    /**
     * BankService dispatch.  See {@code BankService} in
     * {@code doc/14-rust-services.md} §3.2.
     *
     * @param methodId     0=GetBalance, 1=Deposit, 2=Withdraw, 3=Transfer,
     *                     4=GetHistory, 5=LockDevice, 6=UnlockDevice,
     *                     7=GenerateInviteCode, 8=AcceptInviteCode,
     *                     9=RequestHardwareToken, 10=BindHardware,
     *                     11=ListHardware, 12=RevokeHardware,
     *                     13=Authenticate (doc/18 §7, task #83)
     */
    public static Optional<byte[]> callBank(int methodId, byte[] requestBytes) {
        if (!NATIVE_AVAILABLE) return Optional.empty();
        try {
            return Optional.ofNullable(callBank0(methodId, requestBytes));
        } catch (UnsatisfiedLinkError e) {
            LOGGER.warn("callBank unimplemented on native side: {}", e.getMessage());
            return Optional.empty();
        }
    }

    private static native byte[] callBank0(int methodId, byte[] requestBytes);

    /**
     * CorePodService dispatch.  See {@code CorePodService} in
     * {@code doc/14-rust-services.md} §3.2.
     *
     * @param methodId     0=TickPod, 1=EnterPod, 2=ExitPod, 3=GetPodState
     */
    public static Optional<byte[]> callCorePod(int methodId, byte[] requestBytes) {
        if (!NATIVE_AVAILABLE) return Optional.empty();
        try {
            return Optional.ofNullable(callCorePod0(methodId, requestBytes));
        } catch (UnsatisfiedLinkError e) {
            LOGGER.warn("callCorePod unimplemented on native side: {}", e.getMessage());
            return Optional.empty();
        }
    }

    private static native byte[] callCorePod0(int methodId, byte[] requestBytes);

    /**
     * ContractService dispatch.  STUB until task #7 lands.
     *
     * @param methodId     0=Propose, 1=Accept, 2=Reject, 3=Terminate,
     *                     4=Redeem, 5=Get, 6=List
     */
    public static Optional<byte[]> callContract(int methodId, byte[] requestBytes) {
        if (!NATIVE_AVAILABLE) return Optional.empty();
        try {
            return Optional.ofNullable(callContract0(methodId, requestBytes));
        } catch (UnsatisfiedLinkError e) {
            LOGGER.debug("callContract stub: {}", e.getMessage());
            return Optional.empty();
        }
    }

    private static native byte[] callContract0(int methodId, byte[] requestBytes);

    /** EnvironmentService dispatch.  STUB until task #10 lands. */
    public static Optional<byte[]> callEnvironment(int methodId, byte[] requestBytes) {
        if (!NATIVE_AVAILABLE) return Optional.empty();
        try {
            return Optional.ofNullable(callEnvironment0(methodId, requestBytes));
        } catch (UnsatisfiedLinkError e) {
            return Optional.empty();
        }
    }

    private static native byte[] callEnvironment0(int methodId, byte[] requestBytes);

    /** CreatureService dispatch.  STUB until task #11 lands. */
    public static Optional<byte[]> callCreature(int methodId, byte[] requestBytes) {
        if (!NATIVE_AVAILABLE) return Optional.empty();
        try {
            return Optional.ofNullable(callCreature0(methodId, requestBytes));
        } catch (UnsatisfiedLinkError e) {
            return Optional.empty();
        }
    }

    private static native byte[] callCreature0(int methodId, byte[] requestBytes);

    /**
     * DglabService dispatch.  2026-06-14 task #6: DglabService 5
     * RPCs (SetStrength / GetStrength / GenerateToken / RevokeToken
     * / ListTokens) are implemented in
     * {@code rust/crates/biocapital-grpc/src/dglab_service.rs}.
     * The actual DG_LAB hardware traffic is a Rust-side WebSocket
     * client ({@code rust/crates/biocapital-dglab/src/ws_client.rs},
     * {@code tokio-tungstenite}); Java does not see the hardware
     * per {@code doc/10-hardware-dglab.md §1.2}.  This class is
     * a generic proto-byte JNI dispatch; the
     * {@link #callDglab0} native resolves to
     * {@code biocapital_jni::dglab_dispatch} (16-sable-bridge §3.5).
     */
    public static Optional<byte[]> callDglab(int methodId, byte[] requestBytes) {
        if (!NATIVE_AVAILABLE) return Optional.empty();
        try {
            return Optional.ofNullable(callDglab0(methodId, requestBytes));
        } catch (UnsatisfiedLinkError e) {
            return Optional.empty();
        }
    }

    private static native byte[] callDglab0(int methodId, byte[] requestBytes);

    /**
     * HostileMobService dispatch (task #9, 2026-06-14).
     *
     * <p>Two method_ids, mirroring the gRPC contract in
     * {@code rust/proto/biocapital.proto::HostileMobService} and
     * implemented in
     * {@code rust/crates/biocapital-grpc/src/hostile_mob_service.rs}:
     * <ul>
     *   <li>{@code methodId == 0} → {@code ApplyHostileDamage(DamageRequest) -> DamageResponse}.
     *       Carries the standard {@code DamageRequest} payload; the
     *       Rust side delegates to {@code PlayerStateService.ApplyDamage}
     *       so the audit / hidden-HP / clamp logic lives in exactly
     *       one place. The Java caller should pre-fill
     *       {@code source ∈ {"zombie","skeleton","creeper","mob"}}
     *       so the audit row gets {@code actor_type = "HOSTILE_MOB"}.</li>
     *   <li>{@code methodId == 1} → {@code GetDropChance(CreatureIdRequest) -> DropChanceResponse}.
     *       Returns the highest-priority enabled
     *       {@code mob_replacements.drop_chance_desire_fragment} for
     *       the given creature id (vanilla or variant form). When no
     *       row matches, returns {@code enabled = false} and the
     *       default 3 % probability.</li>
     * </ul>
     *
     * <p>Returns {@code Optional.empty()} on a missing native
     * library; the Java side falls back to the in-process
     * hard-coded constants in {@code Rust 端 HostileMobService}
     * for drop chance / spawn routing.
     *
     * <p>2026-06-14 task #119 cleanup: 删除了原 Javadoc 引用的
     * {@code MobDropsHandler} / {@code HostileMobHandler}（已删文件）。
     * 业务下沉 Rust 端 gRPC。
     */
    public static Optional<byte[]> callHostileMob(int methodId, byte[] requestBytes) {
        if (!NATIVE_AVAILABLE) return Optional.empty();
        try {
            return Optional.ofNullable(callHostileMob0(methodId, requestBytes));
        } catch (UnsatisfiedLinkError e) {
            return Optional.empty();
        }
    }

    private static native byte[] callHostileMob0(int methodId, byte[] requestBytes);

    /** AuditService dispatch.  STUB until task #13 lands. */
    public static Optional<byte[]> callAudit(int methodId, byte[] requestBytes) {
        if (!NATIVE_AVAILABLE) return Optional.empty();
        try {
            return Optional.ofNullable(callAudit0(methodId, requestBytes));
        } catch (UnsatisfiedLinkError e) {
            return Optional.empty();
        }
    }

    private static native byte[] callAudit0(int methodId, byte[] requestBytes);

    // ------------------------------------------------------------------
    // Direct compute helpers (doc/16 §3.4)
    // ------------------------------------------------------------------

    /**
     * Compute the per-tick stress units for a core-pod hosting a given
     * player.  Called every tick by {@code CorePodBlockEntity} (per
     * {@code doc/16-sable-bridge.md} §9).  Bypasses the gRPC dispatch
     * layer for low latency.
     *
     * @param hostUuidAsString canonical 36-char UUID string of the host player
     * @param endurance       current endurance ticks (used for stress decay)
     * @return stress units (SU); 0.0 if host is null/empty or native unavailable
     */
    public static float computePodStress(String hostUuidAsString, int endurance) {
        if (!NATIVE_AVAILABLE) return 0.0f;
        if (hostUuidAsString == null || hostUuidAsString.isEmpty()) return 0.0f;
        try {
            return computePodStress0(hostUuidAsString, endurance);
        } catch (UnsatisfiedLinkError e) {
            return 0.0f;
        }
    }

    private static native float computePodStress0(String hostUuidAsString, int endurance);
}
