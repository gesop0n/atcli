# atcli

AtCoder の問題を解くための CLI。問題データとサンプルの取得、ローカルテスト、コミット、提出を行う。

## インストール

Nix flakes を有効にした環境なら、そのまま実行できる。

```console
$ nix run github:gesop0n/atcli -- --help
```

または

```console
$ nix profile install github:gesop0n/atcli
```

`github:gesop0n/atcli` は main の最新を指す。リリースを使う場合はタグで参照する。`v0.1.0` のようなバージョンタグは動かず、`latest` は最新のリリースを指す。

```console
$ nix profile install github:gesop0n/atcli/latest
$ nix run github:gesop0n/atcli/v0.1.0 -- --help
```

リポジトリの devShell に入れる場合は flake input として参照する。

```nix
{
  inputs.atcli = {
    # 特定のリリースに固定するなら "github:gesop0n/atcli/v0.1.0"
    url = "github:gesop0n/atcli/latest";
    inputs.nixpkgs.follows = "nixpkgs";
  };

  # devShells.default の packages に atcli.packages.${system}.default を追加する
}
```

`atcli test` と `atcli submit` は C++ コンパイラを呼ぶため、同じ shell に GCC が必要である。

Nix を使わない場合は cargo でインストールできる。Rust 1.98 以降が必要。

```console
$ cargo install --git https://github.com/gesop0n/atcli --tag v0.1.0 atcli
```

## リポジトリの準備

atcli は、実行したディレクトリから上へ辿って最初に見つけた `atcli.toml` のあるディレクトリをリポジトリのルートとみなす。解答を管理するリポジトリのルートに `atcli.toml` を置く。

```toml
[repository]
problems_dir = "problems"
attempts_dir = "attempts"
template = "template/main.cpp"
```

`template` に指定した `main.cpp` が、新しい取り組みの雛形になる。

問題ごとに共有するメタデータとテストは `problems_dir` に置かれる。

```text
problems/contest/task/
├── meta.toml
└── tests/
    ├── sample-1.in
    └── sample-1.out
```

日付ごとの取り組みは `attempts_dir` に置かれる。`attempt.toml` の `problem` は、`problems_dir` から問題データへの相対パスになる。

```text
attempts/YYYY/MM/DD/contest/task/
├── attempt.toml  # problem = "abc300/a"
└── main.cpp
```

同じ問題へ別の日に取り組む場合、`problems/` のメタデータとテストは再利用し、日付ごとに異なる `main.cpp` と `attempt.toml` を作成する。日付はコンテスト開催日ではなく、`atcli new` を実行して解き始めた日付になる。

## 使い方

コンテストの全問題について、共有する問題データと、コマンドを実行した日付の取り組みを作成する。開催中のコンテストは未ログインだと問題一覧が 404 になるため、事前に `atcli login` しておく。`new` と `fetch` は保存済みセッションがあればそれを使う。

```console
$ atcli new abc300
# 問題データ: problems/abc300/{a,b,c,d,e,f,g,ex}
# 取り組み:   attempts/2026/09/09/abc300/{a,b,c,d,e,f,g,ex}
```

日付の明示もできる。

```console
$ atcli new abc300 --date 2026-09-08
```

問題を指定した場合は、その問題だけを作成する。ラベルは大文字小文字を区別せず、複数指定できる。`..` で範囲も指定できる。範囲はラベルの文字列計算ではなくコンテストの問題順で解決するため、ラベルの付け方に依存しない。

```console
$ atcli new abc300 a c ex
# 例: 2026/09/09/abc300/{a,c,ex}
$ atcli new abc300 c..e
# 例: 2026/09/09/abc300/{c,d,e}
```

`tessoku-book`（競技プログラミングの鉄則 演習問題集）のような常設コンテストも同じように扱えるが、151 問あるため問題指定を省略すると 1 日分の取り組みとして全問を作ってしまう。問題数が 20 問を超えるコンテストでは問題の指定を必須とし、本当に全問作成する場合だけ `--all` を指定する。

```console
$ atcli new tessoku-book           # エラー。問題の指定を促す
$ atcli new tessoku-book a01..a05  # problems/tessoku-book/{a01,a02,a03,a04,a05}
$ atcli new tessoku-book --all     # 151 問すべて。時間がかかる
```

取り組みディレクトリへ移動して解答を書き、サンプルを実行する。

```console
$ cd attempts/2026/09/08/abc300/a
$ atcli test
$ atcli test --release
$ atcli test --case sample-1
$ atcli test --rebuild
```

