mod atcoder;
mod config;
mod fetch_cmd;
mod model;
mod new_cmd;
mod paths;
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
}

#[derive(Debug, Args)]
struct NewArgs {
    /// `AtCoder` のコンテスト ID（例: abc300）
    contest: String,
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let current_dir = env::current_dir()?;
    let repository = Repository::discover(&current_dir)?;
    let config = Config::load(&repository)?;

    match cli.command {
        Command::New(args) => {
            new_cmd::run(&repository, &config, &args.contest, args.date.as_deref())
        }
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
            )
        }
    }
}
