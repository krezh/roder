{ pkgs, lib, ... }: {

  packages = [
    pkgs.openssl
    pkgs.pkg-config
    pkgs.dart-sass
    pkgs.infisical
    pkgs.heaptrack
    pkgs.cargo-leptos
    pkgs.kubernetes-helm
    pkgs.act
  ];

  claude.code = {
    enable = true;
    mcpServers = {
      "mcp.devenv.sh" = {
        type = "http";
        url = "https://mcp.devenv.sh";
      };
      playwright = {
        type = "stdio";
        command = lib.getExe pkgs.playwright-mcp;
        args = [
          "--headless"
          "--isolated"
          "--allowed-hosts=localhost:8888"
        ];
      };
    };
  };

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

  env = {
    RODER_DEV_MODE = "1";
    RUST_LOG = "info";
    RODER_BASE_URL = "http://127.0.0.1:8080";
    RODER_ALERTMANAGER_URL = "https://alertmanager.plexuz.xyz";
  };

  processes.dev.exec = "cargo leptos watch";

  tasks = {
    "tools:wasm-bindgen" = {
      exec = ''
        WBG_VER=$(${lib.getExe pkgs.yq-go} .workspace.dependencies.wasm-bindgen Cargo.toml)
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

    "test:helm" = {
      exec = ''
        helm lint helm
        helm template roder helm >/dev/null
        helm template roder-ha helm --set replicaCount=2 >/dev/null
      '';
      before = [ "devenv:enterTest" ];
    };

    "test:docker" = {
      exec = ''
        WBG_VER=$(${lib.getExe pkgs.yq-go} .workspace.dependencies.wasm-bindgen Cargo.toml)
        docker buildx build --build-arg WB_VERSION="$WBG_VER" -t roder:test .
      '';
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
    echo "  check        - Type-check the workspace (ssr)"
    echo "  fmt          - Format all code"
    echo "  lint         - Lint (ssr + hydrate)"
    echo "  test         - Run workspace tests"
  '';
}
