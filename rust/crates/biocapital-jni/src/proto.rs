//! Re-export of workspace proto-generated types for the JNI dispatch layer.
//!
//! The actual `cargo build` step that produces `biocapital.pb.rs` lives in
//! `biocapital-grpc` (see `rust/crates/biocapital-grpc/build.rs`).  We
//! re-export those types here so `dispatch.rs` can name them without
//! taking a direct dependency on `biocapital-grpc` (which would pull in
//! tonic transport and bloat the JNI cdylib).
//!
//! For now this module is a placeholder: the `biocapital-grpc` crate has
//! not yet generated the Rust proto stubs (the proto schema is complete,
//! see `doc/14-rust-services.md` §3.2, but `build.rs` + tonic prost-build
//! wiring is part of task #4 / #5 work).  When those land, this file
//! becomes:
//!
//! ```ignore
//! pub use biocapital_grpc::proto::*;
//! ```

// Stub types so the crate compiles before proto-gen lands.
// Remove this stub and uncomment the re-export above once
// `biocapital-grpc` exposes `pub mod proto`.
pub type _Placeholder = ();
