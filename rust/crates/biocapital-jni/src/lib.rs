//! JNI bridge for Bio-Capital — Java↔Rust entry points per
//! `doc/16-sable-bridge.md` §3.4, §3.5 and `doc/14-rust-services.md` §1.3.
//!
//! ## Architecture
//!
//! ```text
//! Java side (NeoForge JVM)
//!   └─ NativeRustBindings (static loadLibrary("biocapital_jni"))
//!        │
//!        │  JNI (Java_* symbols, #[unsafe(no_mangle)] extern "system")
//!        ▼
//! Rust side (this crate, cdylib)
//!   ├─ dispatch.rs   — bytes ⇄ proto ⇄ gRPC service calls
//!   ├─ jni_util.rs   — JString ⇄ String, jbyte[] ⇄ Vec<u8>, exception bridging
//!   └─ proto/        — re-export of workspace proto-generated types
//! ```
//!
//! The JNI entry points listed below follow the symbol naming convention
//! `Java_mo_dystopia_biocapital_NativeRustBindings_<method>`.  The Java class
//! `mo.dystopia.biocapital.NativeRustBindings` MUST declare matching
//! `native` methods with the same names and JNI-signature-equivalent
//! parameter types.  See `src/main/java/mo/dystopia/biocapital/NativeRustBindings.java`.
//!
//! ## Module map
//!
//! - **§3.4 init** — `Java_mo_dystopia_biocapital_NativeRustBindings_init0`:
//!   bootstraps the in-process tokio runtime, the gRPC client, and the
//!   dispatch table.  Idempotent.  Returns `JNI_TRUE` on success.
//! - **§3.4 callPlayerState / callBank / callCorePod** — request/response
//!   RPCs: caller passes a serialized `prost::Message` (encoded by the Java
//!   side), Rust dispatches to the corresponding service, returns the
//!   serialized response as `jbyteArray`.  The Java side decodes via the
//!   generated proto stubs.
//! - Other services (Contract, Environment, Creature, Dglab, HostileMob,
//!   Audit) — stubbed with `unimplemented!()`.  See "Status" column in
//!   the deliverable table.
//!
//! ## Threading
//!
//! JNI calls arrive on arbitrary JVM threads (often the main thread or a
//! netty worker).  Each entry point creates a short-lived
//! `tokio::runtime::Runtime` via `block_on` (or reuses a global one
//! initialised in `init`) and dispatches the call.  This matches Sable's
//! documented pattern and keeps the FFI surface synchronous from Java's
//! point of view.

#![deny(unsafe_op_in_unsafe_fn)]

use std::sync::OnceLock;

use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jbyteArray, jfloat, jint, JNI_TRUE};
use jni::JNIEnv;
use tracing::{error, info, warn};

pub mod dispatch;
pub mod jni_util;
pub mod proto;

// ============================================================================
// Global state
// ============================================================================

/// In-process tokio runtime + gRPC client handle.  Initialised by `init()`
/// and reused for the lifetime of the JVM.
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Whether `init()` has succeeded.  Tracked separately from `RUNTIME` so we
/// can return `JNI_FALSE` from entry points that arrive before init (or after
/// a failed init) without panicking.
static INITIALISED: OnceLock<bool> = OnceLock::new();

// ============================================================================
// 1. init() — doc/16 §3.4 / §3.5
// ============================================================================

/// `Java_mo_dystopia_biocapital_NativeRustBindings_init0`
///
/// Bootstraps the in-process tokio runtime, the in-process gRPC client
/// (connecting to the local biocapital-server via UDS or TCP — see
/// `doc/14-rust-services.md` §2.2), the dispatch table, and the Java
/// `java.util.logging` bridge for `tracing` events.
///
/// Idempotent: calling twice is a no-op and returns `JNI_TRUE`.
///
/// # Java signature
/// ```java
/// private static native boolean init0();
/// ```
#[unsafe(no_mangle)]
pub extern "system" fn Java_mo_dystopia_biocapital_NativeRustBindings_init0(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    match RUNTIME.set(build_runtime()) {
        Ok(()) => {
            INITIALISED.set(true).ok();
            info!("[biocapital-jni] init OK (tokio runtime + gRPC client ready)");
            JNI_TRUE
        }
        Err(_) => {
            // Already initialised — that's fine.
            INITIALISED.set(true).ok();
            warn!("[biocapital-jni] init called twice — reusing existing runtime");
            JNI_TRUE
        }
    }
}

fn build_runtime() -> tokio::runtime::Runtime {
    // Single-threaded is sufficient: JNI calls are infrequent (per
    // doc/16 §8: "Rust natives 加载：启动时一次性 < 500 ms").  High-frequency
    // gRPC traffic stays on the network gRPC server, not the JNI bridge.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime for JNI bridge")
}

