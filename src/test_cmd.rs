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

use crate::{
    config::Config,
    model::ProblemMeta,
    paths::{Attempt, Repository},
};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SourceLocation {
    line: usize,
    column: Option<usize>,
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
    attempt: &Attempt,
    release: bool,
    selected_case: Option<&str>,
    rebuild: bool,
) -> Result<TestSummary> {
    let meta = ProblemMeta::read(&attempt.problem_dir)?;
    if meta.interactive {
        println!(
            "{} interactive task; local sample judge is skipped",
            "SKIP".yellow().bold()
        );
        return Ok(TestSummary::default());
    }

    validate_test_config(config)?;
    let binary = compile(repository, config, &attempt.dir, &meta, release, rebuild)?;
    let cases = discover_cases(&attempt.problem_dir, selected_case)?;
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
            let source = attempt.dir.join("main.cpp");
            if !release && execution.status.code().is_none() {
                let location = find_source_location(&execution.stderr, &source)
                    .or_else(|| crash_source_location(&binary, &case.input, &source, timeout));
                if let Some(location) = location {
                    print_source_location(repository, &source, location);
                }
            }
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
    attempt_dir: &Path,
    meta: &ProblemMeta,
    release: bool,
    rebuild: bool,
) -> Result<PathBuf> {
    let source = attempt_dir.join("main.cpp");
    if !source.is_file() {
        bail!("解答ファイルが見つかりません: {}", source.display());
    }

    let relative = attempt_dir
        .strip_prefix(&repository.root)
        .with_context(|| {
            format!(
                "取り組みディレクトリはリポジトリ内に置いてください: {}",
                attempt_dir.display()
            )
        })?;
    let build_dir = repository.root.join(".atcli/build").join(relative);
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
    let depfile = binary.with_extension("d");
    let fingerprint_file = binary.with_extension("fingerprint");

    let command_signature = compiler_signature(repository, config, &source, release)?;
    if !rebuild && cache_is_fresh(&binary, &depfile, &fingerprint_file, &command_signature) {
        println!(
            "Cached {} [{}]",
            meta.task_id,
            if release { "release" } else { "debug" }
        );
        return Ok(binary);
    }

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
    command.arg("-MMD").arg("-MF").arg(&depfile);
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
    if let Ok(dependencies) = read_dependencies(&depfile) {
        let fingerprint = fingerprint(&command_signature, &dependencies)?;
        fs::write(&fingerprint_file, format!("{fingerprint:016x}\n")).with_context(|| {
            format!(
                "ビルドキャッシュ情報を書き込めません: {}",
                fingerprint_file.display()
            )
        })?;
    }
    Ok(binary)
}

fn compiler_signature(
    repository: &Repository,
    config: &Config,
    source: &Path,
    release: bool,
) -> Result<String> {
    let version = Command::new(&config.cpp.compiler)
        .arg("--version")
        .output()
        .with_context(|| format!("C++ コンパイラを実行できません: {}", config.cpp.compiler))?;
    let flags = if release {
        &config.cpp.release_flags
    } else {
        &config.cpp.debug_flags
    };
    Ok(format!(
        "compiler={}\nversion={}\nsource={}\nstandard={}\nflags={flags:?}\nincludes={:?}\n",
        config.cpp.compiler,
        String::from_utf8_lossy(&version.stdout),
        source.display(),
        config.cpp.standard,
        config
            .cpp
            .include_dirs
            .iter()
            .map(|path| repository.root.join(path))
            .collect::<Vec<_>>()
    ))
}

fn cache_is_fresh(
    binary: &Path,
    depfile: &Path,
    fingerprint_file: &Path,
    command_signature: &str,
) -> bool {
    if !binary.is_file() || !fingerprint_file.is_file() {
        return false;
    }
    let Ok(dependencies) = read_dependencies(depfile) else {
        return false;
    };
    let Ok(current) = fingerprint(command_signature, &dependencies) else {
        return false;
    };
    fs::read_to_string(fingerprint_file)
        .is_ok_and(|stored| stored.trim() == format!("{current:016x}"))
}

