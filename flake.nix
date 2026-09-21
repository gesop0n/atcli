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
    # overlay は system に依存しないので eachSystem の外に出す。
    {
      overlays.default = final: _prev: {
        atcli = final.callPackage ./package.nix { };
      };
    }
    # nixpkgs 26.11 が x86_64-darwin のサポートを打ち切ったため、eachDefaultSystem は
    # 使えない。ビルドできる system だけを明示的に並べる。
    //
      flake-utils.lib.eachSystem
        [
          "aarch64-darwin"
          "aarch64-linux"
          "x86_64-linux"
        ]
        (
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
            atcli = pkgs.callPackage ./package.nix { inherit rustPlatform; };
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