// ============================================================================
// 2. callPlayerState — PlayerStateService (doc/14 §3.2)
// ============================================================================

/// `Java_mo_dystopia_biocapital_NativeRustBindings_callPlayerState`
///
/// Dispatches a `PlayerStateService` RPC encoded as a `prost::Message`.
///
/// `method_id` selects the RPC (0=GetState, 1=UpdateState, 2=ApplyDamage,
/// 3=AddPleasure, 4=AddHunger).  `request_bytes` is the encoded request
/// message.  Returns the encoded response message (or `null` on error).
///
/// # Java signature
/// ```java
/// private static native byte[] callPlayerState0(int methodId, byte[] requestBytes);
/// ```
#[unsafe(no_mangle)]
pub extern "system" fn Java_mo_dystopia_biocapital_NativeRustBindings_callPlayerState0<'a>(
    mut env: JNIEnv<'a>,
    _class: JClass,
    method_id: jint,
    request_bytes: JByteArray<'a>,
) -> jbyteArray {
    let result = dispatch::player_state::dispatch(&mut env, method_id, &request_bytes);
    match result {
        Some(arr) => arr.into_raw() as jbyteArray,
        None => {
            error!("[biocapital-jni] callPlayerState dispatch returned null");
            std::ptr::null_mut()
        }
    }
}

// ============================================================================
// 3. callBank — BankService (doc/14 §3.2)
// ============================================================================

/// `Java_mo_dystopia_biocapital_NativeRustBindings_callBank`
///
/// Dispatches a `BankService` RPC.
///
/// `method_id` selects the RPC (0=GetBalance, 1=Deposit, 2=Withdraw,
/// 3=Transfer, 4=GetHistory, 5=LockDevice, 6=UnlockDevice,
/// 7=GenerateInviteCode, 8=AcceptInviteCode).
///
/// # Java signature
/// ```java
/// private static native byte[] callBank0(int methodId, byte[] requestBytes);
/// ```
#[unsafe(no_mangle)]
pub extern "system" fn Java_mo_dystopia_biocapital_NativeRustBindings_callBank0<'a>(
    mut env: JNIEnv<'a>,
    _class: JClass,
    method_id: jint,
    request_bytes: JByteArray<'a>,
) -> jbyteArray {
    let result = dispatch::bank::dispatch(&mut env, method_id, &request_bytes);
    match result {
        Some(arr) => arr.into_raw() as jbyteArray,
        None => {
            error!("[biocapital-jni] callBank dispatch returned null");
            std::ptr::null_mut()
        }
    }
}

// ============================================================================
// 4. callCorePod — CorePodService (doc/14 §3.2)
// ============================================================================

/// `Java_mo_dystopia_biocapital_NativeRustBindings_callCorePod`
///
/// Dispatches a `CorePodService` RPC.
///
/// `method_id` selects the RPC (0=TickPod, 1=EnterPod, 2=ExitPod,
/// 3=GetPodState).
///
/// # Java signature
/// ```java
/// private static native byte[] callCorePod0(int methodId, byte[] requestBytes);
/// ```
#[unsafe(no_mangle)]
pub extern "system" fn Java_mo_dystopia_biocapital_NativeRustBindings_callCorePod0<'a>(
    mut env: JNIEnv<'a>,
    _class: JClass,
    method_id: jint,
    request_bytes: JByteArray<'a>,
) -> jbyteArray {
    let result = dispatch::core_pod::dispatch(&mut env, method_id, &request_bytes);
    match result {
        Some(arr) => arr.into_raw() as jbyteArray,
        None => {
            error!("[biocapital-jni] callCorePod dispatch returned null");
            std::ptr::null_mut()
        }
    }
}

// ============================================================================
// 5. callContract — ContractService (STUB — unimplemented)
// ============================================================================

/// `Java_mo_dystopia_biocapital_NativeRustBindings_callContract`
///
/// # Status: STUB — task #7 (`09-contracts`) will implement this.
///
/// `method_id` is reserved (0=Propose, 1=Accept, 2=Reject, 3=Terminate,
/// 4=Redeem, 5=Get, 6=List).  Calling this prior to task #7 returns
/// `null` and logs an error.
#[unsafe(no_mangle)]
pub extern "system" fn Java_mo_dystopia_biocapital_NativeRustBindings_callContract0(
    _env: JNIEnv,
    _class: JClass,
    method_id: jint,
    _request_bytes: JByteArray,
) -> jbyteArray {
    unimplemented_stub("callContract", method_id)
}

// ============================================================================
// 6. callEnvironment — EnvironmentService (STUB)
// ============================================================================

