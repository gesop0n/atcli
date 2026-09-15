mod atcoder;
mod commit_cmd;
mod config;
mod fetch_cmd;
mod init_cmd;
mod login_cmd;
mod model;
mod new_cmd;
mod path_cmd;
mod paths;
mod session;
mod submit_cmd;
mod test_cmd;

use std::{env, path::PathBuf};

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

use crate::{config::Config, paths::Repository};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// コンテストの問題・テンプレート・サンプルを作成する
    New(NewArgs),
    /// 問題ページからサンプルを取り直す
    Fetch(ProblemPathArgs),
    /// C++ 解答をビルドしてローカルテストする
    Test(TestArgs),
    /// 取り組みと対応する問題データを Git にコミットする
    Commit(CommitArgs),
    /// リポジトリ内のディレクトリパスを表示する
    Path(PathArgs),
    /// 取り組みディレクトリへ移動する（shell 統合が必要）
    Cd(CdArgs),
    /// shell 統合用の関数を出力する
    Init(InitArgs),
    /// `AtCoder` にログインしてセッションを保存する
    Login(LoginArgs),
    /// 保存したログインセッションを削除する
    Logout,
    /// ローカルテスト後に C++ 解答を `AtCoder` へ提出する
    Submit(SubmitArgs),
}

#[derive(Debug, Args)]
struct NewArgs {
    /// `AtCoder` のコンテスト ID（例: abc300）
    contest: String,
    /// 作成する問題（例: a c ex、範囲指定は a01..a05、省略時は全問題）
    #[arg(value_name = "PROBLEM")]
    problems: Vec<String>,
    /// 作成先の日付（省略時はローカルの今日、形式: YYYY-MM-DD）
    #[arg(long)]
    date: Option<String>,
    /// 問題を指定せずに全問題を作成する（問題数が多いコンテストで必要）
    #[arg(long, conflicts_with = "problems")]
    all: bool,
    /// 作成後にコンテストディレクトリへ移動する（shell 統合が必要）
    #[arg(long)]
    cd: bool,
}

#[derive(Debug, Args)]
struct ProblemPathArgs {
    /// 問題・取り組みディレクトリ、またはその子ディレクトリ
    #[arg(default_value = ".")]
    path: PathBuf,
}

#[derive(Debug, Args)]
struct TestArgs {
    /// 取り組みディレクトリまたはその子ディレクトリ
    #[arg(default_value = ".")]
    path: PathBuf,
    /// 提出相当の最適化フラグでビルドする
    #[arg(short = 'r', long)]
    release: bool,
    /// 1 ケースだけ実行する（例: sample-1 または sample-1.in）
    #[arg(short = 'c', long = "case")]
    case: Option<String>,
    /// キャッシュを使わず強制的に再ビルドする
    #[arg(long)]
    rebuild: bool,
}

#[derive(Debug, Args)]
struct CommitArgs {
    /// 取り組みディレクトリまたはその子ディレクトリ
    #[arg(default_value = ".")]
    path: PathBuf,
    /// 自動生成する代わりに使用するコミットメッセージ
    #[arg(short, long)]
    message: Option<String>,
    /// コミットせず、メッセージと対象の変更を表示する
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct PathArgs {
    #[command(subcommand)]
    target: PathTarget,
}

#[derive(Debug, Subcommand)]
enum PathTarget {
    /// リポジトリのルートディレクトリ
    Root,
    /// 今日（または指定日）の取り組みディレクトリ
    Today {
        /// 表示する日付（省略時はローカルの今日、形式: YYYY-MM-DD）
        #[arg(long)]
        date: Option<String>,
    },
    /// コンテスト（または問題）の取り組みディレクトリ
    Attempt {
        /// `AtCoder` のコンテスト ID（例: abc300）
        contest: String,
        /// 問題のラベル（例: b、省略時はコンテストディレクトリ）
        problem: Option<String>,
        /// 表示する日付（省略時は今日、無ければ最新の該当日）
        #[arg(long)]
        date: Option<String>,
    },
}

#[derive(Debug, Args)]
struct CdArgs {
    /// 移動先（`root`、`today`、またはコンテスト ID。省略時は today）
    #[arg(value_name = "TARGET")]
    target: Option<String>,
    /// 問題のラベル（例: b。TARGET にコンテスト ID を指定したときのみ）
    problem: Option<String>,
    /// 対象の日付（省略時は今日、無ければ最新の該当日、形式: YYYY-MM-DD）
    #[arg(long)]
    date: Option<String>,
}

#[derive(Debug, Args)]
struct InitArgs {
    /// 対象の shell
    #[arg(value_enum)]
    shell: init_cmd::Shell,
}

#[derive(Debug, Args)]
struct LoginArgs {
    /// ブラウザの `REVEL_SESSION` を非表示入力する
    #[arg(long)]
    session: bool,
}

#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)]
struct SubmitArgs {
    /// 取り組みディレクトリまたはその子ディレクトリ
    #[arg(default_value = ".")]
    path: PathBuf,
    /// 言語 ID、完全な言語名、または一意に絞れる名前の一部
    #[arg(short, long)]
    language: Option<String>,
    /// この問題で選べる提出言語を表示して終了する
    #[arg(long)]
    list_languages: bool,
    /// ローカルテストを省略する
    #[arg(long)]
    no_test: bool,
    /// 提出前の確認を省略する
    #[arg(short = 'y', long)]
    yes: bool,
    /// 提出後、判定が終わるまで監視する
    #[arg(long, conflicts_with = "no_watch")]
    watch: bool,
    /// 提出後の判定監視を省略する
    #[arg(long, conflicts_with = "watch")]
    no_watch: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Login(args) => login_cmd::login(args.session),
        Command::Logout => login_cmd::logout(),
        // shell 起動時にも呼べるよう、リポジトリ探索より前に処理する。
        Command::Init(args) => {
            init_cmd::run(args.shell);
            Ok(())
        }
        command => run_repository_command(command),
    }
}

