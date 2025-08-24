{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    naersk.url = "github:nix-community/naersk";
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    {
      self,
      flake-utils,
      naersk,
      nixpkgs,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = (import nixpkgs) {
          inherit system;
        };

        naersk' = pkgs.callPackage naersk { };
        scope-rs = naersk'.buildPackage {
          buildInputs = with pkgs; [ perl openssl zstd ];
          nativeBuildInputs = with pkgs; [ pkg-config ];
          src = ./.;
        };

      in {
        # For `nix build` & `nix run`:
        defaultPackage = scope-rs;

        # For `nix develop` (optional, can be skipped):
        devShell = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            cargo-audit
            cargo-nextest
            grcov
            llvmPackages_19.libllvm
            rust-analyzer
          ];
        };

        # Overlay for package usage in other Nix configurations
        overlay = _: _: {
          inherit scope-rs;
        };
      }
    );
}
