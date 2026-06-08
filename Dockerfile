# syntax=docker/dockerfile:1

# ---- build ----------------------------------------------------------------
FROM rust:1-bookworm AS build

# cargo-leptos + the wasm target + wasm-opt (binaryen) for the hydrate bundle.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    rustup target add wasm32-unknown-unknown \
    && cargo install cargo-leptos --locked \
    && apt-get update && apt-get install -y --no-install-recommends binaryen \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

# Install the matching wasm-bindgen-cli. cargo-leptos shells out to it during
# the hydrate build; if the CLI version doesn't match the crate version, the
# JS shim and wasm disagree on the externref table layout and hydration
# throws "failed to grow table" on __wbindgen_init_externref_table.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    WB_VERSION=$(grep -E '^wasm-bindgen[[:space:]]*=[[:space:]]*"=' Cargo.toml \
        | sed -E 's/.*"=([0-9.]+)".*/\1/') \
    && cargo install -f wasm-bindgen-cli --version "${WB_VERSION}"

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
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
