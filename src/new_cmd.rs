use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use chrono::{Datelike, Local, NaiveDate};

use crate::{
    atcoder::{AtCoderClient, replace_samples},
    config::Config,
    model::{AttemptMeta, ContestTask, ProblemMeta},
    paths::Repository,
};

pub fn run(
    repository: &Repository,
    config: &Config,
    contest: &str,
    requested_problems: &[String],
    requested_date: Option<&str>,
) -> Result<()> {
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

    let client = AtCoderClient::new()?;
    println!("Fetching {contest} task list...");
    let tasks = client.contest_tasks(&contest)?;
    let tasks = select_tasks(&tasks, requested_problems)?;

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

        println!(
            "  {:>3}  {} ({} samples{}{})",
            task.label,
            attempt_dir.display(),
            sample_count,
            if interactive { ", interactive" } else { "" },
            if reused { ", reused problem data" } else { "" }
        );
        if index + 1 != tasks.len() {
            thread::sleep(Duration::from_millis(200));
        }
    }

    println!("Created {}", destination.display());
    Ok(())
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
) -> Result<Vec<&'a ContestTask>> {
    if requested_problems.is_empty() {
        return Ok(tasks.iter().collect());
    }

    let selectors = requested_problems
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    if selectors.iter().any(String::is_empty) {
        bail!("問題の指定を空にはできません");
    }

    let unknown = selectors
        .iter()
        .filter(|selector| {
            !tasks.iter().any(|task| {
                task.label.eq_ignore_ascii_case(selector)
                    || task.task_id.eq_ignore_ascii_case(selector)
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        let available = tasks
            .iter()
            .map(|task| task.label.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "指定された問題が見つかりません: {}（選択可能: {available}）",
            unknown.join(", ")
        );
    }

    Ok(tasks
        .iter()
        .filter(|task| {
            selectors.iter().any(|selector| {
                task.label.eq_ignore_ascii_case(selector)
                    || task.task_id.eq_ignore_ascii_case(selector)
            })
        })
        .collect())
}

fn normalize_contest_id(value: &str) -> Result<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || !normalized
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("不正なコンテスト ID です: {value}");
    }
    Ok(normalized)
}

fn parse_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("日付は YYYY-MM-DD 形式で指定してください: {value}"))
}

fn task_directory_name(label: &str, task_id: &str) -> Result<String> {
    let label = label.trim().to_ascii_lowercase();
    if !label.is_empty()
        && label
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Ok(label);
    }

    let fallback = task_id
        .rsplit('_')
        .next()
        .unwrap_or(task_id)
        .to_ascii_lowercase();
    if fallback.is_empty()
        || !fallback
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("問題ラベルから安全なディレクトリ名を作れません: {label}");
    }
    Ok(fallback)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::model::{AttemptMeta, ContestTask};

    use super::{
        normalize_contest_id, parse_date, select_tasks, task_directory_name, write_attempt_meta,
    };

    fn task(label: &str, task_id: &str) -> ContestTask {
        ContestTask {
            label: label.to_owned(),
            task_id: task_id.to_owned(),
            title: String::new(),
            url: String::new(),
        }
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
        let selected = select_tasks(&tasks, &[]).unwrap();

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
        let selected = select_tasks(&tasks, &requested).unwrap();

        assert_eq!(
            selected
                .iter()
                .map(|task| task.label.as_str())
                .collect::<Vec<_>>(),
            ["A", "Ex"]
        );
    }

    #[test]
    fn rejects_unknown_requested_tasks() {
        let tasks = [task("A", "abc300_a"), task("B", "abc300_b")];
        let error = select_tasks(&tasks, &["c".to_owned()]).unwrap_err();

        assert_eq!(
            error.to_string(),
            "指定された問題が見つかりません: c（選択可能: A, B）"
        );
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
