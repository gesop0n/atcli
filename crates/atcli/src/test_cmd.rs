use std::{path::Path, time::Duration};

use anyhow::{Context, Result, bail};
use atcli_core::{
    config::Config,
    model::ProblemMeta,
    paths::{Attempt, Repository},
};
use atcli_judge::{
    BuildRequest, BuildSettings, CaseResult, CompileError, Diagnostics, JudgeSettings, Outcome,
    SourceLocation, Summary, build, discover_cases, judge, normalized_output,
};
use owo_colors::OwoColorize;
use similar::{ChangeTag, TextDiff};

pub fn run(
    repository: &Repository,
    config: &Config,
    attempt: &Attempt,
    release: bool,
    selected_case: Option<&str>,
    rebuild: bool,
) -> Result<Summary> {
    let meta = ProblemMeta::read(&attempt.problem_dir)?;
    if meta.interactive {
        println!(
            "{} interactive task; local sample judge is skipped",
            "SKIP".yellow().bold()
        );
        return Ok(Summary::default());
    }

    validate_test_config(config)?;
    let source = attempt.dir.join("main.cpp");
    let binary = build_solution(repository, config, attempt, &meta, release, rebuild)?;
    let cases = discover_cases(&attempt.problem_dir.join("tests"), selected_case)?;
    let settings = JudgeSettings {
        timeout: Duration::from_millis(meta.time_limit_ms)
            .mul_f64(config.test.timeout_multiplier)
            .max(Duration::from_millis(config.test.minimum_timeout_ms)),
        tolerance: meta.tolerance,
        diagnostics: diagnostics(config, release),
    };

    let mut summary = Summary::default();
    for case in &cases {
        let result = judge(&binary, &source, case, &settings)?;
        summary.record(&result.outcome);
        report(repository, &source, &result, settings.timeout);
    }

    println!(
        "{} passed, {} unchecked, {} failed",
        summary.passed.to_string().green(),
        summary.unchecked.to_string().cyan(),
        summary.failed.to_string().red()
    );
    Ok(summary)
}

/// Turn a finished run into a command exit status.
///
/// # Errors
///
/// Returns an error when any case failed.
pub fn require_success(summary: Summary) -> Result<Summary> {
    if !summary.is_success() {
        bail!("{} test case(s) failed", summary.failed);
    }
    Ok(summary)
}

/// `--release` mirrors what `AtCoder` runs, so it gets no sanitizers and no
/// debugger. Otherwise a silent crash is worth a debugger re-run unless the
/// repository turned that off.
fn diagnostics(config: &Config, release: bool) -> Diagnostics {
    if release {
        Diagnostics::Off
    } else if config.test.crash_diagnostics {
        Diagnostics::Debugger
    } else {
        Diagnostics::Output
    }
}

fn build_solution(
    repository: &Repository,
    config: &Config,
    attempt: &Attempt,
    meta: &ProblemMeta,
    release: bool,
    rebuild: bool,
) -> Result<std::path::PathBuf> {
    let relative = attempt
        .dir
        .strip_prefix(&repository.root)
        .with_context(|| {
            format!(
                "取り組みディレクトリはリポジトリ内に置いてください: {}",
                attempt.dir.display()
            )
        })?;
    let settings = BuildSettings {
        compiler: config.cpp.compiler.clone(),
        standard: config.cpp.standard.clone(),
        include_dirs: config
            .cpp
            .include_dirs
            .iter()
            .map(|path| repository.root.join(path))
            .collect(),
        flags: if release {
            config.cpp.release_flags.clone()
        } else {
            config.cpp.debug_flags.clone()
        },
    };
    let build_dir = repository.root.join(".atcli/build").join(relative);
    let profile = if release { "release" } else { "debug" };
    let source = attempt.dir.join("main.cpp");
    let request = BuildRequest {
        source: &source,
        build_dir: &build_dir,
        binary_name: if release {
            "main-release"
        } else {
            "main-debug"
        },
        settings: &settings,
        rebuild,
    };

    match build(&request) {
        Ok(outcome) => {
            let verb = if outcome.cached { "Cached" } else { "Built" };
            println!("{verb} {} [{profile}]", meta.task_id);
            Ok(outcome.binary)
        }
        Err(error) => {
            if let Some(failure) = error.downcast_ref::<CompileError>() {
                if !failure.stdout.is_empty() {
                    eprint!("{}", failure.stdout);
                }
                if !failure.stderr.is_empty() {
                    eprint!("{}", failure.stderr);
                }
            }
            Err(error)
        }
    }
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

fn report(repository: &Repository, source: &Path, result: &CaseResult, timeout: Duration) {
    let millis = result.elapsed.as_secs_f64() * 1_000.0;
    match &result.outcome {
        Outcome::TimeLimitExceeded => println!(
            "{} {:<20} > {} ms",
            "TLE".red().bold(),
            result.name,
            timeout.as_millis()
        ),
        Outcome::RuntimeError { status, location } => {
            println!(
                "{} {:<20} exit={status} ({millis:.1} ms)",
                "RE".red().bold(),
                result.name,
            );
            if let Some(location) = location {
                print_source_location(repository, source, *location);
            }
        }
        Outcome::Accepted => println!(
            "{} {:<20} ({millis:.1} ms)",
            "AC".green().bold(),
            result.name
        ),
        Outcome::WrongAnswer { expected, actual } => {
            println!("{} {:<20} ({millis:.1} ms)", "WA".red().bold(), result.name);
            print_diff(expected, actual);
        }
        Outcome::Unchecked { stdout } => {
            println!(
                "{} {:<20} ({millis:.1} ms; no .out)",
                "RUN".cyan().bold(),
                result.name,
            );
            print!("{stdout}");
            if !stdout.ends_with('\n') {
                println!();
            }
        }
    }
    print_stderr(&result.stderr);
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

    let Ok(contents) = std::fs::read_to_string(source) else {
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
