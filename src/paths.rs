use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::model::AttemptMeta;

#[derive(Debug)]
pub struct Repository {
    pub root: PathBuf,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Attempt {
    pub dir: PathBuf,
    pub problem_dir: PathBuf,
}

impl Repository {
    pub fn discover(start: &Path) -> Result<Self> {
        let mut current = start
            .canonicalize()
            .with_context(|| format!("パスを解決できません: {}", start.display()))?;
        if current.is_file() {
            current.pop();
        }

        loop {
            if current.join("atcli.toml").is_file() {
                return Ok(Self { root: current });
            }
            if !current.pop() {
                break;
            }
        }

        bail!("atcli.toml が見つかりません。AtCoder リポジトリ内で実行してください")
    }

    pub fn find_attempt(&self, start: &Path, problems_location: &Path) -> Result<Attempt> {
        let attempt_dir = self.find_ancestor_with(start, "attempt.toml").with_context(|| {
            "attempt.toml が見つかりません。取り組みディレクトリ内で実行するか、取り組みディレクトリを指定してください"
        })?;
        let attempt = AttemptMeta::read(&attempt_dir)?;
        let problem_dir = self.resolve_problem(&attempt.problem, problems_location)?;
        Ok(Attempt {
            dir: attempt_dir,
            problem_dir,
        })
    }

    pub fn find_problem_for(&self, start: &Path, problems_location: &Path) -> Result<PathBuf> {
        let mut current = start
            .canonicalize()
            .with_context(|| format!("パスを解決できません: {}", start.display()))?;
        if current.is_file() {
            current.pop();
        }

        loop {
            if current.join("meta.toml").is_file() {
                return Ok(current);
            }
            if current.join("attempt.toml").is_file() {
                let attempt = AttemptMeta::read(&current)?;
                return self.resolve_problem(&attempt.problem, problems_location);
            }
            if current == self.root || !current.pop() {
                break;
            }
        }

        bail!(
            "meta.toml または attempt.toml が見つかりません。問題か取り組みのディレクトリ内で実行してください"
        )
    }

    fn find_ancestor_with(&self, start: &Path, marker: &str) -> Result<PathBuf> {
        let mut current = start
            .canonicalize()
            .with_context(|| format!("パスを解決できません: {}", start.display()))?;
        if current.is_file() {
            current.pop();
        }

        loop {
            if current.join(marker).is_file() {
                return Ok(current);
            }
            if current == self.root || !current.pop() {
                break;
            }
        }
        bail!("{marker} が見つかりません")
    }

    fn resolve_problem(&self, relative: &Path, problems_location: &Path) -> Result<PathBuf> {
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!(
                "attempt.toml の problem は problems_dir からの相対パスにしてください: {}",
                relative.display()
            );
        }

        let problems_root = self
            .root
            .join(problems_location)
            .canonicalize()
            .with_context(|| {
                format!(
                    "問題データディレクトリを解決できません: {}",
                    self.root.join(problems_location).display()
                )
            })?;
        let problem_dir = problems_root
            .join(relative)
            .canonicalize()
            .with_context(|| {
                format!(
                    "attempt.toml が参照する問題データを解決できません: {}",
                    problems_root.join(relative).display()
                )
            })?;
        if !problem_dir.starts_with(&problems_root) || !problem_dir.join("meta.toml").is_file() {
            bail!(
                "attempt.toml が参照する問題データが不正です: {}",
                problem_dir.display()
            );
        }
        Ok(problem_dir)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tempfile::tempdir;

    use super::Repository;

    #[test]
    fn discovers_repository_and_attempt_from_descendant() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("atcli.toml"), "").unwrap();
        let problem = temp.path().join("problems/abc300/a");
        fs::create_dir_all(&problem).unwrap();
        fs::write(problem.join("meta.toml"), "").unwrap();
        let attempt = temp.path().join("attempts/2026/09/09/abc300/a");
        let nested = attempt.join("notes");
        fs::create_dir_all(&nested).unwrap();
        fs::write(attempt.join("attempt.toml"), "problem = \"abc300/a\"\n").unwrap();

        let repository = Repository::discover(&nested).unwrap();
        assert_eq!(repository.root, temp.path().canonicalize().unwrap());
        let found = repository
            .find_attempt(&nested, Path::new("problems"))
            .unwrap();
        assert_eq!(found.dir, attempt.canonicalize().unwrap());
        assert_eq!(found.problem_dir, problem.canonicalize().unwrap());
        assert_eq!(
            repository
                .find_problem_for(&nested, Path::new("problems"))
                .unwrap(),
            problem.canonicalize().unwrap()
        );
    }

    #[test]
    fn rejects_attempt_reference_outside_problems_directory() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("atcli.toml"), "").unwrap();
        fs::create_dir_all(temp.path().join("problems")).unwrap();
        let attempt = temp.path().join("attempts/2026/09/09/abc300/a");
        fs::create_dir_all(&attempt).unwrap();
        fs::write(
            attempt.join("attempt.toml"),
            "problem = \"../../outside\"\n",
        )
        .unwrap();

        let repository = Repository::discover(&attempt).unwrap();
        assert!(
            repository
                .find_attempt(&attempt, Path::new("problems"))
                .is_err()
        );
    }
}
