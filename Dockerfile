# syntax=docker/dockerfile:1

# ---- build ----------------------------------------------------------------
FROM rust:1-bookworm AS build

# cargo-leptos + the wasm target + wasm-opt (binaryen) for the hydrate bundle.
RUN rustup target add wasm32-unknown-unknown \
    && cargo install cargo-leptos --locked \
    && apt-get update && apt-get install -y --no-install-recommends binaryen \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

# Builds the SSR server binary + hashed site assets into target/site.
RUN cd crates/app && cargo leptos build --release

# ---- runtime --------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --user-group --no-create-home --shell /usr/sbin/nologin roder

WORKDIR /app
COPY --from=build /app/target/release/roder /app/roder
COPY --from=build /app/target/site /app/site

ENV LEPTOS_SITE_ROOT=/app/site \
    LEPTOS_SITE_PKG_DIR=pkg \
    LEPTOS_SITE_ADDR=0.0.0.0:8080 \
    RUST_LOG=info

EXPOSE 8080
USER 1000
ENTRYPOINT ["/app/roder"]
