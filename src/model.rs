use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContestTask {
    pub label: String,
    pub task_id: String,
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sample {
    pub number: u32,
    pub input: String,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskPage {
    pub samples: Vec<Sample>,
    pub time_limit_ms: u64,
    pub interactive: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProblemMeta {
    pub url: String,
    pub contest: String,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub time_limit_ms: u64,
    pub interactive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
}

impl ProblemMeta {
    pub fn read(problem_dir: &Path) -> Result<Self> {
        let path = problem_dir.join("meta.toml");
        let source = fs::read_to_string(&path)
            .with_context(|| format!("問題メタデータを読めません: {}", path.display()))?;
        toml::from_str(&source)
            .with_context(|| format!("問題メタデータの形式が不正です: {}", path.display()))
    }

    pub fn write(&self, problem_dir: &Path) -> Result<()> {
        let path = problem_dir.join("meta.toml");
        let source = toml::to_string_pretty(self).context("問題メタデータを変換できません")?;
        fs::write(&path, source)
            .with_context(|| format!("問題メタデータを書き込めません: {}", path.display()))
    }
}
