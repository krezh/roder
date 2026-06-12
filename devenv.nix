{ pkgs, ... }:

{
  packages = [
    pkgs.openssl
    pkgs.pkg-config
    pkgs.dart-sass
  ];

  claude.code.enable = true;

  languages.rust = {
    enable = true;
    channel = "stable";
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
    check.exec = "cargo check --workspace --features ssr";
    fmt.exec = "cargo fmt --all";
    lint.exec = ''
      cargo clippy --workspace --features ssr -- -D warnings
      cargo clippy -p roder --no-default-features --features hydrate -- -D warnings
    '';
    test.exec = "cargo test --workspace --features ssr";
    docker-build.exec = ''
      TAG="''${1:-roder:dev}"
      docker build --build-arg RELEASE=false -t "$TAG" .
    '';
    docker-release.exec = ''
      TAG="''${1:-roder:release}"
      docker build -t "$TAG" .
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

  processes.dev = {
    exec = "cargo leptos watch";
    process-compose.environment = {
      RODER_DEV_MODE = "1";
      RUST_LOG = "info";
    };
  };

  tasks = {
    "tools:cargo-leptos" = {
      exec = ''
        if ! command -v cargo-leptos &> /dev/null; then
          echo "Installing cargo-leptos..."
          cargo install cargo-leptos --locked --quiet
        fi
      '';
      before = [
        "devenv:enterShell"
        "devenv:processes:dev"
      ];
    };

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
      exec = "docker build -t roder:test .";
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
    echo "  dev          - Start dev server with hot-reload"
    echo "  check        - Type-check the workspace (ssr)"
    echo "  fmt          - Format all code"
    echo "  lint         - Lint (ssr + hydrate)"
    echo "  test         - Run workspace tests"
    echo "  docker-build - Build debug image [tag]"
    echo "  docker-release - Build release image [tag]"
    echo "  docker-run   - Build & run dev image [tag]"
  '';
}
