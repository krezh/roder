# roder task runner. Run `just` to list recipes.
# The Leptos package is the workspace root, so cargo-leptos recipes run from here.

# Primary LAN IP — the default host the HTTPS proxy serves on.
lan_ip := `ip -4 route get 1.1.1.1 2>/dev/null | grep -oP 'src \K[\d.]+' | head -1`

_default:
    @just --list

# Hot-reloading dev server (SSR + wasm hydrate) at http://127.0.0.1:8080.
# RODER_DEV_MODE bypasses OIDC and uses your current kubeconfig (e.g. `just kind-up`),
# so local testing needs no IdP. Use `just dev-oidc` to exercise the real login flow.
dev:
    RODER_DEV_MODE=1 cargo leptos watch

# Like `dev`, but with real OIDC (reads OIDC_* / BASE_URL from the environment).
dev-oidc:
    cargo leptos watch

# Dev server + HTTPS proxy together, for phones/remote devices that block
# WebAssembly on insecure (http://LAN-IP) origins. Open https://<lan-ip>:8443.
# Uses a trusted mkcert cert if `just dev-certs` was run, else Caddy's internal CA.
# `auto_https disable_redirects` keeps Caddy off ports 80/443 — only 8443 is bound.
dev-https host=lan_ip:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p .certs
    if test -f .certs/roder.pem; then tls="tls .certs/roder.pem .certs/roder-key.pem"; else tls="tls internal"; fi
    printf '{\n  auto_https disable_redirects\n}\nhttps://%s:8443 {\n  %s\n  reverse_proxy 127.0.0.1:8080\n}\n' "{{ host }}" "$tls" > .certs/Caddyfile
    echo "roder over HTTPS:  https://{{ host }}:8443"
    trap 'kill 0' EXIT
    caddy run --adapter caddyfile --config .certs/Caddyfile &
    RODER_DEV_MODE=1 cargo leptos watch

# Generate an mkcert cert for `dev-https`; install the printed rootCA on the device.
dev-certs host=lan_ip:
    mkcert -install
    mkdir -p .certs
    mkcert -cert-file .certs/roder.pem -key-file .certs/roder-key.pem {{ host }} localhost 127.0.0.1
    @echo "Install this CA on the device: $(mkcert -CAROOT)/rootCA.pem"

# Bundle the self-hosted webfonts (Rubik for the UI, JetBrainsMono Nerd Font Mono
# for logs) into public/fonts as compressed woff2, copied from your installed
# fonts so the strict CSP needs no external CDN and there's no runtime internet.
fonts:
    #!/usr/bin/env bash
    set -eu
    dst="public/fonts"
    mkdir -p "$dst"
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    bundle() {
      local src out
      src=$(fc-list | grep -Fm1 "$1" | cut -d: -f1 | xargs)
      [ -n "$src" ] || { echo "font not installed: $1"; exit 1; }
      cp "$src" "$tmp/f.ttf"
      woff2_compress "$tmp/f.ttf"
      cp "$tmp/f.woff2" "$dst/$2"
      rm -f "$tmp/f.ttf" "$tmp/f.woff2"
      echo "  $2 <- $src"
    }
    bundle "Rubik[wght].ttf" "rubik.woff2"
    bundle "JetBrainsMonoNerdFontMono-Regular.ttf" "jetbrains-mono-nerd.woff2"
    rm -f "$dst"/*.ttf
    echo "installed: $(ls -1 "$dst")"

# Production build: server binary + hashed wasm/site assets
build:
    cargo leptos build --release

# Type-check the whole workspace (server feature set)
check:
    cargo check --workspace --features ssr

# Format
fmt:
    cargo fmt --all

# Lint (server + wasm feature sets)
lint:
    cargo clippy --workspace --features ssr -- -D warnings
    cargo clippy -p roder --no-default-features --features hydrate -- -D warnings

# Tests
test:
    cargo test --workspace --features ssr

# Build the container image
docker tag="roder:dev":
    docker build -t {{ tag }} .

# Spin up a local kind cluster for the verify loop
kind-up:
    kind create cluster --name roder-dev

kind-down:
    kind delete cluster --name roder-dev
