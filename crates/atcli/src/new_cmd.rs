use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use chrono::{Datelike, Local, NaiveDate};

use atcli_atcoder::client::{AtCoderClient, replace_samples};
use atcli_core::{
    config::Config,
    model::{AttemptMeta, ContestTask, ProblemMeta},
    paths::{Repository, normalize_directory_component},
};

/// 問題指定を省略して全問題を作成できる上限。`ABC` などの通常コンテストはいずれも下回る。
const MAX_IMPLICIT_TASKS: usize = 20;
/// エラー文に並べる選択可能なラベルの上限。
const MAX_LISTED_LABELS: usize = 20;

/// 作成したコンテストディレクトリを返す。
///
/// `quiet` を立てると進捗表示を stderr へ回す。`--cd` のとき stdout を移動先のパス専用にして、
/// shell 側が `$( )` でそのまま受け取れるようにするため。
pub fn run(
    repository: &Repository,
    config: &Config,
    contest: &str,
    requested_problems: &[String],
    requested_date: Option<&str>,
    all: bool,
    quiet: bool,
) -> Result<PathBuf> {
    let contest = normalize_contest_id(contest)?;
    let date = requested_date
        .map(parse_date)
        .transpose()?
        .unwrap_or_else(|| Local::now().date_naive());
    let attempts_root = repository.root.join(&config.repository.attempts_dir);
    let problems_root = repository.root.join(&config.repository.problems_dir);
    let destination = attempts_root
        .join(format!("{:04}", date.year()))
        .join(format!("{:02}", date.month()))
        .join(format!("{:02}", date.day()))
        .join(&contest);
    let template_path = repository.root.join(&config.repository.template);
    let template = fs::read_to_string(&template_path)
        .with_context(|| format!("C++ テンプレートを読めません: {}", template_path.display()))?;

    let client = AtCoderClient::with_saved_session()?;
    report(quiet, &format!("Fetching {contest} task list..."));
    let tasks = client.contest_tasks(&contest)?;
    let tasks = select_tasks(&tasks, requested_problems, all)?;

    for (index, task) in tasks.iter().enumerate() {
        let directory_name = task_directory_name(&task.label, &task.task_id)?;
        let problem_relative = PathBuf::from(&contest).join(&directory_name);
        let problem_dir = problems_root.join(&problem_relative);
        let attempt_dir = destination.join(&directory_name);

        let (sample_count, interactive, reused) = if problem_dir.join("meta.toml").is_file() {
            let meta = ProblemMeta::read(&problem_dir)?;
            if meta.contest != contest || meta.task_id != task.task_id {
                bail!(
                    "既存の問題データが別の問題を指しています: {}",
                    problem_dir.display()
                );
            }
            (count_samples(&problem_dir)?, meta.interactive, true)
        } else {
            fs::create_dir_all(problem_dir.join("tests")).with_context(|| {
                format!(
                    "問題データディレクトリを作成できません: {}",
                    problem_dir.display()
                )
            })?;
            let page = client.task_page(&task.url)?;
            let meta = ProblemMeta {
                url: task.url.clone(),
                contest: contest.clone(),
                task_id: task.task_id.clone(),
                label: task.label.clone(),
                title: task.title.clone(),
                time_limit_ms: page.time_limit_ms,
                interactive: page.interactive,
                tolerance: None,
            };
            replace_samples(&problem_dir, &page.samples)?;
            meta.write(&problem_dir)?;
            (page.samples.len(), page.interactive, false)
        };

        fs::create_dir_all(&attempt_dir).with_context(|| {
            format!(
                "取り組みディレクトリを作成できません: {}",
                attempt_dir.display()
            )
        })?;
        let attempt = AttemptMeta {
            problem: problem_relative,
        };
        write_attempt_meta(&attempt_dir, &attempt)?;
        let main_cpp = attempt_dir.join("main.cpp");
        if !main_cpp.exists() {
            fs::write(&main_cpp, &template)
                .with_context(|| format!("テンプレートを書き込めません: {}", main_cpp.display()))?;
        }

        report(
            quiet,
            &format!(
                "  {:>3}  {} ({} samples{}{})",
                task.label,
                attempt_dir.display(),
                sample_count,
                if interactive { ", interactive" } else { "" },
                if reused { ", reused problem data" } else { "" }
            ),
        );
        if index + 1 != tasks.len() {
            thread::sleep(Duration::from_millis(200));
        }
    }

    report(quiet, &format!("Created {}", destination.display()));
    Ok(destination)
}

fn report(quiet: bool, line: &str) {
    if quiet {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}

fn write_attempt_meta(attempt_dir: &Path, expected: &AttemptMeta) -> Result<()> {
    if attempt_dir.join("attempt.toml").is_file() {
        let existing = AttemptMeta::read(attempt_dir)?;
        if existing != *expected {
            bail!(
                "既存の attempt.toml が別の問題を指しています: {}",
                attempt_dir.display()
            );
        }
        return Ok(());
    }
    expected.write(attempt_dir)
}

fn count_samples(problem_dir: &Path) -> Result<usize> {
    let tests_dir = problem_dir.join("tests");
    let count = fs::read_dir(&tests_dir)
        .with_context(|| format!("テストディレクトリを読めません: {}", tests_dir.display()))?
        .filter_map(Result::ok)
        .filter(|entry| {
            let path = entry.path();
            path.extension().is_some_and(|extension| extension == "in")
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.starts_with("sample-"))
        })
        .count();
    Ok(count)
}

