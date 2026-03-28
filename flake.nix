{
  description = "agtx dev shell and package";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        agtx = pkgs.rustPlatform.buildRustPackage {
          pname = "agtx";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [
            pkgs.pkg-config
            pkgs.makeWrapper
          ];
          buildInputs = [
            pkgs.openssl
          ];
          postFixup = ''
            wrapProgram $out/bin/agtx \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.tmux pkgs.git pkgs.gh ]}
          '';
        };
      in {
        packages.default = agtx;
        apps.default = {
          type = "app";
          program = "${agtx}/bin/agtx";
        };

        devShells.default = pkgs.mkShell {
          packages = [
            pkgs.rustc
            pkgs.cargo
            pkgs.rustfmt
            pkgs.clippy
            pkgs.git
            pkgs.tmux
            pkgs.gh
            pkgs.pkg-config
            pkgs.openssl
          ];
        };
      });
}