同じビルド設定で `main.cpp` と依存ヘッダに変更がなければ、前回のビルド結果を再利用する。`--rebuild` を指定するとキャッシュを使わず再ビルドする。

デバッグビルドは既定で AddressSanitizer と UndefinedBehaviorSanitizer を有効にする。範囲外アクセスや未定義動作はその場で検出され、標準エラー出力に `main.cpp` の位置が出るので、そこから停止位置と該当行を表示する。実行速度は数倍遅くなるため、重いケースで `timeout_multiplier` の引き上げが必要になることがある。

サニタイザが何も言わずにシグナルで異常終了した場合に限り、macOS では LLDB、Linux では GDB が利用できれば失敗ケースを再実行して停止位置を求める。macOS でこの再実行を行うには developer mode が必要で、無効なままだと実行のたびに管理者パスワードを求められる。`sudo DevToolsSecurity -enable` を一度実行しておくか、`crash_diagnostics = false` で再実行自体を無効にする。

`--release` では提出時と同じ挙動を優先するため、サニタイザもデバッガによる追加診断も行わない。

ローカルが macOS の場合、コンパイラのメジャーバージョンを合わせても AtCoder の x86_64 Linux 環境を完全には再現していない。

テストを通過した取り組みを Git に保存するには、取り組みディレクトリで `atcli commit` を実行する。取り組みと、それが参照する問題データにある変更だけをステージしてコミットするため、別の問題ですでにステージしている変更は含まれない。コミットメッセージは問題メタデータから `Solve ABC178 B: Product Max` の形式で生成される。

```console
$ atcli commit
$ atcli commit -m "解説AC ABC178 B"
$ atcli commit --dry-run
```

`--dry-run` は、コミットやステージを行わずにメッセージと対象の変更を表示する。`atcli commit path/to/attempt` のように取り組みディレクトリを指定することもできる。テストは自動実行しないため、必要に応じて先に `atcli test` を実行する。

サンプルを問題ページから取り直すには `atcli fetch` を使う。`sample-*.in` と `sample-*.out` だけを更新し、`my-*.in` などの自作ケースは残す。

```console
$ atcli fetch
```

`tests/my-1.in` のように対応する `.out` がないケースは、実行結果を表示するだけで合否判定しない。`.out` を置くと通常の比較対象になる。

### ディレクトリ移動

atcli 自身は親 shell の cwd を変えられないため、移動系のコマンドは shell 関数のラッパーを通す。その関数は `atcli init zsh` が出力する。

```zsh
eval "$(atcli init zsh)"
```

これで `atcd`、`atcli cd`、`atcli new --cd` が使えるようになる。

```console
$ atcd                        # 今日の取り組みディレクトリ
$ atcd root                   # リポジトリのルート
$ atcd --date 2026-09-08      # 指定日の取り組みディレクトリ
$ atcli cd abc462             # attempts/YYYY/MM/DD/abc462
$ atcli cd abc462 b           # attempts/YYYY/MM/DD/abc462/b
$ atcli new abc462 b --cd     # 作成して、そのまま attempts/YYYY/MM/DD/abc462/b へ移動する
```

`atcli cd` は日付を省略するとまず今日を見て、無ければそのコンテストを含む最新の日へ遡る。数日前に解いた問題へ戻るときに日付を思い出さなくて済む。`--date` を明示した場合は遡らない。`atcli new --cd` は、1 問だけ作ったときはその問題のディレクトリへ、複数の問題を作ったときはコンテストディレクトリへ移動する。

`atcli` を direnv 経由でリポジトリ内でのみ PATH に載せている場合、shell 起動時に上の `eval` は実行できない。初回呼び出しで本物へ差し替えるブートストラップを置く。

```zsh
_atcli_bootstrap() {
  local _atcli_init
  _atcli_init="$(command atcli init zsh)" || return
  unfunction atcli atcd _atcli_bootstrap 2>/dev/null
  eval "$_atcli_init"
}
atcli() { _atcli_bootstrap || return; atcli "$@"; }
atcd()  { _atcli_bootstrap || return; atcd  "$@"; }
```

`atcli init zsh` の出力を取得してから `unfunction` する順序が重要で、atcli が PATH にないときもブートストラップが残り、次回また試せる。

shell 統合なしでパスだけが欲しい場合は `atcli path` を使う。`atcli path root`、`atcli path today [--date]`、`atcli path attempt <contest> [problem] [--date]` がある。

### ログインと提出