fn select_tasks<'a>(
    tasks: &'a [ContestTask],
    requested_problems: &[String],
    all: bool,
) -> Result<Vec<&'a ContestTask>> {
    if requested_problems.is_empty() {
        if !all && tasks.len() > MAX_IMPLICIT_TASKS {
            bail!(
                "問題数が多いコンテストです（{} 問）。作成する問題を指定するか、全問題を作成する場合は --all を指定してください（例: a01..a05）",
                tasks.len()
            );
        }
        return Ok(tasks.iter().collect());
    }

    // 添字の集合として持つことで、重複排除とコンテスト順の復元を同時に行う。
    let mut selected = BTreeSet::new();
    for requested in requested_problems {
        let selector = requested.trim();
        if let Some((start, end)) = selector.split_once("..") {
            let start_index = resolve_index(tasks, start)?;
            let end_index = resolve_index(tasks, end)?;
            if start_index > end_index {
                bail!("範囲の開始と終了が逆です: {selector}");
            }
            selected.extend(start_index..=end_index);
        } else {
            selected.insert(resolve_index(tasks, selector)?);
        }
    }

    Ok(selected.into_iter().map(|index| &tasks[index]).collect())
}

/// ラベルまたは問題 ID から、コンテストの問題一覧における位置を求める。
///
/// 範囲指定をラベルの文字列計算ではなく位置で解決することで、`A77` から `B01` のような
/// 区分をまたぐ範囲も、他コンテスト由来の問題 ID が混ざる場合も、ラベル体系を仮定せずに扱える。
fn resolve_index(tasks: &[ContestTask], selector: &str) -> Result<usize> {
    let selector = selector.trim();
    if selector.is_empty() {
        bail!("問題の指定を空にはできません");
    }
    tasks
        .iter()
        .position(|task| {
            task.label.eq_ignore_ascii_case(selector) || task.task_id.eq_ignore_ascii_case(selector)
        })
        .with_context(|| {
            format!(
                "指定された問題が見つかりません: {selector}（選択可能: {}）",
                available_labels(tasks)
            )
        })
}

fn available_labels(tasks: &[ContestTask]) -> String {
    let listed = tasks
        .iter()
        .take(MAX_LISTED_LABELS)
        .map(|task| task.label.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if tasks.len() > MAX_LISTED_LABELS {
        format!("{listed}, ...他 {} 件", tasks.len() - MAX_LISTED_LABELS)
    } else {
        listed
    }
}

fn normalize_contest_id(value: &str) -> Result<String> {
    normalize_directory_component(value)
        .with_context(|| format!("不正なコンテスト ID です: {value}"))
}

fn parse_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("日付は YYYY-MM-DD 形式で指定してください: {value}"))
}

