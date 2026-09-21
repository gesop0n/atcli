{
  lib,
  rustPlatform,
  gitMinimal,
  makeWrapper,
}:
rustPlatform.buildRustPackage {
  pname = "atcli";
  # バージョンは Cargo.toml だけで管理する。リリースのタグもここから決まる。
  inherit ((lib.importTOML ./Cargo.toml).workspace.package) version;
  src = lib.cleanSource ./.;
  cargoLock.lockFile = ./Cargo.lock;
  # ワークスペースのうち配布するのは atcli だけ。
  cargoBuildFlags = [
    "-p"
    "atcli"
  ];
  cargoTestFlags = [ "--workspace" ];
  # git は commit サブコマンドとそのテストで使う
  nativeBuildInputs = [
    gitMinimal
    makeWrapper
  ];
  postInstall = ''
    wrapProgram $out/bin/atcli \
      --prefix PATH : ${lib.makeBinPath [ gitMinimal ]}
  '';
  meta = {
    description = "Prepare and locally test AtCoder solutions";
    # apps を置かない代わりに nix run のフォールバック先を明示する
    mainProgram = "atcli";
  };
}
