use std::{fs, path::Path, thread, time::Duration};

use anyhow::{Context, Result, bail};
use chrono::{Datelike, Local, NaiveDate};

use crate::{
    atcoder::{AtCoderClient, replace_samples},
    config::Config,
    model::ProblemMeta,
    paths::Repository,
};

pub fn run(
    repository: &Repository,
    config: &Config,
    contest: &str,
    requested_date: Option<&str>,
) -> Result<()> {
    let contest = normalize_contest_id(contest)?;
    let date = requested_date
        .map(parse_date)
        .transpose()?
        .unwrap_or_else(|| Local::now().date_naive());
    let solutions_root = if config.repository.solutions_dir == Path::new(".") {
        repository.root.clone()
    } else {
        repository.root.join(&config.repository.solutions_dir)
    };
    let destination = solutions_root
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

    for (index, task) in tasks.iter().enumerate() {
        let directory_name = task_directory_name(&task.label, &task.task_id)?;
        let problem_dir = destination.join(directory_name);
        fs::create_dir_all(problem_dir.join("tests")).with_context(|| {
            format!(
                "問題ディレクトリを作成できません: {}",
                problem_dir.display()
            )
        })?;

        let main_cpp = problem_dir.join("main.cpp");
        if !main_cpp.exists() {
            fs::write(&main_cpp, &template)
                .with_context(|| format!("テンプレートを書き込めません: {}", main_cpp.display()))?;
        }

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
        meta.write(&problem_dir)?;
        replace_samples(&problem_dir, &page.samples)?;

        println!(
            "  {:>3}  {} ({} samples{})",
            task.label,
            problem_dir.display(),
            page.samples.len(),
            if page.interactive {
                ", interactive"
            } else {
                ""
            }
        );
        if index + 1 != tasks.len() {
            thread::sleep(Duration::from_millis(200));
        }
    }

    println!("Created {}", destination.display());
    Ok(())
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
    use super::{normalize_contest_id, parse_date, task_directory_name};

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
}
