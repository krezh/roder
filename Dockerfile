# syntax=docker/dockerfile:1

# ---- build ----------------------------------------------------------------
FROM ghcr.io/rust-lang/rust:1.96.1-trixie@sha256:1f0dbad1df66647807e6952d1db85d0b2bda7606cb2139d82517e4f009967376 AS build

# cargo-leptos + the wasm target.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    rustup target add wasm32-unknown-unknown
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo install cargo-leptos --locked

RUN wget https://github.com/mikefarah/yq/releases/latest/download/yq_linux_amd64 -O /usr/local/bin/yq &&\
    chmod +x /usr/local/bin/yq
WORKDIR /app
COPY . .

# Derive the wasm-bindgen CLI version directly from Cargo.lock so it always
# matches the compiled crate.
# The shim the CLI emits and the wasm the crate produces must be identical
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    WB=$(yq .workspace.dependencies.wasm-bindgen Cargo.toml) \
    && cargo install -f wasm-bindgen-cli --version "$WB"

# RELEASE=true  → optimised release binary
# RELEASE=false → debug binary
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
    # jemalloc tuning (tikv-jemallocator reads the _RJEM_-prefixed var)
    _RJEM_MALLOC_CONF=background_thread:true,dirty_decay_ms:5000,muzzy_decay_ms:5000

EXPOSE 8080
USER 65532
ENTRYPOINT ["/app/roder"]
