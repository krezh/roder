_default:
    @just --list

# Type-check the whole workspace (server feature set).
check:
    cargo check --workspace --features ssr

# Format all code in the workspace.
fmt:
    cargo fmt --all

# Lint (server + wasm feature sets).
lint:
    cargo clippy --workspace --features ssr -- -D warnings
    cargo clippy -p roder --no-default-features --features hydrate -- -D warnings

# Run all workspace tests.
test:
    cargo test --workspace --features ssr

# Build the production container image.
docker tag="roder:dev":
    docker build -t {{ tag }} .

# Build the production image and run it locally in dev mode (bypasses OIDC,
# uses the host kubeconfig). Open http://127.0.0.1:8080.
docker-run kubeconfig="${KUBECONFIG:-$HOME/.kube/config}" tag="roder:dev": (docker tag)
    #!/usr/bin/env bash
    set -euo pipefail
    if ! test -r "{{ kubeconfig }}"; then
        echo "kubeconfig not readable: {{ kubeconfig }}" >&2
        exit 1
    fi
    name="roder-docker-test"
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker run --rm \
      --name "$name" \
      --network host \
      --user 0:0 \
      -e RODER_DEV_MODE=1 \
      -e RUST_LOG=info \
      -e KUBECONFIG=/tmp/kube/config \
      -v "{{ kubeconfig }}:/tmp/kube/config:ro" \
      {{ tag }}

# Spin up / tear down a local kind cluster.
kind-up:
    kind create cluster --name roder-dev

kind-down:
    kind delete cluster --name roder-dev
