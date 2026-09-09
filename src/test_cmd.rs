use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use owo_colors::OwoColorize;
use similar::{ChangeTag, TextDiff};
use wait_timeout::ChildExt;

use crate::{config::Config, model::ProblemMeta, paths::Repository};

struct TestCase {
    name: String,
    input: PathBuf,
    expected: Option<PathBuf>,
}

struct Execution {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    elapsed: Duration,
    timed_out: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TestSummary {
    pub passed: usize,
    pub unchecked: usize,
    pub failed: usize,
}

impl TestSummary {
    pub fn require_success(self) -> Result<Self> {
        if self.failed > 0 {
            bail!("{} test case(s) failed", self.failed);
        }
        Ok(self)
    }
}

pub fn run(
    repository: &Repository,
    config: &Config,
    problem_dir: &Path,
    release: bool,
    selected_case: Option<&str>,
) -> Result<TestSummary> {
    let meta = ProblemMeta::read(problem_dir)?;
    if meta.interactive {
        println!(
            "{} interactive task; local sample judge is skipped",
            "SKIP".yellow().bold()
        );
        return Ok(TestSummary::default());
    }

    validate_test_config(config)?;
    let binary = compile(repository, config, problem_dir, &meta, release)?;
    let cases = discover_cases(problem_dir, selected_case)?;
    let timeout = Duration::from_millis(meta.time_limit_ms)
        .mul_f64(config.test.timeout_multiplier)
        .max(Duration::from_millis(config.test.minimum_timeout_ms));
    let mut passed = 0_usize;
    let mut unchecked = 0_usize;
    let mut failed = 0_usize;

    for case in &cases {
        let input = fs::read(&case.input)
            .with_context(|| format!("テスト入力を読めません: {}", case.input.display()))?;
        let execution = execute(&binary, input, timeout)?;

        if execution.timed_out {
            println!(
                "{} {:<20} > {} ms",
                "TLE".red().bold(),
                case.name,
                timeout.as_millis()
            );
            print_stderr(&execution.stderr);
            failed += 1;
            continue;
        }
        if !execution.status.success() {
            println!(
                "{} {:<20} exit={} ({:.1} ms)",
                "RE".red().bold(),
                case.name,
                execution.status,
                execution.elapsed.as_secs_f64() * 1_000.0
            );
            print_stderr(&execution.stderr);
            failed += 1;
            continue;
        }

        if let Some(expected_path) = &case.expected {
            let expected = fs::read_to_string(expected_path)
                .with_context(|| format!("期待出力を読めません: {}", expected_path.display()))?;
            if outputs_equal(&expected, &execution.stdout, meta.tolerance) {
                println!(
                    "{} {:<20} ({:.1} ms)",
                    "AC".green().bold(),
                    case.name,
                    execution.elapsed.as_secs_f64() * 1_000.0
                );
                passed += 1;
            } else {
                println!(
                    "{} {:<20} ({:.1} ms)",
                    "WA".red().bold(),
                    case.name,
                    execution.elapsed.as_secs_f64() * 1_000.0
                );
                print_diff(&expected, &execution.stdout);
                print_stderr(&execution.stderr);
                failed += 1;
            }
        } else {
            println!(
                "{} {:<20} ({:.1} ms; no .out)",
                "RUN".cyan().bold(),
                case.name,
                execution.elapsed.as_secs_f64() * 1_000.0
            );
            print!("{}", execution.stdout);
            if !execution.stdout.ends_with('\n') {
                println!();
            }
            print_stderr(&execution.stderr);
            unchecked += 1;
        }
    }

    println!(
        "{} passed, {} unchecked, {} failed",
        passed.to_string().green(),
        unchecked.to_string().cyan(),
        failed.to_string().red()
    );
    Ok(TestSummary {
        passed,
        unchecked,
        failed,
    })
}

fn validate_test_config(config: &Config) -> Result<()> {
    if !config.test.timeout_multiplier.is_finite() || config.test.timeout_multiplier <= 0.0 {
        bail!("test.timeout_multiplier は正の有限値にしてください");
    }
    if config.test.minimum_timeout_ms == 0 {
        bail!("test.minimum_timeout_ms は 1 以上にしてください");
    }
    Ok(())
}

fn compile(
    repository: &Repository,
    config: &Config,
    problem_dir: &Path,
    meta: &ProblemMeta,
    release: bool,
) -> Result<PathBuf> {
    let source = problem_dir.join("main.cpp");
    if !source.is_file() {
        bail!("解答ファイルが見つかりません: {}", source.display());
    }

    let relative = problem_dir
        .strip_prefix(&repository.root)
        .unwrap_or(problem_dir);
    let build_key = relative
        .to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let build_dir = repository.root.join(".atcli/build").join(build_key);
    fs::create_dir_all(&build_dir).with_context(|| {
        format!(
            "ビルドディレクトリを作成できません: {}",
            build_dir.display()
        )
    })?;
    let binary = build_dir.join(if release {
        "main-release"
    } else {
        "main-debug"
    });

    let mut command = Command::new(&config.cpp.compiler);
    command
        .arg(&source)
        .arg(format!("-std={}", config.cpp.standard))
        .args(if release {
            &config.cpp.release_flags
        } else {
            &config.cpp.debug_flags
        });
    for include_dir in &config.cpp.include_dirs {
        command.arg(format!("-I{}", repository.root.join(include_dir).display()));
    }
    command.arg("-o").arg(&binary);

    println!(
        "Building {} [{}]...",
        meta.task_id,
        if release { "release" } else { "debug" }
    );
    let output = command
        .output()
        .with_context(|| format!("C++ コンパイラを実行できません: {}", config.cpp.compiler))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stdout.is_empty() {
            eprint!("{stdout}");
        }
        if !stderr.is_empty() {
            eprint!("{stderr}");
        }
        bail!("C++ のコンパイルに失敗しました");
    }
    Ok(binary)
}

