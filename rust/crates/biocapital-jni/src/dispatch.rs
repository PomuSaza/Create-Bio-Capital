//! Per-service dispatch tables for the JNI bridge.
//!
//! Each `pub mod` here implements a single `dispatch(env, method_id, bytes)`
//! function that:
//!
//! 1. Decodes the incoming `request_bytes` (a `prost::Message` encoded by
//!    the Java side via the standard `*OuterClass.parseFrom(byte[])`
//!    pathway) into the concrete request type for the requested RPC.
//! 2. Dispatches to the appropriate in-process service (or, when running
//!    in "network gRPC" mode, to the local gRPC client) — see
//!    `doc/14-rust-services.md` §1.3.
//! 3. Encodes the response back into bytes for the JVM.
//!
//! On any error the function returns `Err`, which `lib.rs` translates to a
//! `null` `jbyteArray`.  The Java side (`NativeRustBindings.java`)
//! converts `null` → `Optional.empty()` and degrades gracefully.

use jni::objects::{JByteArray, JClass};
use jni::sys::jint;
use jni::JNIEnv;
use thiserror::Error;
use tracing::warn;

use crate::jni_util::{read_bytes, write_bytes};

#[derive(Debug, Error)]
pub enum DispatchError {
    #[error("JNI error: {0}")]
    Jni(#[from] jni::errors::Error),

    #[error("proto decode error: {0}")]
    Decode(#[from] prost::DecodeError),

    #[error("service not initialised; call NativeRustBindings.init() first")]
    NotInitialised,

    #[error("unknown method_id {0} for service {1}")]
    UnknownMethod(jint, &'static str),
}

/// Common helper: read bytes, run a closure on the parsed request, write
/// the encoded response back as a `jbyteArray`.  The closure is responsible
/// for the actual proto round-trip — this keeps the per-service modules
/// short.
fn run<'a, F>(
    env: &mut JNIEnv<'a>,
    _class: &JClass<'a>,
    request: &JByteArray<'a>,
    service: &'static str,
    f: F,
) -> Option<JByteArray<'a>>
where
    F: FnOnce(Option<Vec<u8>>) -> Result<Vec<u8>, DispatchError>,
{
    let req = match read_bytes(env, request) {
        Ok(b) => b,
        Err(e) => {
            warn!("[biocapital-jni] {service}: failed to read request bytes: {e}");
            return None;
        }
    };
    match f(req) {
        Ok(resp) => match write_bytes(env, resp) {
            Ok(arr) => Some(arr),
            Err(e) => {
                warn!("[biocapital-jni] {service}: failed to write response bytes: {e}");
                None
            }
        },
        Err(e) => {
            warn!("[biocapital-jni] {service} dispatch failed: {e}");
            None
        }
    }
}

// ============================================================================
// PlayerStateService  (doc/14 §3.2; service is in scope of task #3)
// ============================================================================

pub mod player_state {
    use super::*;

    /// `method_id` is one of:
    ///   0 = GetState, 1 = UpdateState, 2 = ApplyDamage,
    ///   3 = AddPleasure, 4 = AddHunger
    pub fn dispatch<'a>(
        env: &mut JNIEnv<'a>,
        _method_id: jint,
        request: &JByteArray<'a>,
    ) -> Option<JByteArray<'a>> {
        run(env, &JClass::default(), request, "PlayerStateService", |req| {
            // TODO(task #3): wire to biocapital_core::player_state module.
            // For now: echo the request bytes back as a no-op.  This lets
            // the Java side be exercised end-to-end (load library → call
            // native method → receive a buffer) without depending on
            // player-state implementation work.
            Ok(req.unwrap_or_default())
        })
    }
}

// ============================================================================
// BankService  (doc/14 §3.2; service is the PRIORITY module of task #4)
// ============================================================================

pub mod bank {
    use super::*;

    /// `method_id` is one of:
    ///   0 = GetBalance, 1 = Deposit, 2 = Withdraw, 3 = Transfer,
    ///   4 = GetHistory, 5 = LockDevice, 6 = UnlockDevice,
    ///   7 = GenerateInviteCode, 8 = AcceptInviteCode
    pub fn dispatch<'a>(
        env: &mut JNIEnv<'a>,
        _method_id: jint,
        request: &JByteArray<'a>,
    ) -> Option<JByteArray<'a>> {
        run(env, &JClass::default(), request, "BankService", |req| {
            // TODO(task #4): wire to biocapital_bank module.
            Ok(req.unwrap_or_default())
        })
    }
}

// ============================================================================
// CorePodService  (doc/14 §3.2; service is task #5)
// ============================================================================

pub mod core_pod {
    use super::*;

    /// `method_id` is one of:
    ///   0 = TickPod, 1 = EnterPod, 2 = ExitPod, 3 = GetPodState
    pub fn dispatch<'a>(
        env: &mut JNIEnv<'a>,
        _method_id: jint,
        request: &JByteArray<'a>,
    ) -> Option<JByteArray<'a>> {
        run(env, &JClass::default(), request, "CorePodService", |req| {
            // TODO(task #5): wire to biocapital_pod module.
            Ok(req.unwrap_or_default())
        })
    }
}
