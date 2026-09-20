use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use wait_timeout::ChildExt;

use crate::diagnose::{Diagnostics, SourceLocation, crash_source_location, find_source_location};

/// One `.in` file and, when present, the `.out` file beside it.
#[derive(Clone, Debug)]
pub struct TestCase {
    pub name: String,
    pub input: PathBuf,
    pub expected: Option<PathBuf>,
}

/// How to judge a single case.
#[derive(Clone, Copy, Debug)]
pub struct JudgeSettings {
    pub timeout: Duration,
    /// Compare numeric tokens within this absolute or relative error.
    pub tolerance: Option<f64>,
    pub diagnostics: Diagnostics,
}

/// What became of one test case.
#[derive(Clone, Debug)]
pub struct CaseResult {
    pub name: String,
    pub elapsed: Duration,
    pub stderr: String,
    pub outcome: Outcome,
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Accepted,
    WrongAnswer {
        expected: String,
        actual: String,
    },
    RuntimeError {
        status: ExitStatus,
        location: Option<SourceLocation>,
    },
    TimeLimitExceeded,
    /// Ran to completion, but there was no `.out` file to compare against.
    Unchecked {
        stdout: String,
    },
}

impl Outcome {
    #[must_use]
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::WrongAnswer { .. } | Self::RuntimeError { .. } | Self::TimeLimitExceeded
        )
    }
}

/// Running counts over a set of [`CaseResult`]s.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Summary {
    pub passed: usize,
    pub unchecked: usize,
    pub failed: usize,
}

impl Summary {
    pub fn record(&mut self, outcome: &Outcome) {
        match outcome {
            Outcome::Accepted => self.passed += 1,
            Outcome::Unchecked { .. } => self.unchecked += 1,
            _ => self.failed += 1,
        }
    }

    #[must_use]
    pub fn is_success(&self) -> bool {
        self.failed == 0
    }
}

/// Collect the `.in` files in `tests_dir`, optionally keeping only `selected`.
///
/// # Errors
///
/// Returns an error when the directory cannot be read or matches no case.
pub fn discover_cases(tests_dir: &Path, selected: Option<&str>) -> Result<Vec<TestCase>> {
    let mut inputs = fs::read_dir(tests_dir)
        .with_context(|| format!("テストディレクトリを読めません: {}", tests_dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "in"))
        .collect::<Vec<_>>();
    inputs.sort();

    let selected = selected.map(|name| name.strip_suffix(".in").unwrap_or(name));
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

/// Run `binary` against one case and classify the result.
///
/// `source` is only used to attribute a crash to a line, so it may be missing.
///
/// # Errors
///
/// Returns an error when the case files or the binary cannot be used.
pub fn judge(
    binary: &Path,
    source: &Path,
    case: &TestCase,
    settings: &JudgeSettings,
) -> Result<CaseResult> {
    let input = fs::read(&case.input)
        .with_context(|| format!("テスト入力を読めません: {}", case.input.display()))?;
    let execution = execute(binary, input, settings.timeout)?;
    let stderr = execution.stderr;

    let outcome = if execution.timed_out {
        Outcome::TimeLimitExceeded
    } else if execution.status.success() {
        match &case.expected {
            None => Outcome::Unchecked {
                stdout: execution.stdout,
            },
            Some(expected_path) => {
                let expected = fs::read_to_string(expected_path).with_context(|| {
                    format!("期待出力を読めません: {}", expected_path.display())
                })?;
                if outputs_equal(&expected, &execution.stdout, settings.tolerance) {
                    Outcome::Accepted
                } else {
                    Outcome::WrongAnswer {
                        expected,
                        actual: execution.stdout,
                    }
                }
            }
        }
    } else {
        Outcome::RuntimeError {
            status: execution.status,
            location: locate_crash(&stderr, binary, source, case, execution.status, settings),
        }
    };

    Ok(CaseResult {
        name: case.name.clone(),
        elapsed: execution.elapsed,
        stderr,
        outcome,
    })
}

/// Sanitizers name the faulting line on stderr, so look there first: it costs
/// nothing and works however the process ended. Only a signal death that stayed
/// silent is worth paying for a debugger re-run.
fn locate_crash(
    stderr: &str,
    binary: &Path,
    source: &Path,
    case: &TestCase,
    status: ExitStatus,
    settings: &JudgeSettings,
) -> Option<SourceLocation> {
    if settings.diagnostics == Diagnostics::Off {
        return None;
    }
    find_source_location(stderr, source).or_else(|| {
        let silent_crash = status.code().is_none() && settings.diagnostics == Diagnostics::Debugger;
        silent_crash
            .then(|| crash_source_location(binary, &case.input, source, settings.timeout))
            .flatten()
    })
}

struct Execution {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    elapsed: Duration,
    timed_out: bool,
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

/// Drop trailing whitespace and blank lines so that cosmetic differences do not
/// fail a case.
#[must_use]
pub fn normalized_output(value: &str) -> String {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = normalized.lines().map(str::trim_end).collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Compare two outputs, treating numeric tokens as equal when they agree within
/// `tolerance` in absolute or relative terms.
#[must_use]
pub fn outputs_equal(expected: &str, actual: &str, tolerance: Option<f64>) -> bool {
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