fn task_directory_name(label: &str, task_id: &str) -> Result<String> {
    if let Some(name) = normalize_directory_component(label) {
        return Ok(name);
    }

    let fallback = task_id.rsplit('_').next().unwrap_or(task_id);
    normalize_directory_component(fallback).with_context(|| {
        format!(
            "問題ラベルから安全なディレクトリ名を作れません: {}",
            label.trim().to_ascii_lowercase()
        )
    })
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use atcli_core::model::{AttemptMeta, ContestTask};

    use super::{
        MAX_IMPLICIT_TASKS, MAX_LISTED_LABELS, normalize_contest_id, parse_date, select_tasks,
        task_directory_name, write_attempt_meta,
    };

    fn task(label: &str, task_id: &str) -> ContestTask {
        ContestTask {
            label: label.to_owned(),
            task_id: task_id.to_owned(),
            title: String::new(),
            url: String::new(),
        }
    }

    fn numbered_tasks(count: usize) -> Vec<ContestTask> {
        (0..count)
            .map(|index| task(&format!("A{index:02}"), &format!("tessoku_book_{index}")))
            .collect()
    }

    fn labels<'a>(tasks: &[&'a ContestTask]) -> Vec<&'a str> {
        tasks.iter().map(|task| task.label.as_str()).collect()
    }

    #[test]
    fn validates_contest_id() {
        assert_eq!(normalize_contest_id(" ABC300 ").unwrap(), "abc300");
        assert!(normalize_contest_id("../../tmp").is_err());
    }

    #[test]
    fn accepts_explicit_date() {
        assert_eq!(parse_date("2026-09-08").unwrap().to_string(), "2026-09-08");
        assert!(parse_date("2026/09/08").is_err());
    }

    #[test]
    fn derives_problem_directory_name() {
        assert_eq!(task_directory_name("Ex", "abc300_h").unwrap(), "ex");
        assert_eq!(task_directory_name("問題A", "abc300_a").unwrap(), "a");
    }

    #[test]
    fn selects_all_tasks_when_no_problem_is_requested() {
        let tasks = [task("A", "abc300_a"), task("B", "abc300_b")];
        let selected = select_tasks(&tasks, &[], false).unwrap();

        assert_eq!(selected, tasks.iter().collect::<Vec<_>>());
    }

    #[test]
    fn selects_requested_tasks_by_label_or_task_id() {
        let tasks = [
            task("A", "abc300_a"),
            task("B", "abc300_b"),
            task("Ex", "abc300_h"),
        ];
        let requested = ["ex".to_owned(), "ABC300_A".to_owned()];
        let selected = select_tasks(&tasks, &requested, false).unwrap();

        assert_eq!(labels(&selected), ["A", "Ex"]);
    }

    #[test]
    fn rejects_unknown_requested_tasks() {
        let tasks = [task("A", "abc300_a"), task("B", "abc300_b")];
        let error = select_tasks(&tasks, &["c".to_owned()], false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "指定された問題が見つかりません: c（選択可能: A, B）"
        );
    }

    #[test]
    fn expands_label_ranges() {
        let tasks = numbered_tasks(5);
        let selected = select_tasks(&tasks, &["a01..a03".to_owned()], false).unwrap();

        assert_eq!(labels(&selected), ["A01", "A02", "A03"]);
    }

    #[test]
    fn expands_ranges_across_label_sections() {
        let tasks = [
            task("A76", "tessoku_book_bx"),
            task("A77", "typical90_a"),
            task("B01", "tessoku_book_by"),
            task("B02", "tessoku_book_bz"),
            task("B03", "tessoku_book_ca"),
        ];
        let selected = select_tasks(&tasks, &["a77..b02".to_owned()], false).unwrap();

        assert_eq!(labels(&selected), ["A77", "B01", "B02"]);
    }

    #[test]
    fn resolves_range_endpoints_by_task_id() {
        let tasks = [
            task("A76", "tessoku_book_bx"),
            task("A77", "typical90_a"),
            task("B01", "tessoku_book_by"),
        ];
        let selected = select_tasks(&tasks, &["typical90_a..b01".to_owned()], false).unwrap();

        assert_eq!(labels(&selected), ["A77", "B01"]);
    }

    #[test]
    fn merges_overlapping_selectors_in_contest_order() {
        let tasks = [
            task("A", "abc300_a"),
            task("B", "abc300_b"),
            task("C", "abc300_c"),
        ];
        let requested = ["c".to_owned(), "a..b".to_owned(), "B".to_owned()];
        let selected = select_tasks(&tasks, &requested, false).unwrap();

        assert_eq!(labels(&selected), ["A", "B", "C"]);
    }

    #[test]
    fn rejects_reversed_range() {
        let tasks = numbered_tasks(5);
        let error = select_tasks(&tasks, &["a03..a01".to_owned()], false).unwrap_err();

        assert_eq!(error.to_string(), "範囲の開始と終了が逆です: a03..a01");
    }

    #[test]
    fn rejects_range_with_unknown_endpoint() {
        let tasks = numbered_tasks(3);
        let error = select_tasks(&tasks, &["a01..zz9".to_owned()], false).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("指定された問題が見つかりません: zz9")
        );
    }

    #[test]
    fn rejects_empty_range_endpoint() {
        let tasks = numbered_tasks(3);
        let error = select_tasks(&tasks, ["..".to_owned()].as_slice(), false).unwrap_err();

        assert_eq!(error.to_string(), "問題の指定を空にはできません");
    }

    #[test]
    fn requires_explicit_selection_for_large_contests() {
        let tasks = numbered_tasks(MAX_IMPLICIT_TASKS + 1);

        let error = select_tasks(&tasks, &[], false).unwrap_err();
        assert!(error.to_string().contains("--all"));
        assert_eq!(select_tasks(&tasks, &[], true).unwrap().len(), tasks.len());
    }

    #[test]
    fn keeps_creating_every_task_for_small_contests() {
        let tasks = numbered_tasks(MAX_IMPLICIT_TASKS);

        assert_eq!(select_tasks(&tasks, &[], false).unwrap().len(), tasks.len());
    }

    #[test]
    fn truncates_available_labels_in_error() {
        let tasks = numbered_tasks(MAX_LISTED_LABELS + 10);

        let message = select_tasks(&tasks, &["zz9".to_owned()], false)
            .unwrap_err()
            .to_string();

        assert!(message.contains("...他 10 件"));
        assert!(!message.contains("A29"));
    }

    #[test]
    fn creates_idempotent_attempt_reference() {
        let temp = tempdir().unwrap();
        let expected = AttemptMeta {
            problem: "abc300/a".into(),
        };

        write_attempt_meta(temp.path(), &expected).unwrap();
        write_attempt_meta(temp.path(), &expected).unwrap();
        assert_eq!(AttemptMeta::read(temp.path()).unwrap(), expected);

        let other = AttemptMeta {
            problem: "abc300/b".into(),
        };
        assert!(write_attempt_meta(temp.path(), &other).is_err());
    }
}
