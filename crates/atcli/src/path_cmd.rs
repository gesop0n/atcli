use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::{Datelike, Local, NaiveDate};

use atcli_core::{
    config::Config,
    paths::{Repository, normalize_directory_component},
};

/// エラー文に並べる候補の上限。
const MAX_LISTED_CANDIDATES: usize = 20;

pub fn root(repository: &Repository) -> PathBuf {
    repository.root.clone()
}

pub fn today(
    repository: &Repository,
    config: &Config,
    requested_date: Option<&str>,
) -> Result<PathBuf> {
    let date = requested_date
        .map(parse_date)
        .transpose()?
        .unwrap_or_else(|| Local::now().date_naive());
    let path = date_dir(&attempts_root(repository, config), date);
    if !path.is_dir() {
        anyhow::bail!("取り組みディレクトリがありません: {}", path.display());
    }
    Ok(path.canonicalize()?)
}

/// コンテスト（問題を指定すればその問題）の取り組みディレクトリを解決する。
///
/// 日付を省略した場合はまず今日を見て、無ければそのコンテストを含む最新の日へ遡る。
/// 数日前に解いた問題へ戻るときに日付を思い出さなくて済む。
pub fn attempt(
    repository: &Repository,
    config: &Config,
    contest: &str,
    problem: Option<&str>,
    requested_date: Option<&str>,
) -> Result<PathBuf> {
    let contest = normalize_directory_component(contest)
        .with_context(|| format!("不正なコンテスト ID です: {contest}"))?;
    let problem = problem
        .map(|value| {
            normalize_directory_component(value)
                .with_context(|| format!("不正な問題の指定です: {value}"))
        })
        .transpose()?;

    let attempts_root = attempts_root(repository, config);
    let dates = candidate_dates(&attempts_root, requested_date)?;

    for date in &dates {
        let contest_dir = date_dir(&attempts_root, *date).join(&contest);
        let target = match &problem {
            Some(problem) => contest_dir.join(problem),
            None => contest_dir,
        };
        if target.is_dir() {
            return Ok(target.canonicalize()?);
        }
    }

    // コンテストは見つかるのに問題が無いときは、その場所の候補を並べたほうが早く直せる。
    if problem.is_some() {
        for date in &dates {
            let contest_dir = date_dir(&attempts_root, *date).join(&contest);
            if contest_dir.is_dir() {
                bail!(
                    "取り組みディレクトリがありません: {}（選択可能: {}）",
                    contest_dir.display(),
                    join_candidates(&directory_names(&contest_dir)?)
                );
            }
        }
    }
    bail!(
        "{contest} の取り組みディレクトリが見つかりません（最近のコンテスト: {}）",
        join_candidates(&recent_contests(&attempts_root)?)
    )
}

fn attempts_root(repository: &Repository, config: &Config) -> PathBuf {
    repository.root.join(&config.repository.attempts_dir)
}

fn date_dir(attempts_root: &Path, date: NaiveDate) -> PathBuf {
    attempts_root
        .join(format!("{:04}", date.year()))
        .join(format!("{:02}", date.month()))
        .join(format!("{:02}", date.day()))
}

/// 探索する日付を、優先度の高い順に並べて返す。
///
/// 日付が明示された場合はその日だけを見る（意図しない日へ黙って移動しないため）。
fn candidate_dates(attempts_root: &Path, requested_date: Option<&str>) -> Result<Vec<NaiveDate>> {
    if let Some(value) = requested_date {
        return Ok(vec![parse_date(value)?]);
    }

    let today = Local::now().date_naive();
    let mut dates = vec![today];
    dates.extend(
        existing_dates(attempts_root)
            .into_iter()
            .rev()
            .filter(|date| *date != today),
    );
    Ok(dates)
}

/// `attempts/YYYY/MM/DD` として存在する日付を昇順で集める。
fn existing_dates(attempts_root: &Path) -> Vec<NaiveDate> {
    let mut dates = BTreeSet::new();
    for year in numeric_entries(attempts_root) {
        for month in numeric_entries(&attempts_root.join(&year)) {
            for day in numeric_entries(&attempts_root.join(&year).join(&month)) {
                if let Ok(date) =
                    NaiveDate::parse_from_str(&format!("{year}-{month}-{day}"), "%Y-%m-%d")
                {
                    dates.insert(date);
                }
            }
        }
    }
    dates.into_iter().collect()
}

fn numeric_entries(dir: &Path) -> Vec<String> {
    directory_names(dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| name.chars().all(|character| character.is_ascii_digit()))
        .collect()
}

