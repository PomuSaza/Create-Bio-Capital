# =============================================================================
# Bio-Capital Rust native build image
# =============================================================================
#
# Per doc/16-sable-bridge.md §3.3: shared build image for Sable-style JNI
# cross-compilation.  Sourceable from Sable project as well (compatible
# toolchain, identical install steps).
#
# Base image: rust:1.96-slim (matches rust/Cargo.toml
# `rust-version = "1.96"` and edition = "2024"; rust 1.78 lacks the
# `edition2024` feature and would refuse to parse the workspace
# manifest). Pinned to the same version as the host toolchain so
# `cargo --version` and `rustc --version` agree across local
# builds and CI.
#
# Inputs (mounted at /build):
#   - rust/                  (Cargo workspace)
#   - build/                 (Gradle build output root)
#
# Outputs (per-target):
#   - target/<triple>/release/libbiocapital_jni.so
#   - target/<triple>/release/libbiocapital_jni.dylib
#   - target/<triple>/release/biocapital_jni.dll
#
# Supported target triples (set via --build-arg RUST_TARGET=...):
#   - x86_64-unknown-linux-gnu
#   - x86_64-pc-windows-gnu
#   - x86_64-apple-darwin
#   - aarch64-apple-darwin
#   - aarch64-unknown-linux-gnu
# =============================================================================

FROM rust:1.96-slim

# Disable interactive prompts (Debian frontend).
ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# ----------------------------------------------------------------------------
# 1. System toolchain required by:
#    - jni crate (links against libjvm.so at runtime, but we only need JDK
#      headers at compile time for the `jni` Rust crate)
#    - tonic / prost (no system C deps strictly required, but cmake + clang
#      are needed by sqlx + rapier downstream)
#    - openssl-sys (libssl-dev + pkg-config)
# ----------------------------------------------------------------------------
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        git \
        build-essential \
        clang \
        cmake \
        pkg-config \
        libssl-dev \
        libudev-dev \
        zlib1g-dev \
        gcc-mingw-w64-x86-64 \
        gcc-aarch64-linux-gnu \
        # Cross libc headers — required by `ring` (rustls dep) and
        # similar C-extension crates that need a full glibc during
        # cross-compile. Without these, `bits/libc-header-start.h`
        # is missing and the build fails for aarch64-unknown-linux-gnu.
        libc6-dev-arm64-cross \
    && rm -rf /var/lib/apt/lists/*

# Java headers for `jni` crate (JDK 21 matches the project's
# Minecraft 1.21.1 toolchain — see build.gradle `JavaLanguageVersion.of(21)`).
# 2026-06-14 user decision: JDK 21 统一（升级自 JDK 17）
# 从官方镜像复制 JDK 21 目录，并配置依赖路径
COPY --from=eclipse-temurin:21-jdk /opt/java/openjdk /opt/java/openjdk
ENV JAVA_HOME=/opt/java/openjdk
ENV PATH=$JAVA_HOME/bin:$PATH

# ----------------------------------------------------------------------------
# 2. Rust target matrix.
#    `rust:1.78-slim` ships with the host target already installed; we add the
#    additional targets required by the cross-compile matrix in
#    doc/16-sable-bridge.md §3.3 and §7.1.
# ----------------------------------------------------------------------------
RUN rustup target add \
        x86_64-unknown-linux-gnu \
        x86_64-pc-windows-gnu \
        x86_64-apple-darwin \
        aarch64-apple-darwin \
        aarch64-unknown-linux-gnu

# ----------------------------------------------------------------------------
# 3. Pre-warm the registry index (tiny optimization for CI caching).
# ----------------------------------------------------------------------------
RUN mkdir -p /usr/local/cargo/registry \
    && cargo --version \
    && rustc --version

# ----------------------------------------------------------------------------
# 4. Working directory layout (matches doc/16 §3.2).
#    - /build        : mount point for rust/ + build/
#    - /build/rust   : Cargo workspace
# ----------------------------------------------------------------------------
WORKDIR /build

# Default entrypoint: build for the host target.  Override at `docker run`:
#   docker run --rm -v $PWD/..:/build <image> \
#       bash -c "cargo build --release --target x86_64-pc-windows-gnu -p biocapital-jni"
ENTRYPOINT ["cargo"]
CMD ["build", "--release", "-p", "biocapital-jni"]
