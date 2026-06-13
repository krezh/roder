# syntax=docker/dockerfile:1

# ---- build ----------------------------------------------------------------
FROM ghcr.io/rust-lang/rust:1.96.0-trixie@sha256:4fd8406017c992f7b8ab55a2f99a1d56aeb1d7ecd255850dfa04239a88601f73 AS build

# cargo-leptos + the wasm target.
# binaryen is not installed from apt: Debian bookworm ships a version that
# corrupts the externref table wasm-bindgen 0.2.92+ uses. cargo-leptos
# downloads the correct wasm-opt version itself at build time.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    rustup target add wasm32-unknown-unknown \
    && cargo install cargo-leptos --locked

WORKDIR /app
COPY . .

# Derive the wasm-bindgen CLI version directly from Cargo.lock so it always
# matches the compiled crate. There is no ARG to keep in sync — when Renovate
# bumps the crate, Cargo.lock changes, this layer is invalidated, and the new
# CLI is installed automatically.
#
# The shim the CLI emits and the wasm the crate produces must be identical or
# hydration throws "failed to grow table" on __wbindgen_init_externref_table.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    WB=$(awk '/^name = "wasm-bindgen"$/{f=1} f && /^version/{gsub(/"/, "", $3); print $3; exit}' Cargo.lock) \
    && cargo install -f wasm-bindgen-cli --version "$WB"

# RELEASE=true  → optimised release binary (CI/production, slow to link)
# RELEASE=false → debug binary (local dev, no LTO, fast incremental builds)
ARG RELEASE=true
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    export LEPTOS_ENV=PROD LEPTOS_SITE_ROOT=/app/site-out && \
    if [ "$RELEASE" = "true" ]; then \
        cargo leptos build --release --bin-features ssr,jemalloc \
        && cp /app/target/release/roder /app/roder-bin; \
    else \
        cargo leptos build --bin-features ssr,jemalloc \
        && cp /app/target/debug/roder /app/roder-bin; \
    fi

# ---- runtime --------------------------------------------------------------
FROM gcr.io/distroless/cc-debian13@sha256:a017e74bd2a12d98342dbecd33d121d2b160415ed777573dc1808969e989d94d AS runtime

WORKDIR /app
COPY --from=build /app/roder-bin /app/roder
COPY --from=build /app/site-out /app/site

ENV LEPTOS_SITE_ROOT=/app/site \
    LEPTOS_SITE_PKG_DIR=pkg \
    LEPTOS_SITE_ADDR=0.0.0.0:8080 \
    RUST_LOG=info \
    # jemalloc tuning (tikv-jemallocator reads the _RJEM_-prefixed var): a
    # background thread purges dirty/muzzy pages back to the OS on a short decay
    # so RSS drops promptly after a watch relist spike instead of staying
    # resident — the behaviour that was tripping the container memory limit.
    _RJEM_MALLOC_CONF=background_thread:true,dirty_decay_ms:5000,muzzy_decay_ms:5000

EXPOSE 8080
USER 65532
ENTRYPOINT ["/app/roder"]
