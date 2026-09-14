use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};

use crate::{
    model::ProblemMeta,
    paths::{Attempt, Repository},
};

pub fn run(
    repository: &Repository,
    attempt: &Attempt,
    requested_message: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    validate_git_root(repository)?;

    let meta = ProblemMeta::read(&attempt.problem_dir)?;
    let message = requested_message
        .map(str::trim)
        .map_or_else(|| default_message(&meta), ToOwned::to_owned);
    if message.is_empty() {
        bail!("コミットメッセージを空にはできません");
    }

    let paths = [
        relative_path(repository, &attempt.dir)?,
        relative_path(repository, &attempt.problem_dir)?,
    ];
    reject_unmerged_files(repository, &paths)?;
    let status = status_for(repository, &paths)?;
    if status.is_empty() {
        bail!("この取り組みにコミットする変更はありません");
    }

    if dry_run {
        println!("Commit message: {message}");
        println!("Changes:");
        print!("{status}");
        if !status.ends_with('\n') {
            println!();
        }
        return Ok(());
    }

    git_add(repository, &paths)?;
    if !has_staged_changes(repository, &paths)? {
        bail!("この取り組みにコミットする変更はありません");
    }
    git_commit(repository, &paths, &message)
}

fn default_message(meta: &ProblemMeta) -> String {
    let contest = match meta.contest.as_str() {
        "tessoku-book" => "鉄則".to_owned(),
        contest => contest.to_ascii_uppercase(),
    };
    let problem = if meta.label.trim().is_empty() {
        meta.task_id.trim()
    } else {
        meta.label.trim()
    };
    let title = meta.title.trim();

    if title.is_empty() {
        format!("Solve {contest} {problem}")
    } else {
        format!("Solve {contest} {problem}: {title}")
    }
}

fn validate_git_root(repository: &Repository) -> Result<()> {
    let output = git_output(repository, ["rev-parse", "--show-toplevel"])?;
    require_success(output, "Git リポジトリのルートを確認できません").and_then(|output| {
        let root = String::from_utf8(output.stdout)
            .context("Git リポジトリのルートが UTF-8 ではありません")?;
        let root = PathBuf::from(root.trim())
            .canonicalize()
            .with_context(|| format!("Git リポジトリのルートを解決できません: {}", root.trim()))?;
        if root != repository.root {
            bail!(
                "atcli.toml と Git のリポジトリルートが一致しません: {} / {}",
                repository.root.display(),
                root.display()
            );
        }
        Ok(())
    })
}

fn relative_path(repository: &Repository, path: &Path) -> Result<PathBuf> {
    path.strip_prefix(&repository.root)
        .map(Path::to_path_buf)
        .with_context(|| format!("コミット対象がリポジトリの外にあります: {}", path.display()))
}

fn reject_unmerged_files(repository: &Repository, paths: &[PathBuf; 2]) -> Result<()> {
    let mut command = git_command(repository);
    command.args(["diff", "--name-only", "--diff-filter=U", "--"]);
    command.args(paths);
    let output = command
        .output()
        .context("競合中のファイルを確認するために Git を実行できません")?;
    let output = require_success(output, "競合中のファイルを確認できません")?;
    if !output.stdout.is_empty() {
        let files = String::from_utf8_lossy(&output.stdout);
        bail!("コミット対象に未解決の競合があります:\n{files}");
    }
    Ok(())
}

fn status_for(repository: &Repository, paths: &[PathBuf; 2]) -> Result<String> {
    let mut command = git_command(repository);
    command.args(["status", "--short", "--untracked-files=all", "--"]);
    command.args(paths);
    let output = command
        .output()
        .context("コミット対象を確認するために Git を実行できません")?;
    let output = require_success(output, "コミット対象を確認できません")?;
    String::from_utf8(output.stdout).context("Git のステータス出力が UTF-8 ではありません")
}

fn git_add(repository: &Repository, paths: &[PathBuf; 2]) -> Result<()> {
    let mut command = git_command(repository);
    command.args(["add", "--all", "--"]);
    command.args(paths);
    let output = command
        .output()
        .context("コミット対象をステージするために Git を実行できません")?;
    require_success(output, "コミット対象をステージできません").map(|_| ())
}

fn has_staged_changes(repository: &Repository, paths: &[PathBuf; 2]) -> Result<bool> {
    let mut command = git_command(repository);
    command.args(["diff", "--cached", "--quiet", "--"]);
    command.args(paths);
    let status = command
        .status()
        .context("ステージ済みの変更を確認するために Git を実行できません")?;
    match status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => bail!("ステージ済みの変更を確認できません: {status}"),
    }
}

fn git_commit(repository: &Repository, paths: &[PathBuf; 2], message: &str) -> Result<()> {
    let mut command = git_command(repository);
    command.args(["commit", "--only", "--message"]);
    command.arg(message);
    command.arg("--");
    command.args(paths);
    let output = command
        .output()
        .context("コミットを作成するために Git を実行できません")?;
    let output = require_success(output, "コミットを作成できません")?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    Ok(())
}

fn git_output<I, S>(repository: &Repository, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_command(repository)
        .args(args)
        .output()
        .context("Git を実行できません")
}

fn git_command(repository: &Repository) -> Command {
    let mut command = Command::new("git");
    command.current_dir(&repository.root);
    command
}