AtCoder のログイン画面で Cloudflare のブラウザ認証が求められる場合は、ブラウザでログインしてから Developer Tools の Cookie 一覧にある `REVEL_SESSION` を取り込む。

```console
$ atcli login --session
REVEL_SESSION: # 値は画面に表示されない
```

ブラウザ認証がない環境では `atcli login` でユーザー名とパスワードによるログインも試せる。CI では `ATCODER_USERNAME` / `ATCODER_PASSWORD`、Cookie を直接取り込む場合は `ATCODER_REVEL_SESSION` を利用できる。パスワードは保存せず、セッション Cookie だけを `~/.atcli/session.json` へパーミッション `0600` で保存する。

取り組みディレクトリで `submit` を実行すると、`--release` 相当で全ローカルテストを行い、提出内容を確認してから送信する。提出後はデフォルトで判定完了まで監視する。

```console
$ atcli submit
$ atcli submit --language 'C++23 (GCC' --yes
$ atcli submit --list-languages
$ atcli submit --no-watch
```

AtCoder の提出ページで CAPTCHA が要求される練習提出は、非公式 CLI から直接送信できない。`tessoku-book` などの常設コンテストへの提出がこれにあたる。`atcli submit` が表示する URL をブラウザで開き、表示された `main.cpp` を貼り付けて CAPTCHA を完了して提出する。開催中コンテストなど、提出ページに CAPTCHA がない場合は従来どおり CLI から直接提出する。

テストに失敗した解答は提出しない。interactive 問題など、ローカル判定できない場合に限り、確認のうえ `--no-test` で明示的に省略できる。保存したセッションを削除するには `atcli logout` を使う。

## 設定

リポジトリのルートの `atcli.toml` で設定する。各キーは省略でき、省略時は次の既定値になる。

```toml
[repository]
problems_dir = "problems"
attempts_dir = "attempts"
template = "template/main.cpp"

[cpp]
compiler = "g++"
standard = "gnu++23"
include_dirs = ["lib", "ac-library"]
debug_flags = ["-O0", "-g3", "-Wall", "-Wextra", "-fsanitize=address,undefined", "-fno-sanitize-recover=all"]
release_flags = ["-O2", "-Wall", "-Wextra"]

[test]
# ローカル環境と AtCoder の実行環境には差があるため、問題の制限時間に余裕を持たせる。
timeout_multiplier = 2.0
minimum_timeout_ms = 1000
# シグナルで異常終了し、サニタイザが位置を示さなかった場合に LLDB / GDB で再実行する。
crash_diagnostics = true

[submit]
# AtCoder の言語 ID、言語名、または一意に絞れる名前の一部。
language = "C++23 (GCC"
watch = true
poll_interval_ms = 2000
```

`include_dirs` はリポジトリのルートからの相対パスで、`atcli test` のコンパイル時に `-I` として渡される。

## 構成

Cargo ワークスペースで、4 つのクレートに分かれている。

| クレート | 役割 |
| --- | --- |
| `atcli-core` | リポジトリの構造、設定、問題メタデータ |
| `atcli-atcoder` | ログイン、セッションの保存、ページの取得と解析 |
| `atcli-judge` | コンパイル、実行、出力比較、停止位置の特定 |
| `atcli` | コマンドの組み立てと、端末への表示 |

`atcli-judge` は何も表示せず、判定結果を値として返す。他のクレートにも依存しないので、
`atcli` の外でも「C++ をビルドしてテストケースで判定する」用途に使える。表示は `atcli` に集約している。

API ドキュメントは <https://gesop0n.github.io/atcli/doc/atcli_judge/index.html> にある。

## 開発

```console
$ nix develop
$ cargo test --workspace
$ cargo clippy --workspace --all-targets -- -D warnings
```

devShell には Rust toolchain に加えて GCC が入る。`atcli test` が C++ コンパイラを呼ぶため、手元で動作確認するのに必要。

解答リポジトリから手元のチェックアウトを使って動作確認する場合は、flake input を差し替える。

```console
$ nix develop --override-input atcli path:/path/to/atcli
```

### リリース

`Cargo.toml` の `workspace.package.version` を上げて main に push する。CI が通ると [Tag Release](.github/workflows/tag.yml) が `v<version>` タグと GitHub Release を作り、`latest` タグをそこへ動かす。version を変えない push では何もしない。

`workspace.dependencies` にある内部クレートの `version` も同じ値に上げる。ずれていると依存解決に失敗して CI が落ち、タグは付かない。

## ライセンス

MIT
