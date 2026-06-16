# Create: Bio-Capital — Rust Server

This is the **Rust server-side** of the Create: Bio-Capital mod.
The Java (NeoForge) client launches the `biocapital-cli` binary as a subprocess
and talks to it over gRPC + JNI.

## Layout

| Path | Purpose |
|---|---|
| `Cargo.toml` | Workspace root (12 members) |
| `crates/biocapital-*` | 12 library/binary crates |
| `proto/biocapital.proto` | Shared protobuf schema (`biocapital.v1`) |
| `migrations/` | `sqlx` PostgreSQL migrations |
| `docker/` | Build image (shared with Sable, see `16-sable-bridge`) |

See `doc/14-rust-services.md` for the canonical design and
`doc/99-integration-matrix.md` §4 for the gRPC service-to-crate map.

## Build

```bash
cd rust
cargo build                 # build everything
cargo test                  # run unit + integration tests
cargo build -p biocapital-cli   # build the server binary only
```

The Java Gradle build (`gradlew buildRustNatives`) wraps these commands inside
the shared `docker/` image, so a local `cargo build` is not required for normal
mod development.

## Crate map (12)

| Crate | Module | Role |
|---|---|---|
| `biocapital-core` | 02/03/05/06 | Core data structures (PlayerState, BodyPart, etc.) |
| `biocapital-bank` | 08 | Bank ledger logic (PRIORITY) |
| `biocapital-contract` | 09 | Slave-contract logic |
| `biocapital-pod` | 04 | Core-pod production formulas |
| `biocapital-environment` | 07 | Environment effects |
| `biocapital-creature` | 13 | Bio customization |
| `biocapital-dglab` | 10 | DG_LAB integration (direct, no Java hop) |
| `biocapital-webui` | 15 | Web UI HTTP API (axum) |
| `biocapital-grpc` | 14 | gRPC server + client (tonic) |
| `biocapital-pg` | 14 | PostgreSQL schema + migrations |
| `biocapital-jni` | 16 | JNI bridge (Java entry point) |
| `biocapital-cli` | 14 | CLI entry (start / migrate / backup) |

## Sable integration

- See `doc/16-sable-bridge.md` for the JNI contract
- Build image lives in `rust/docker/` and is shared with Sable's own build

## Status

`0.1.0-alpha.1` — workspace skeleton only.
Each crate is a placeholder (`pub fn placeholder() {}`); per-crate
implementation is tracked in the corresponding `doc/NN-*.md` module.