fn directory_names(dir: &Path) -> Result<Vec<String>> {
    let entries = fs::read_dir(dir)
        .with_context(|| format!("ディレクトリを読めません: {}", dir.display()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<BTreeSet<_>>();
    Ok(entries.into_iter().collect())
}

/// 新しい日から順に、取り組んだことのあるコンテスト ID を重複なく集める。
fn recent_contests(attempts_root: &Path) -> Result<Vec<String>> {
    let mut contests = Vec::new();
    for date in existing_dates(attempts_root).into_iter().rev() {
        for name in directory_names(&date_dir(attempts_root, date))? {
            if !contests.contains(&name) {
                contests.push(name);
            }
            if contests.len() >= MAX_LISTED_CANDIDATES {
                return Ok(contests);
            }
        }
    }
    Ok(contests)
}

fn join_candidates(candidates: &[String]) -> String {
    if candidates.is_empty() {
        return "なし".to_string();
    }
    let listed = candidates
        .iter()
        .take(MAX_LISTED_CANDIDATES)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if candidates.len() > MAX_LISTED_CANDIDATES {
        format!(
            "{listed}, ...他 {} 件",
            candidates.len() - MAX_LISTED_CANDIDATES
        )
    } else {
        listed
    }
}

fn parse_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("日付は YYYY-MM-DD 形式で指定してください: {value}"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn repository_with(dirs: &[&str]) -> (tempfile::TempDir, Repository, Config) {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("atcli.toml"), "").unwrap();
        for dir in dirs {
            fs::create_dir_all(temp.path().join(dir)).unwrap();
        }
        let repository = Repository::discover(temp.path()).unwrap();
        (temp, repository, Config::default())
    }

    #[test]
    fn accepts_explicit_existing_date() {
        let (_temp, repository, config) = repository_with(&["attempts/2026/09/08"]);

        today(&repository, &config, Some("2026-09-08")).unwrap();
        assert!(today(&repository, &config, Some("2026-09-09")).is_err());
    }

    #[test]
    fn accepts_repository_root() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("atcli.toml"), "").unwrap();
        let nested = temp.path().join("attempts/2026/09/08");
        fs::create_dir_all(&nested).unwrap();
        let repository = Repository::discover(&nested).unwrap();

        assert_eq!(root(&repository), temp.path().canonicalize().unwrap());
    }

    #[test]
    fn rejects_invalid_date() {
        assert!(parse_date("2026/09/08").is_err());
    }

    #[test]
    fn resolves_contest_and_problem_on_explicit_date() {
        let (temp, repository, config) = repository_with(&["attempts/2026/09/08/abc300/b"]);

        assert_eq!(
            attempt(&repository, &config, "ABC300", None, Some("2026-09-08")).unwrap(),
            temp.path()
                .join("attempts/2026/09/08/abc300")
                .canonicalize()
                .unwrap()
        );
        assert_eq!(
            attempt(
                &repository,
                &config,
                "abc300",
                Some("B"),
                Some("2026-09-08")
            )
            .unwrap(),
            temp.path()
                .join("attempts/2026/09/08/abc300/b")
                .canonicalize()
                .unwrap()
        );
    }

    #[test]
    fn falls_back_to_most_recent_date_when_unspecified() {
        let (temp, repository, config) = repository_with(&[
            "attempts/2026/09/08/abc300/a",
            "attempts/2026/09/11/abc300/a",
        ]);

        // 今日のディレクトリは無いので、そのコンテストを含む最新の日へ遡る。
        assert_eq!(
            attempt(&repository, &config, "abc300", Some("a"), None).unwrap(),
            temp.path()
                .join("attempts/2026/09/11/abc300/a")
                .canonicalize()
                .unwrap()
        );
    }

    #[test]
    fn does_not_fall_back_when_date_is_explicit() {
        let (_temp, repository, config) = repository_with(&["attempts/2026/09/11/abc300/a"]);

        assert!(
            attempt(
                &repository,
                &config,
                "abc300",
                Some("a"),
                Some("2026-09-08")
            )
            .is_err()
        );
    }

    #[test]
    fn reports_available_problems_when_problem_is_missing() {
        let (_temp, repository, config) = repository_with(&[
            "attempts/2026/09/08/abc300/a",
            "attempts/2026/09/08/abc300/b",
        ]);

        let error = attempt(
            &repository,
            &config,
            "abc300",
            Some("c"),
            Some("2026-09-08"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("a, b"), "{error}");
    }

    #[test]
    fn reports_recent_contests_when_contest_is_missing() {
        let (_temp, repository, config) = repository_with(&["attempts/2026/09/08/abc300/a"]);

        let error = attempt(&repository, &config, "abc999", None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("abc300"), "{error}");
    }

    #[test]
    fn rejects_contest_id_that_escapes_the_attempts_directory() {
        let (_temp, repository, config) = repository_with(&["attempts/2026/09/08/abc300/a"]);

        assert!(attempt(&repository, &config, "../../tmp", None, None).is_err());
        assert!(attempt(&repository, &config, "abc300", Some("../.."), None).is_err());
    }
}