fn run_repository_command(command: Command) -> Result<()> {
    let current_dir = env::current_dir()?;
    let repository = Repository::discover(&current_dir)?;
    let config = Config::load(&repository)?;

    match command {
        Command::New(args) => run_new(&repository, &config, &args),
        Command::Fetch(args) => {
            let problem_dir = repository.find_problem_for(
                &current_dir.join(args.path),
                &config.repository.problems_dir,
            )?;
            fetch_cmd::run(&problem_dir)
        }
        Command::Test(args) => {
            let attempt = repository.find_attempt(
                &current_dir.join(args.path),
                &config.repository.problems_dir,
            )?;
            test_cmd::run(
                &repository,
                &config,
                &attempt,
                args.release,
                args.case.as_deref(),
                args.rebuild,
            )?
            .require_success()
            .map(|_| ())
        }
        Command::Commit(args) => {
            let attempt = repository.find_attempt(
                &current_dir.join(args.path),
                &config.repository.problems_dir,
            )?;
            commit_cmd::run(&repository, &attempt, args.message.as_deref(), args.dry_run)
        }
        Command::Path(args) => {
            println!(
                "{}",
                resolve_path_target(&repository, &config, args.target)?.display()
            );
            Ok(())
        }
        Command::Cd(args) => {
            let dir = resolve_cd_target(&repository, &config, &args)?;
            init_cmd::warn_unless_integrated("atcli cd");
            println!("{}", dir.display());
            Ok(())
        }
        Command::Submit(args) => {
            let attempt = repository.find_attempt(
                &current_dir.join(args.path),
                &config.repository.problems_dir,
            )?;
            let watch = match (args.watch, args.no_watch) {
                (true, false) => Some(true),
                (false, true) => Some(false),
                (false, false) => None,
                (true, true) => unreachable!("clap rejects conflicting flags"),
            };
            submit_cmd::run(
                &repository,
                &config,
                &attempt,
                submit_cmd::Options {
                    language: args.language.as_deref(),
                    list_languages: args.list_languages,
                    no_test: args.no_test,
                    yes: args.yes,
                    watch,
                },
            )
        }
        Command::Login(_) | Command::Logout | Command::Init(_) => {
            unreachable!("handled before repository discovery")
        }
    }
}

/// `--cd` のときだけ、移動先を stdout へ 1 行で出す。
///
/// 進捗表示は `new_cmd` 側で stderr へ回るため、stdout はパス専用になる。
fn run_new(repository: &Repository, config: &Config, args: &NewArgs) -> Result<()> {
    let destination = new_cmd::run(
        repository,
        config,
        &args.contest,
        &args.problems,
        args.date.as_deref(),
        args.all,
        args.cd,
    )?;
    if args.cd {
        init_cmd::warn_unless_integrated("--cd");
        println!("{}", destination.display());
    }
    Ok(())
}

fn resolve_path_target(
    repository: &Repository,
    config: &Config,
    target: PathTarget,
) -> Result<PathBuf> {
    match target {
        PathTarget::Root => Ok(path_cmd::root(repository)),
        PathTarget::Today { date } => path_cmd::today(repository, config, date.as_deref()),
        PathTarget::Attempt {
            contest,
            problem,
            date,
        } => path_cmd::attempt(
            repository,
            config,
            &contest,
            problem.as_deref(),
            date.as_deref(),
        ),
    }
}

/// `atcd` 由来の緩い引数の取り方をそのまま引き継ぐ。
///
/// `root` と `today` を予約語として扱うが、どちらも実在の `AtCoder` コンテスト ID とは衝突しない。
fn resolve_cd_target(repository: &Repository, config: &Config, args: &CdArgs) -> Result<PathBuf> {
    match args.target.as_deref() {
        Some("root") => {
            if args.problem.is_some() || args.date.is_some() {
                anyhow::bail!("root には問題や日付を指定できません");
            }
            Ok(path_cmd::root(repository))
        }
        None | Some("today") => {
            if let Some(problem) = &args.problem {
                anyhow::bail!(
                    "問題を指定するときはコンテスト ID も指定してください（例: atcli cd abc300 {problem}）"
                );
            }
            path_cmd::today(repository, config, args.date.as_deref())
        }
        Some(contest) => path_cmd::attempt(
            repository,
            config,
            contest,
            args.problem.as_deref(),
            args.date.as_deref(),
        ),
    }
}
