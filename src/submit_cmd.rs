use std::{
    fs,
    io::{self, Write},
    path::Path,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use owo_colors::OwoColorize;

use crate::{
    atcoder::{AtCoderClient, Language},
    config::Config,
    model::ProblemMeta,
    paths::Repository,
    session::SessionStore,
    test_cmd,
};

#[derive(Clone, Copy)]
pub struct Options<'a> {
    pub language: Option<&'a str>,
    pub list_languages: bool,
    pub no_test: bool,
    pub yes: bool,
    pub watch: Option<bool>,
}

pub fn run(
    repository: &Repository,
    config: &Config,
    problem_dir: &Path,
    options: Options<'_>,
) -> Result<()> {
    validate_config(config)?;
    let meta = ProblemMeta::read(problem_dir)?;
    let store = SessionStore::discover()?;
    let session = store
        .load()?
        .context("ログインセッションがありません。`atcli login` を実行してください")?;
    let client = AtCoderClient::with_session(&session)?;
    if !client.session_is_valid()? {
        bail!("AtCoder のセッションが無効です。`atcli login` を実行してください");
    }
    let page = client.submit_page(&meta.contest, &meta.task_id)?;

    if options.list_languages {
        print_languages(&page.languages);
        return Ok(());
    }
    let language_query = options.language.unwrap_or(config.submit.language.as_str());
    let language = resolve_language(&page.languages, language_query)?;

    if !options.no_test {
        if meta.interactive {
            bail!(
                "interactive task はローカル判定できません。確認済みなら `atcli submit --no-test` を指定してください"
            );
        }
        let summary =
            test_cmd::run(repository, config, problem_dir, true, None)?.require_success()?;
        if summary.passed == 0 {
            bail!(
                "期待出力付きのテストに合格していません。確認済みなら `atcli submit --no-test` を指定してください"
            );
        }
    }

    let source_path = problem_dir.join("main.cpp");
    let source = fs::read_to_string(&source_path)
        .with_context(|| format!("解答ファイルを読めません: {}", source_path.display()))?;
    if source.trim().is_empty() {
        bail!("解答ファイルが空です: {}", source_path.display());
    }
    let line_count = source.lines().count();
    println!("Contest:  {}", meta.contest);
    println!("Task:     {} ({})", meta.task_id, meta.label);
    println!("Language: {} ({})", language.name, language.id);
    println!("Source:   {} ({line_count} lines)", source_path.display());

    if !options.yes && !confirm("Submit to AtCoder? [y/N] ")? {
        bail!("提出をキャンセルしました");
    }

    let previous_id = if options.watch.unwrap_or(config.submit.watch) {
        client
            .latest_submission(&meta.contest, &meta.task_id)?
            .map(|submission| submission.id)
    } else {
        None
    };
    client.submit(
        &meta.contest,
        &meta.task_id,
        &language.id,
        &source,
        &page.csrf_token,
    )?;
    println!("{} submission accepted", "OK".green().bold());

    if options.watch.unwrap_or(config.submit.watch) {
        watch_submission(&client, config, &meta, previous_id)?;
    }
    Ok(())
}

fn validate_config(config: &Config) -> Result<()> {
    if config.submit.poll_interval_ms < 1_000 {
        bail!("submit.poll_interval_ms は AtCoder への負荷を避けるため 1000 以上にしてください");
    }
    Ok(())
}

fn print_languages(languages: &[Language]) {
    for language in languages {
        println!("{:>5}  {}", language.id, language.name);
    }
}

fn resolve_language<'a>(languages: &'a [Language], query: &str) -> Result<&'a Language> {
    let query = query.trim();
    if query.is_empty() {
        bail!("提出言語を空にはできません");
    }
    if let Some(language) = languages
        .iter()
        .find(|language| language.id == query || language.name.eq_ignore_ascii_case(query))
    {
        return Ok(language);
    }

    let query_lower = query.to_ascii_lowercase();
    let matches = languages
        .iter()
        .filter(|language| language.name.to_ascii_lowercase().contains(&query_lower))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [language] => Ok(*language),
        [] => bail!(
            "提出言語が見つかりません: {query}（`atcli submit --list-languages` で確認できます）"
        ),
        _ => bail!(
            "提出言語が複数見つかりました: {}。言語 ID または完全な名前を指定してください",
            matches
                .iter()
                .map(|language| format!("{} ({})", language.name, language.id))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn confirm(message: &str) -> Result<bool> {
    print!("{message}");
    io::stdout()
        .flush()
        .context("確認プロンプトを表示できません")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("標準入力を読めません")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn watch_submission(
    client: &AtCoderClient,
    config: &Config,
    meta: &ProblemMeta,
    previous_id: Option<u64>,
) -> Result<()> {
    let interval = Duration::from_millis(config.submit.poll_interval_ms);
    let mut last_status = None;
    loop {
        thread::sleep(interval);
        let Some(submission) = client.latest_submission(&meta.contest, &meta.task_id)? else {
            continue;
        };
        if previous_id.is_some_and(|id| submission.id == id) {
            continue;
        }
        if last_status.as_deref() != Some(submission.result.as_str()) {
            println!(
                "{}  {}  {}",
                submission.result, submission.language, submission.url
            );
            last_status = Some(submission.result.clone());
        }
        if submission.is_finished() {
            if submission.result == "AC" {
                println!("{} {}", "AC".green().bold(), submission.url);
                return Ok(());
            }
            bail!("判定結果: {} ({})", submission.result, submission.url);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::atcoder::Language;

    use super::resolve_language;

    fn language(id: &str, name: &str) -> Language {
        Language {
            id: id.to_owned(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn resolves_language_by_id_exact_name_or_unique_fragment() {
        let languages = [
            language("5001", "C++ 20 (gcc 12.2)"),
            language("5002", "C++ 23 (gcc 12.2)"),
            language("5078", "Python (CPython 3.11.4)"),
        ];

        assert_eq!(resolve_language(&languages, "5001").unwrap().id, "5001");
        assert_eq!(
            resolve_language(&languages, "c++ 23 (GCC 12.2)")
                .unwrap()
                .id,
            "5002"
        );
        assert_eq!(resolve_language(&languages, "Python").unwrap().id, "5078");
        assert!(resolve_language(&languages, "C++").is_err());
        assert!(resolve_language(&languages, "Rust").is_err());
    }
}
