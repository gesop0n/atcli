use anyhow::{Context, Result};
use chrono::{Datelike, Local, NaiveDate};

use crate::{config::Config, paths::Repository};

pub fn today(repository: &Repository, config: &Config, requested_date: Option<&str>) -> Result<()> {
    let date = requested_date
        .map(parse_date)
        .transpose()?
        .unwrap_or_else(|| Local::now().date_naive());
    let path = repository
        .root
        .join(&config.repository.attempts_dir)
        .join(format!("{:04}", date.year()))
        .join(format!("{:02}", date.month()))
        .join(format!("{:02}", date.day()));
    if !path.is_dir() {
        anyhow::bail!("取り組みディレクトリがありません: {}", path.display());
    }
    println!("{}", path.canonicalize()?.display());
    Ok(())
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

    #[test]
    fn accepts_explicit_existing_date() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("atcli.toml"), "").unwrap();
        fs::create_dir_all(temp.path().join("attempts/2026/09/08")).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        let config = Config::default();

        today(&repository, &config, Some("2026-09-08")).unwrap();
        assert!(today(&repository, &config, Some("2026-09-09")).is_err());
    }

    #[test]
    fn rejects_invalid_date() {
        assert!(parse_date("2026/09/08").is_err());
    }
}
