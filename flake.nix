{
  description = "atcli: a small local workflow tool for solving AtCoder problems";

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
          overlays = [ rust-overlay.overlays.default ];
        };
        rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        rustPlatform = pkgs.makeRustPlatform {
          cargo = rust;
          rustc = rust;
        };
        atcli = rustPlatform.buildRustPackage {
          pname = "atcli";
          version = "0.1.0";
          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;
          # git は commit サブコマンドとそのテストで使う
          nativeBuildInputs = [
            pkgs.gitMinimal
            pkgs.makeWrapper
          ];
          postInstall = ''
            wrapProgram $out/bin/atcli \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.gitMinimal ]}
          '';
          meta = {
            description = "Prepare and locally test AtCoder solutions";
            # apps を置かない代わりに nix run のフォールバック先を明示する
            mainProgram = "atcli";
          };
        };
      in
      {
        formatter = pkgs.nixfmt-tree;

        packages = {
          inherit atcli;
          default = atcli;
        };

        checks.atcli = atcli;

        # atcli の開発環境。
        # g++ は atcli test が呼ぶため、手元で動作確認するのに必要。
        devShells.default = pkgs.mkShell {
          packages = [
            rust
            pkgs.gitMinimal
            pkgs.gcc15
          ];
        };
      }
    );
}
