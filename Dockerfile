# syntax=docker/dockerfile:1

# ---- build ----------------------------------------------------------------
FROM rust:1-bookworm AS build

# Pin the wasm-bindgen CLI to the exact crate version. The shim the CLI emits
# and the wasm the crate produces must come from the same version, or
# hydration throws "failed to grow table" on __wbindgen_init_externref_table.
# The arg is referenced in the cargo leptos build RUN below so buildkit's
# cache key for that step changes when WB_VERSION changes, which evicts the
# stale wasm artifacts that would otherwise survive in the GHA buildx cache.
ARG WB_VERSION=0.2.122

# cargo-leptos + the wasm target + wasm-opt (binaryen) for the hydrate bundle.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    rustup target add wasm32-unknown-unknown \
    && cargo install cargo-leptos --locked \
    && apt-get update && apt-get install -y --no-install-recommends binaryen \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

# Install the matching wasm-bindgen-cli.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo install -f wasm-bindgen-cli --version "${WB_VERSION}"

# WB_VERSION is referenced in the cache key for this RUN, so bumping the crate
# pin in Cargo.toml (and the default above) invalidates the GHA-cached
# /app/target/ and forces the wasm to be re-linked against a matching shim.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    WB_VERSION=${WB_VERSION} \
    LEPTOS_SITE_ROOT=/app/site-out \
    cargo leptos build --release \
 && cp /app/target/release/roder /app/roder-bin

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
