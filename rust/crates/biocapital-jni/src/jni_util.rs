//! Small JNI helpers shared by all `dispatch::*` modules.
//!
//! Centralising these here keeps the FFI surface of `lib.rs` thin and the
//! JNI↔Rust conversions uniform across the 9 services.

use jni::objects::{JByteArray, JString};
use jni::sys::jint;
use jni::{errors::Result as JniResult, JNIEnv};

/// Read a `jbyteArray` into a `Vec<u8>`.  Returns `Ok(None)` if the input
/// is a null reference (caller treats as "no request body").
pub fn read_bytes<'a>(
    env: &mut JNIEnv<'a>,
    bytes: &JByteArray<'a>,
) -> JniResult<Option<Vec<u8>>> {
    if bytes.is_null() {
        return Ok(None);
    }
    let v = env.convert_byte_array(bytes)?;
    Ok(Some(v))
}

/// Wrap a `Vec<u8>` into a freshly allocated `jbyteArray` for return to
/// Java.  An empty `Vec` becomes a zero-length array (NOT null) so the
/// Java side can distinguish "empty body" from "service unavailable".
pub fn write_bytes<'a>(env: &JNIEnv<'a>, bytes: Vec<u8>) -> JniResult<JByteArray<'a>> {
    let arr = env.byte_array_from_slice(&bytes)?;
    Ok(arr)
}

/// Convert a JNI `method_id` to a stable enum-like value with a friendly
/// debug name.  Pure helper, no allocations.
pub fn method_name(service: &str, method_id: jint) -> String {
    format!("{service}#{method_id}")
}

/// Read a JString and return it as an owned `String`.  Returns `Ok(None)`
/// if the input is a null reference.
pub fn read_string<'a>(
    env: &mut JNIEnv<'a>,
    s: &JString<'a>,
) -> JniResult<Option<String>> {
    if s.is_null() {
        return Ok(None);
    }
    Ok(Some(env.get_string(s)?.into()))
}