fn read_dependencies(depfile: &Path) -> Result<Vec<PathBuf>> {
    let contents = fs::read_to_string(depfile)
        .with_context(|| format!("依存ファイルを読めません: {}", depfile.display()))?;
    parse_dependencies(&contents)
}

fn parse_dependencies(contents: &str) -> Result<Vec<PathBuf>> {
    let contents = contents.replace("\\\r\n", " ").replace("\\\n", " ");
    let (_, dependencies) = contents
        .split_once(':')
        .context("依存ファイルの形式が不正です")?;
    let mut paths = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in dependencies.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character.is_whitespace() {
            if !current.is_empty() {
                paths.push(PathBuf::from(std::mem::take(&mut current)));
            }
        } else {
            current.push(character);
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        paths.push(PathBuf::from(current));
    }
    if paths.is_empty() {
        bail!("依存ファイルに入力がありません");
    }
    Ok(paths)
}

fn fingerprint(command_signature: &str, dependencies: &[PathBuf]) -> Result<u64> {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    hash_bytes(&mut hash, command_signature.as_bytes());
    for dependency in dependencies {
        hash_bytes(&mut hash, dependency.as_os_str().as_encoded_bytes());
        let contents = fs::read(dependency)
            .with_context(|| format!("依存ファイルを読めません: {}", dependency.display()))?;
        hash_bytes(&mut hash, &contents);
    }
    Ok(hash)
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
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
            ChangeTag::Delete => eprint!("{} {}", "-".red(), change.to_string().red()),
            ChangeTag::Insert => eprint!("{} {}", "+".green(), change.to_string().green()),
            ChangeTag::Equal => eprint!("  {change}"),
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

fn find_source_location(output: &str, source: &Path) -> Option<SourceLocation> {
    let file_name = source.file_name()?.to_str()?;
    let marker = format!("{file_name}:");
    output.lines().find_map(|line| {
        let (_, suffix) = line.rsplit_once(&marker)?;
        let mut parts = suffix.split(':');
        let line = parts.next()?.parse().ok()?;
        let column = parts
            .next()
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
            .filter(|column| *column > 0);
        (line > 0).then_some(SourceLocation { line, column })
    })
}

fn print_source_location(repository: &Repository, source: &Path, location: SourceLocation) {
    let display_path = source
        .strip_prefix(&repository.root)
        .unwrap_or(source)
        .display();
    let column = location
        .column
        .map_or_else(String::new, |column| format!(":{column}"));
    eprintln!("{}", "source:".dimmed());
    eprintln!("  {display_path}:{}{column}", location.line);

    let Ok(contents) = fs::read_to_string(source) else {
        return;
    };
    let Some(source_line) = contents.lines().nth(location.line - 1) else {
        return;
    };
    let number_width = location.line.to_string().len();
    eprintln!("  {:>number_width$} | {source_line}", location.line);
    if let Some(column) = location.column {
        let padding = source_line
            .chars()
            .take(column.saturating_sub(1))
            .map(|character| if character == '\t' { '\t' } else { ' ' })
            .collect::<String>();
        eprintln!("  {:>number_width$} | {padding}{}", "", "^".red().bold());
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn crash_source_location(
    binary: &Path,
    input: &Path,
    source: &Path,
    test_timeout: Duration,
) -> Option<SourceLocation> {
    let mut command = debugger_command(binary, input);
    let diagnostic_timeout = test_timeout.min(Duration::from_secs(2)) + Duration::from_secs(3);
    let output = capture_command(&mut command, diagnostic_timeout)?;
    find_source_location(&output, source)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn crash_source_location(
    _binary: &Path,
    _input: &Path,
    _source: &Path,
    _test_timeout: Duration,
) -> Option<SourceLocation> {
    None
}

#[cfg(target_os = "macos")]
fn debugger_command(binary: &Path, input: &Path) -> Command {
    let input_command = format!(
        "settings set target.input-path {}",
        quote_lldb_argument(input)
    );
    let mut command = Command::new("lldb");
    command
        .arg("--batch")
        .arg("--no-lldbinit")
        .arg("--source-quietly")
        .arg("--no-use-colors")
        .arg("-o")
        .arg(input_command)
        .arg("-o")
        .arg("run")
        .arg("-k")
        .arg("bt 16")
        .arg("-k")
        .arg("process kill")
        .arg("-k")
        .arg("quit")
        .arg(binary);
    command
}

#[cfg(target_os = "linux")]
fn debugger_command(binary: &Path, input: &Path) -> Command {
    let run_command = format!("run < {}", quote_shell_argument(input));
    let mut command = Command::new("gdb");
    command
        .arg("--batch")
        .arg("--quiet")
        .arg("-ex")
        .arg("set pagination off")
        .arg("-ex")
        .arg(run_command)
        .arg("-ex")
        .arg("bt 16")
        .arg("--args")
        .arg(binary);
    command
}

#[cfg(target_os = "macos")]
fn quote_lldb_argument(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

#[cfg(target_os = "linux")]
fn quote_shell_argument(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn capture_command(command: &mut Command, timeout: Duration) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let stdout_thread = thread::spawn(move || {
        let mut buffer = Vec::new();
        stdout.read_to_end(&mut buffer).map(|_| buffer)
    });
    let stderr_thread = thread::spawn(move || {
        let mut buffer = Vec::new();
        stderr.read_to_end(&mut buffer).map(|_| buffer)
    });

    let finished = child.wait_timeout(timeout).ok()?.is_some();
    if !finished {
        let _ = child.kill();
        let _ = child.wait();
    }
    let mut output = stdout_thread.join().ok()?.ok()?;
    output.extend(stderr_thread.join().ok()?.ok()?);
    Some(String::from_utf8_lossy(&output).into_owned())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use super::{
        SourceLocation, cache_is_fresh, find_source_location, fingerprint, normalized_output,
        outputs_equal, parse_dependencies,
    };

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

    #[test]
    fn parses_makefile_dependencies_with_continuations_and_spaces() {
        let depfile = concat!("main: /tmp/main.cpp \\", "\n", " /tmp/local\\ header.hpp\n");
        let dependencies = parse_dependencies(depfile).unwrap();
        assert_eq!(
            dependencies,
            [
                PathBuf::from("/tmp/main.cpp"),
                PathBuf::from("/tmp/local header.hpp"),
            ]
        );
    }

    #[test]
    fn invalidates_cache_when_dependency_or_command_changes() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("main.cpp");
        let header = temp.path().join("local.hpp");
        let binary = temp.path().join("main");
        let depfile = temp.path().join("main.d");
        let fingerprint_file = temp.path().join("main.fingerprint");
        fs::write(&source, "#include \"local.hpp\"\n").unwrap();
        fs::write(&header, "constexpr int answer = 42;\n").unwrap();
        fs::write(&binary, "binary").unwrap();
        fs::write(
            &depfile,
            format!("main: {} {}\n", source.display(), header.display()),
        )
        .unwrap();
        let dependencies = [source, header.clone()];
        let current = fingerprint("g++ -O0", &dependencies).unwrap();
        fs::write(&fingerprint_file, format!("{current:016x}\n")).unwrap();

        assert!(cache_is_fresh(
            &binary,
            &depfile,
            &fingerprint_file,
            "g++ -O0"
        ));
        assert!(!cache_is_fresh(
            &binary,
            &depfile,
            &fingerprint_file,
            "g++ -O2"
        ));

        fs::write(header, "constexpr int answer = 43;\n").unwrap();
        assert!(!cache_is_fresh(
            &binary,
            &depfile,
            &fingerprint_file,
            "g++ -O0"
        ));
    }

    #[test]
    fn extracts_source_location_from_lldb_and_gdb_backtraces() {
        let source = PathBuf::from("/work/attempt/main.cpp");
        let lldb = "frame #5: binary`main at main.cpp:15:34\n";
        let gdb = "#5  main () at /work/attempt/main.cpp:27\n";

        assert_eq!(
            find_source_location(lldb, &source),
            Some(SourceLocation {
                line: 15,
                column: Some(34),
            })
        );
        assert_eq!(
            find_source_location(gdb, &source),
            Some(SourceLocation {
                line: 27,
                column: None,
            })
        );
    }
}
