# syntax=docker/dockerfile:1

# ---- build ----------------------------------------------------------------
FROM rust:1-bookworm AS build

# cargo-leptos + the wasm target.
# binaryen (wasm-opt) is intentionally omitted: Debian bookworm ships an old
# version that corrupts the externref table wasm-bindgen 0.2.92+ uses, causing
# "failed to grow table" at runtime. The Rust opt-level="z" profile already
# produces well-optimized wasm; cargo-leptos skips wasm-opt when it's absent.
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
        cargo leptos build --release \
        && cp /app/target/release/roder /app/roder-bin; \
    else \
        cargo leptos build \
        && cp /app/target/debug/roder /app/roder-bin; \
    fi

# ---- runtime --------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --user-group --no-create-home --shell /usr/sbin/nologin roder

WORKDIR /app
COPY --from=build /app/roder-bin /app/roder
COPY --from=build /app/site-out /app/site

ENV LEPTOS_SITE_ROOT=/app/site \
    LEPTOS_SITE_PKG_DIR=pkg \
    LEPTOS_SITE_ADDR=0.0.0.0:8080 \
    RUST_LOG=info

EXPOSE 8080
USER 1000
ENTRYPOINT ["/app/roder"]