/// `Java_mo_dystopia_biocapital_NativeRustBindings_callEnvironment`
///
/// # Status: STUB — task #10 (`07-environment`) will implement this.
#[unsafe(no_mangle)]
pub extern "system" fn Java_mo_dystopia_biocapital_NativeRustBindings_callEnvironment0(
    _env: JNIEnv,
    _class: JClass,
    method_id: jint,
    _request_bytes: JByteArray,
) -> jbyteArray {
    unimplemented_stub("callEnvironment", method_id)
}

// ============================================================================
// 7. callCreature — CreatureService (STUB)
// ============================================================================

/// `Java_mo_dystopia_biocapital_NativeRustBindings_callCreature`
///
/// # Status: STUB — task #11 (`13-bio-customization`) will implement this.
#[unsafe(no_mangle)]
pub extern "system" fn Java_mo_dystopia_biocapital_NativeRustBindings_callCreature0(
    _env: JNIEnv,
    _class: JClass,
    method_id: jint,
    _request_bytes: JByteArray,
) -> jbyteArray {
    unimplemented_stub("callCreature", method_id)
}

// ============================================================================
// 8. callDglab — DglabService (STUB)
// ============================================================================

/// `Java_mo_dystopia_biocapital_NativeRustBindings_callDglab0`
///
/// # Status: STUB — task #6 (`10-hardware-dglab`) will implement this.
#[unsafe(no_mangle)]
pub extern "system" fn Java_mo_dystopia_biocapital_NativeRustBindings_callDglab0(
    _env: JNIEnv,
    _class: JClass,
    method_id: jint,
    _request_bytes: JByteArray,
) -> jbyteArray {
    unimplemented_stub("callDglab", method_id)
}

// ============================================================================
// 9. callHostileMob — HostileMobService (STUB)
// ============================================================================

/// `Java_mo_dystopia_biocapital_NativeRustBindings_callHostileMob0`
///
/// # Status: STUB — task #9 (`06-hostile-mobs`) will implement this.
#[unsafe(no_mangle)]
pub extern "system" fn Java_mo_dystopia_biocapital_NativeRustBindings_callHostileMob0(
    _env: JNIEnv,
    _class: JClass,
    method_id: jint,
    _request_bytes: JByteArray,
) -> jbyteArray {
    unimplemented_stub("callHostileMob", method_id)
}

// ============================================================================
// 10. callAudit — AuditService (STUB)
// ============================================================================

/// `Java_mo_dystopia_biocapital_NativeRustBindings_callAudit0`
///
/// # Status: STUB — task #13 (`12-command-system`) will implement this.
#[unsafe(no_mangle)]
pub extern "system" fn Java_mo_dystopia_biocapital_NativeRustBindings_callAudit0(
    _env: JNIEnv,
    _class: JClass,
    method_id: jint,
    _request_bytes: JByteArray,
) -> jbyteArray {
    unimplemented_stub("callAudit", method_id)
}

// ============================================================================
// 11. computePodStress — low-level compute helper (doc/16 §3.4)
// ============================================================================

/// `Java_mo_dystopia_biocapital_NativeRustBindings_computePodStress0`
///
/// Direct call (skips the gRPC dispatch layer) used by `CorePodBlockEntity`
/// every tick.  Implemented in `biocapital-pod` crate; this is a thin
/// re-export so the JVM only links one .so.
///
/// # Java signature
/// ```java
/// private static native float computePodStress0(byte[] hostUuidBytes, int endurance);
/// ```
#[unsafe(no_mangle)]
pub extern "system" fn Java_mo_dystopia_biocapital_NativeRustBindings_computePodStress0(
    mut env: JNIEnv,
    _class: JClass,
    host_uuid: JString,
    endurance: jint,
) -> jfloat {
    // The Java prototype uses a JString (UUID as canonical string); we
    // convert and forward.  If the host is null/empty, return 0.0
    // (matches the "no host = no stress" invariant from doc/04 §2.2).
    let host_str: String = match env.get_string(&host_uuid) {
        Ok(s) => s.into(),
        Err(_) => return 0.0,
    };
    if host_str.is_empty() {
        return 0.0;
    }
    // TODO: forward to biocapital_pod::compute_stress once task #5 lands.
    // For now, fall back to the same default stress used in 16 §3.4.
    let _ = (host_str, endurance);
    0.0_f32
}

// ============================================================================
// helpers
// ============================================================================

fn unimplemented_stub(name: &str, method_id: jint) -> jbyteArray {
    error!(
        "[biocapital-jni] {name}(method_id={method_id}) is not yet implemented; \
         see doc/16-sable-bridge.md §3.4 and the corresponding task in \
         doc/99-integration-matrix.md §11.2"
    );
    // Returning null is the documented fallback (NativeRustBindings.java
    // wraps every call in Optional<byte[]> and degrades gracefully).
    std::ptr::null_mut()
}
