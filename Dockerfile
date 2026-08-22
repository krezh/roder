# syntax=docker/dockerfile:1

# ---- build ----------------------------------------------------------------
FROM ghcr.io/rust-lang/rust:1.98.0-trixie@sha256:7f7a53a25a0319dd8284e279d529d45759cb384d59b14cc6806132910f45522e AS build

ARG WB_VERSION

# Build tools and the wasm target are installed before source is copied so
# application changes do not invalidate these layers.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    rustup target add wasm32-unknown-unknown
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo install cargo-leptos --locked
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    test -n "${WB_VERSION}" && \
    cargo install wasm-bindgen-cli --version "${WB_VERSION}" --locked

WORKDIR /app
COPY . .

# RELEASE=true  → optimised release binary
# RELEASE=false → debug binary
ARG RELEASE=true
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    export LEPTOS_ENV=PROD LEPTOS_SITE_ROOT=/app/site-out && \
    if [ "$RELEASE" = "true" ]; then \
        cargo leptos build --release --bin-features ssr,jemalloc \
        && cp /app/target/release/roder /app/roder-bin \
        && cp /app/target/release/hash.txt /app/hash.txt; \
    else \
        cargo leptos build --bin-features ssr,jemalloc \
        && cp /app/target/debug/roder /app/roder-bin \
        && cp /app/target/debug/hash.txt /app/hash.txt; \
    fi

# ---- runtime --------------------------------------------------------------
FROM gcr.io/distroless/cc-debian13@sha256:e86cf4f565c8eee2cbb2be073bb107dafb14734b53d5872da20fdf47418a02f4 AS runtime

WORKDIR /app
COPY --from=build /app/roder-bin /app/roder
COPY --from=build /app/hash.txt /app/hash.txt
COPY --from=build /app/site-out /app/site

ENV LEPTOS_SITE_ROOT=/app/site \
    LEPTOS_SITE_PKG_DIR=pkg \
    LEPTOS_SITE_ADDR=0.0.0.0:8080 \
    LEPTOS_HASH_FILES=true \
    RUST_LOG=info \
    # jemalloc tuning (tikv-jemallocator reads the _RJEM_-prefixed var)
    _RJEM_MALLOC_CONF=background_thread:true,dirty_decay_ms:5000,muzzy_decay_ms:5000

EXPOSE 8080 8443
USER 65532
ENTRYPOINT ["/app/roder"]
