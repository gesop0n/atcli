use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::paths::Repository;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub repository: RepositoryConfig,
    pub cpp: CppConfig,
    pub test: TestConfig,
    pub submit: SubmitConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct RepositoryConfig {
    pub solutions_dir: PathBuf,
    pub template: PathBuf,
}

impl Default for RepositoryConfig {
    fn default() -> Self {
        Self {
            solutions_dir: PathBuf::from("."),
            template: PathBuf::from("template/main.cpp"),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct CppConfig {
    pub compiler: String,
    pub standard: String,
    pub include_dirs: Vec<PathBuf>,
    pub debug_flags: Vec<String>,
    pub release_flags: Vec<String>,
}

impl Default for CppConfig {
    fn default() -> Self {
        Self {
            compiler: "g++".to_owned(),
            standard: "gnu++23".to_owned(),
            include_dirs: vec![PathBuf::from("lib"), PathBuf::from("ac-library")],
            debug_flags: vec![
                "-O0".to_owned(),
                "-g3".to_owned(),
                "-Wall".to_owned(),
                "-Wextra".to_owned(),
            ],
            release_flags: vec!["-O2".to_owned(), "-Wall".to_owned(), "-Wextra".to_owned()],
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct TestConfig {
    pub timeout_multiplier: f64,
    pub minimum_timeout_ms: u64,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            timeout_multiplier: 2.0,
            minimum_timeout_ms: 1_000,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct SubmitConfig {
    /// Language ID or an unambiguous part of the language name on `AtCoder`.
    pub language: String,
    pub watch: bool,
    pub poll_interval_ms: u64,
}

impl Default for SubmitConfig {
    fn default() -> Self {
        Self {
            language: "C++23 (GCC".to_owned(),
            watch: true,
            poll_interval_ms: 2_000,
        }
    }
}

impl Config {
    pub fn load(repository: &Repository) -> Result<Self> {
        let path = repository.root.join("atcli.toml");
        let source = fs::read_to_string(&path)
            .with_context(|| format!("設定ファイルを読めません: {}", path.display()))?;
        toml::from_str(&source)
            .with_context(|| format!("設定ファイルの形式が不正です: {}", path.display()))
    }
}
