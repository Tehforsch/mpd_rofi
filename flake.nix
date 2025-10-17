{
  description = "Music selection tool";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" ];
        };

        buildInputs = with pkgs; [
          rustToolchain
        ];

        nativeBuildInputs = with pkgs; [
          pkg-config
        ];
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "music_selection";
          version = "0.1.0";
          
          src = ./.;
          
          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          inherit buildInputs nativeBuildInputs;

          meta = with pkgs.lib; {
            description = "Music selection tool";
            license = licenses.mit;
            maintainers = [ ];
          };
        };

        devShells.default = pkgs.mkShell {
          inherit buildInputs nativeBuildInputs;
          
          shellHook = ''
            echo "Music selection development environment"
          '';
        };
      });
}