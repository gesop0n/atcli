mod atcoder;
mod config;
mod fetch_cmd;
mod login_cmd;
mod model;
mod new_cmd;
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
    /// 作成する問題（例: a c ex、省略時は全問題）
    #[arg(value_name = "PROBLEM")]
    problems: Vec<String>,
    /// 作成先の日付（省略時はローカルの今日、形式: YYYY-MM-DD）
    #[arg(long)]
    date: Option<String>,
}

#[derive(Debug, Args)]
struct ProblemPathArgs {
    /// 問題ディレクトリまたはその子ディレクトリ
    #[arg(default_value = ".")]
    path: PathBuf,
}

#[derive(Debug, Args)]
struct TestArgs {
    /// 問題ディレクトリまたはその子ディレクトリ
    #[arg(default_value = ".")]
    path: PathBuf,
    /// 提出相当の最適化フラグでビルドする
    #[arg(short = 'r', long)]
    release: bool,
    /// 1 ケースだけ実行する（例: sample-1 または sample-1.in）
    #[arg(short = 'c', long = "case")]
    case: Option<String>,
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
    /// 問題ディレクトリまたはその子ディレクトリ
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
        command => run_repository_command(command),
    }
}

fn run_repository_command(command: Command) -> Result<()> {
    let current_dir = env::current_dir()?;
    let repository = Repository::discover(&current_dir)?;
    let config = Config::load(&repository)?;

    match command {
        Command::New(args) => new_cmd::run(
            &repository,
            &config,
            &args.contest,
            &args.problems,
            args.date.as_deref(),
        ),
        Command::Fetch(args) => {
            let problem_dir = repository.find_problem_dir(&current_dir.join(args.path))?;
            fetch_cmd::run(&problem_dir)
        }
        Command::Test(args) => {
            let problem_dir = repository.find_problem_dir(&current_dir.join(args.path))?;
            test_cmd::run(
                &repository,
                &config,
                &problem_dir,
                args.release,
                args.case.as_deref(),
            )?
            .require_success()
            .map(|_| ())
        }
        Command::Submit(args) => {
            let problem_dir = repository.find_problem_dir(&current_dir.join(args.path))?;
            let watch = match (args.watch, args.no_watch) {
                (true, false) => Some(true),
                (false, true) => Some(false),
                (false, false) => None,
                (true, true) => unreachable!("clap rejects conflicting flags"),
            };
            submit_cmd::run(
                &repository,
                &config,
                &problem_dir,
                submit_cmd::Options {
                    language: args.language.as_deref(),
                    list_languages: args.list_languages,
                    no_test: args.no_test,
                    yes: args.yes,
                    watch,
                },
            )
        }
        Command::Login(_) | Command::Logout => unreachable!("handled before repository discovery"),
    }
}
