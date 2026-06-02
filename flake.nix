{
  description = "roder — an in-cluster Kubernetes web GUI (axum + kube-rs + Leptos)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      rust-overlay,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        # Stable Rust with the wasm32 target — required to build the Leptos hydrate
        # bundle. The nixpkgs rust channel can't cross-compile, so we use rust-overlay.
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-unknown-unknown" ];
          extensions = [
            "rust-src"
            "clippy"
            "rustfmt"
            "rust-analyzer"
          ];
        };
      in
      {
        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain
            pkgs.cargo-leptos
            # wasm-bindgen-cli MUST match the pinned `wasm-bindgen` crate (=0.2.100)
            # so cargo-leptos uses it from the store instead of downloading a binary
            # that can't run on NixOS. Bump both together on a leptos upgrade.
            pkgs.wasm-bindgen-cli_0_2_100
            pkgs.dart-sass # styles
            pkgs.binaryen # wasm-opt
            pkgs.just
            # cluster tooling for the dev/verify loop
            pkgs.kubectl
            pkgs.kubernetes-helm
            pkgs.kind
            # HTTPS for testing from phones/remote devices, which block WebAssembly
            # on insecure (http://LAN-IP) origins. `just dev-https`.
            pkgs.caddy
            pkgs.mkcert
            # `just fonts`: fetch + woff2-compress the self-hosted webfonts.
            pkgs.unzip
            pkgs.woff2
          ];
        };
      }
    );
}