fn discover_cases(problem_dir: &Path, selected_case: Option<&str>) -> Result<Vec<TestCase>> {
    let tests_dir = problem_dir.join("tests");
    let mut inputs = fs::read_dir(&tests_dir)
        .with_context(|| format!("テストディレクトリを読めません: {}", tests_dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "in"))
        .collect::<Vec<_>>();
    inputs.sort();

    let selected = selected_case.map(|name| name.strip_suffix(".in").unwrap_or(name));
    let cases = inputs
        .into_iter()
        .filter_map(|input| {
            let name = input.file_stem()?.to_str()?.to_owned();
            if selected.is_some_and(|selected| selected != name) {
                return None;
            }
            let output = input.with_extension("out");
            Some(TestCase {
                name,
                input,
                expected: output.is_file().then_some(output),
            })
        })
        .collect::<Vec<_>>();

    if cases.is_empty() {
        if let Some(selected) = selected {
            bail!("テストケースが見つかりません: {selected}");
        }
        bail!("テストケースがありません: {}", tests_dir.display());
    }
    Ok(cases)
}

fn execute(binary: &Path, input: Vec<u8>, timeout: Duration) -> Result<Execution> {
    let mut child = Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("ビルドしたプログラムを実行できません: {}", binary.display()))?;
    let mut stdin = child
        .stdin
        .take()
        .context("子プロセスの stdin を取得できません")?;
    let mut stdout = child
        .stdout
        .take()
        .context("子プロセスの stdout を取得できません")?;
    let mut stderr = child
        .stderr
        .take()
        .context("子プロセスの stderr を取得できません")?;

    let input_thread = thread::spawn(move || {
        if let Err(error) = stdin.write_all(&input)
            && error.kind() != io::ErrorKind::BrokenPipe
        {
            return Err(error);
        }
        Ok(())
    });
    let stdout_thread = thread::spawn(move || {
        let mut buffer = Vec::new();
        stdout.read_to_end(&mut buffer)?;
        Ok::<_, io::Error>(buffer)
    });
    let stderr_thread = thread::spawn(move || {
        let mut buffer = Vec::new();
        stderr.read_to_end(&mut buffer)?;
        Ok::<_, io::Error>(buffer)
    });

    let started = Instant::now();
    let (status, timed_out) = if let Some(status) = child
        .wait_timeout(timeout)
        .context("実行終了を待機できません")?
    {
        (status, false)
    } else {
        child.kill().context("時間切れのプロセスを終了できません")?;
        (
            child.wait().context("終了したプロセスを回収できません")?,
            true,
        )
    };
    let elapsed = started.elapsed();

    join_io_thread(input_thread, "stdin writer")?;
    let stdout = join_io_thread(stdout_thread, "stdout reader")?;
    let stderr = join_io_thread(stderr_thread, "stderr reader")?;

    Ok(Execution {
        status,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        elapsed,
        timed_out,
    })
}

