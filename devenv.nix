{ pkgs, ... }: {

  packages = [
    pkgs.openssl
    pkgs.pkg-config
    pkgs.dart-sass
    pkgs.infisical
    pkgs.heaptrack
    pkgs.cargo-leptos
  ];

  claude.code.enable = true;

  languages.rust = {
    enable = true;
    channel = "stable";
    lsp.enable = true;
    components = [
      "rustc"
      "cargo"
      "clippy"
      "rustfmt"
      "rust-analyzer"
    ];
    targets = [ "wasm32-unknown-unknown" ];
  };

  scripts = {
    # Run roder with real OIDC auth (not dev-bypass) for testing the OAuth flow.
    # Needs RODER_OIDC_* + RODER_SESSION_KEY in the env — `enterShell` loads them
    # from Infisical (see below); without them roder hard-fails at startup.
    dev-auth.exec = ''
      ig() { timeout 10 infisical secrets get "$1" --path="$2" --env=default --plain 2>/dev/null; }
      if roder_cid=$(ig KUBERNETES_OAUTH_CLIENT_ID /Kubernetes/DexTek/Kubernetes) && [ -n "$roder_cid" ]; then
        export RODER_OIDC_CLIENT_ID="$roder_cid"
        export RODER_OIDC_CLIENT_SECRET="$(ig KUBERNETES_OAUTH_CLIENT_SECRET /Kubernetes/DexTek/Kubernetes)"
        export RODER_SESSION_KEY="$(ig RODER_SESSION_KEY /Kubernetes/DexTek/Roder)"
        export RODER_OIDC_ISSUER_URL="https://sso.plexuz.xyz/application/o/kubernetes/"
        export RODER_BASE_URL="''${RODER_BASE_URL:-http://localhost:8080}"
        echo "infisical: roder OIDC secrets loaded — 'dev-auth' ready"
      else
        echo "infisical: not loaded (dev-bypass mode). For OAuth: infisical login && direnv reload"
      fi
      echo "Starting roder in real-auth (OIDC) mode…"
      env -u RODER_DEV_MODE cargo leptos watch
    '';

    docker-run.exec = ''
      TAG="''${2:-roder:dev}"
      name="roder-docker-test"
      docker rm -f "$name" >/dev/null 2>&1 || true
      docker run --rm \
        --name "$name" \
        --network host \
        --user 0:0 \
        -e RODER_DEV_MODE=1 \
        -e RUST_LOG=info \
        -e KUBECONFIG=/tmp/kube/config \
        -v "~/.kube/config:/tmp/kube/config:ro" \
        "$TAG"
    '';
  };

  env.RODER_DEV_MODE = "1";
  env.RUST_LOG = "info";

  processes.dev.exec = "cargo leptos watch";

  tasks = {
    "tools:wasm-bindgen" = {
      exec = ''
        WBG_VER=$(grep -E '^wasm-bindgen\s*=' Cargo.toml | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
        INSTALLED_VER=$(wasm-bindgen --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' || echo "none")
        if [ "$INSTALLED_VER" != "$WBG_VER" ]; then
          echo "Installing wasm-bindgen-cli $WBG_VER..."
          cargo install wasm-bindgen-cli --version "$WBG_VER" --quiet --force
        fi
      '';
      before = [
        "devenv:enterShell"
        "devenv:processes:dev"
      ];
    };

    "test:fmt" = {
      exec = "cargo fmt --all -- --check";
      before = [ "devenv:enterTest" ];
    };

    "test:lint-ssr" = {
      exec = "cargo clippy --workspace --features ssr -- -D warnings";
      before = [ "devenv:enterTest" ];
    };

    "test:lint-hydrate" = {
      exec = "cargo clippy -p roder --no-default-features --features hydrate -- -D warnings";
      before = [ "devenv:enterTest" ];
    };

    "test:cargo" = {
      exec = "cargo test --workspace --features ssr";
      before = [ "devenv:enterTest" ];
    };

    "test:docker" = {
      exec = "docker buildx build -t roder:test .";
      before = [ "devenv:enterTest" ];
    };
  };
  enterShell = ''
    echo "roder dev environment"
    echo "  rustc: $(rustc --version)"
    echo "  cargo-leptos: $(cargo-leptos --version)"
    echo "  wasm-bindgen: $(wasm-bindgen --version)"
    echo "  sass: $(sass --version)"
    echo ""
    echo "Available commands:"
    echo "  dev          - Start dev server with hot-reload (auth bypassed)"
    echo "  dev-auth     - Start dev server with real OIDC auth (needs Infisical)"
    echo "  check        - Type-check the workspace (ssr)"
    echo "  fmt          - Format all code"
    echo "  lint         - Lint (ssr + hydrate)"
    echo "  test         - Run workspace tests"
    echo "  docker-build - Build debug image [tag]"
    echo "  docker-release - Build release image [tag]"
    echo "  docker-run   - Build & run dev image [tag]"
  '';
}