fn require_success(output: Output, context: &str) -> Result<Output> {
    if output.status.success() {
        return Ok(output);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        bail!("{context}: {}", output.status);
    }
    bail!("{context}: {detail}");
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
    };

    use tempfile::TempDir;

    use crate::{
        model::{AttemptMeta, ProblemMeta},
        paths::{Attempt, Repository},
    };

    use super::{default_message, run};

    #[test]
    fn builds_message_from_problem_metadata() {
        let mut meta = problem_meta("abc178", "B", "Product Max");
        assert_eq!(default_message(&meta), "Solve ABC178 B: Product Max");

        meta.contest = "tessoku-book".to_owned();
        meta.label = "A05".to_owned();
        meta.title = "Three Cards".to_owned();
        assert_eq!(default_message(&meta), "Solve 鉄則 A05: Three Cards");

        meta.title.clear();
        assert_eq!(default_message(&meta), "Solve 鉄則 A05");
    }

    #[test]
    fn commits_only_the_attempt_and_its_problem() {
        let fixture = GitFixture::new();
        let (repository, attempt) = fixture.create_attempt();
        fs::write(fixture.path().join("unrelated.txt"), "staged change\n").unwrap();
        fixture.git(["add", "unrelated.txt"]);

        run(&repository, &attempt, None, false).unwrap();

        assert_eq!(
            fixture.git_stdout(["log", "-1", "--format=%s"]),
            "Solve ABC178 B: Product Max\n"
        );
        let committed = fixture.git_stdout(["show", "--pretty=format:", "--name-only", "HEAD"]);
        assert!(committed.contains("attempts/2026/09/14/abc178/b/attempt.toml"));
        assert!(committed.contains("attempts/2026/09/14/abc178/b/main.cpp"));
        assert!(committed.contains("problems/abc178/b/meta.toml"));
        assert!(committed.contains("problems/abc178/b/tests/sample-1.in"));
        assert!(!committed.contains("unrelated.txt"));
        assert_eq!(
            fixture.git_stdout(["diff", "--cached", "--name-only"]),
            "unrelated.txt\n"
        );
    }

    #[test]
    fn dry_run_does_not_stage_or_commit_changes() {
        let fixture = GitFixture::new();
        let (repository, attempt) = fixture.create_attempt();
        let head = fixture.git_stdout(["rev-parse", "HEAD"]);

        run(&repository, &attempt, Some("Custom message"), true).unwrap();

        assert_eq!(fixture.git_stdout(["rev-parse", "HEAD"]), head);
        assert!(
            fixture
                .git_stdout(["diff", "--cached", "--name-only"])
                .is_empty()
        );
        assert!(
            fixture
                .git_stdout(["status", "--short"])
                .contains("?? attempts/")
        );
    }

    #[test]
    fn rejects_an_empty_custom_message() {
        let fixture = GitFixture::new();
        let (repository, attempt) = fixture.create_attempt();
        let error = run(&repository, &attempt, Some("  "), false).unwrap_err();

        assert_eq!(error.to_string(), "コミットメッセージを空にはできません");
    }

    struct GitFixture {
        temp: TempDir,
    }

    impl GitFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let fixture = Self { temp };
            fixture.git(["init", "--quiet"]);
            fixture.git(["config", "user.name", "atcli test"]);
            fixture.git(["config", "user.email", "atcli@example.com"]);
            fixture.git(["config", "commit.gpgsign", "false"]);
            fs::write(fixture.path().join("atcli.toml"), "").unwrap();
            fs::write(fixture.path().join("unrelated.txt"), "initial\n").unwrap();
            fixture.git(["add", "atcli.toml", "unrelated.txt"]);
            fixture.git(["commit", "--quiet", "-m", "Initial commit"]);
            fixture
        }

        fn path(&self) -> &Path {
            self.temp.path()
        }

        fn create_attempt(&self) -> (Repository, Attempt) {
            let problem_dir = self.path().join("problems/abc178/b");
            fs::create_dir_all(problem_dir.join("tests")).unwrap();
            ProblemMeta {
                url: "https://atcoder.jp/contests/abc178/tasks/abc178_b".to_owned(),
                contest: "abc178".to_owned(),
                task_id: "abc178_b".to_owned(),
                label: "B".to_owned(),
                title: "Product Max".to_owned(),
                time_limit_ms: 2_000,
                interactive: false,
                tolerance: None,
            }
            .write(&problem_dir)
            .unwrap();
            fs::write(problem_dir.join("tests/sample-1.in"), "1 2 3 4\n").unwrap();

            let attempt_dir = self.path().join("attempts/2026/09/14/abc178/b");
            fs::create_dir_all(&attempt_dir).unwrap();
            AttemptMeta {
                problem: PathBuf::from("abc178/b"),
            }
            .write(&attempt_dir)
            .unwrap();
            fs::write(attempt_dir.join("main.cpp"), "int main() {}\n").unwrap();

            let repository = Repository::discover(self.path()).unwrap();
            let attempt = Attempt {
                dir: attempt_dir.canonicalize().unwrap(),
                problem_dir: problem_dir.canonicalize().unwrap(),
            };
            (repository, attempt)
        }

        fn git<const N: usize>(&self, args: [&str; N]) {
            let status = Command::new("git")
                .current_dir(self.path())
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        }

        fn git_stdout<const N: usize>(&self, args: [&str; N]) -> String {
            let output = Command::new("git")
                .current_dir(self.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap()
        }
    }

    fn problem_meta(contest: &str, label: &str, title: &str) -> ProblemMeta {
        ProblemMeta {
            url: String::new(),
            contest: contest.to_owned(),
            task_id: format!("{contest}_{}", label.to_ascii_lowercase()),
            label: label.to_owned(),
            title: title.to_owned(),
            time_limit_ms: 2_000,
            interactive: false,
            tolerance: None,
        }
    }
}