fn join_io_thread<T>(handle: thread::JoinHandle<io::Result<T>>, name: &str) -> Result<T> {
    handle
        .join()
        .map_err(|_| anyhow::anyhow!("{name} thread panicked"))?
        .with_context(|| format!("{name} thread failed"))
}

fn normalized_output(value: &str) -> String {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = normalized.lines().map(str::trim_end).collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

fn outputs_equal(expected: &str, actual: &str, tolerance: Option<f64>) -> bool {
    let expected = normalized_output(expected);
    let actual = normalized_output(actual);
    let Some(tolerance) = tolerance else {
        return expected == actual;
    };
    if !tolerance.is_finite() || tolerance < 0.0 {
        return false;
    }

    let expected_tokens = expected.split_whitespace().collect::<Vec<_>>();
    let actual_tokens = actual.split_whitespace().collect::<Vec<_>>();
    expected_tokens.len() == actual_tokens.len()
        && expected_tokens
            .iter()
            .zip(actual_tokens)
            .all(
                |(expected, actual)| match (expected.parse::<f64>(), actual.parse::<f64>()) {
                    (Ok(expected), Ok(actual)) => {
                        let difference = (expected - actual).abs();
                        difference <= tolerance
                            || difference <= tolerance * expected.abs().max(actual.abs())
                    }
                    _ => *expected == actual,
                },
            )
}

fn print_diff(expected: &str, actual: &str) {
    eprintln!("{}", "--- expected / +++ actual".dimmed());
    let expected = normalized_output(expected);
    let actual = normalized_output(actual);
    let diff = TextDiff::from_lines(&expected, &actual);
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => eprint!("{}{}", "-".red(), change.to_string().red()),
            ChangeTag::Insert => eprint!("{}{}", "+".green(), change.to_string().green()),
            ChangeTag::Equal => eprint!(" {change}"),
        }
    }
    if !expected.is_empty() || !actual.is_empty() {
        eprintln!();
    }
}

fn print_stderr(stderr: &str) {
    if !stderr.is_empty() {
        eprintln!("{}", "stderr:".dimmed());
        eprint!("{stderr}");
        if !stderr.ends_with('\n') {
            eprintln!();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{normalized_output, outputs_equal};

    #[test]
    fn ignores_line_endings_trailing_spaces_and_final_blank_lines() {
        assert_eq!(normalized_output("a  \r\nb\r\n\r\n"), "a\nb");
        assert!(outputs_equal("a  \nb\n", "a\nb\n\n", None));
    }

    #[test]
    fn compares_numeric_tokens_with_tolerance() {
        assert!(outputs_equal("answer 1.0", "answer 1.0000001", Some(1e-6)));
        assert!(!outputs_equal("answer 1.0", "answer 1.1", Some(1e-6)));
    }
}
