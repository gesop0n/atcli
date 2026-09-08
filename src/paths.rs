use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Debug)]
pub struct Repository {
    pub root: PathBuf,
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

    pub fn find_problem_dir(&self, start: &Path) -> Result<PathBuf> {
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
            if current == self.root || !current.pop() {
                break;
            }
        }

        bail!(
            "meta.toml が見つかりません。問題ディレクトリ内で実行するか、問題ディレクトリを指定してください"
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::Repository;

    #[test]
    fn discovers_repository_and_problem_from_descendant() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("atcli.toml"), "").unwrap();
        let problem = temp.path().join("2026/09/09/abc300/a");
        let nested = problem.join("tests");
        fs::create_dir_all(&nested).unwrap();
        fs::write(problem.join("meta.toml"), "").unwrap();

        let repository = Repository::discover(&nested).unwrap();
        assert_eq!(repository.root, temp.path().canonicalize().unwrap());
        assert_eq!(
            repository.find_problem_dir(&nested).unwrap(),
            problem.canonicalize().unwrap()
        );
    }
}
