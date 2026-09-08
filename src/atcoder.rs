use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use scraper::{ElementRef, Html, Selector};

use crate::model::{ContestTask, Sample, TaskPage};

const BASE_URL: &str = "https://atcoder.jp";

pub struct AtCoderClient {
    client: Client,
}

impl AtCoderClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("atcli/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .context("HTTP クライアントを初期化できません")?;
        Ok(Self { client })
    }

    pub fn contest_tasks(&self, contest: &str) -> Result<Vec<ContestTask>> {
        let url = format!("{BASE_URL}/contests/{contest}/tasks?lang=en");
        let html = self.get(&url)?;
        parse_contest_tasks(&html, contest, &url)
    }

    pub fn task_page(&self, url: &str) -> Result<TaskPage> {
        let separator = if url.contains('?') { '&' } else { '?' };
        let html = self.get(&format!("{url}{separator}lang=en"))?;
        Ok(parse_task_page(&html))
    }

    fn get(&self, url: &str) -> Result<String> {
        let response = self
            .client
            .get(url)
            .send()
            .with_context(|| format!("AtCoder への接続に失敗しました: {url}"))?
            .error_for_status()
            .with_context(|| format!("AtCoder がエラーを返しました: {url}"))?;
        response
            .text()
            .with_context(|| format!("AtCoder のレスポンスを読めません: {url}"))
    }
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static CSS selector must be valid")
}

fn element_text(element: ElementRef<'_>) -> String {
    element.text().collect::<String>().trim().to_owned()
}

pub fn parse_contest_tasks(html: &str, contest: &str, page_url: &str) -> Result<Vec<ContestTask>> {
    let document = Html::parse_document(html);
    let row_selector = selector("table tbody tr");
    let anchor_selector = selector("a");
    let expected_path = format!("/contests/{contest}/tasks/");
    let mut seen = HashSet::new();
    let mut tasks = Vec::new();

    for row in document.select(&row_selector) {
        let matching_links = row
            .select(&anchor_selector)
            .filter_map(|anchor| {
                let href = anchor.value().attr("href")?;
                href.starts_with(&expected_path)
                    .then(|| (href.to_owned(), element_text(anchor)))
            })
            .collect::<Vec<_>>();
        let Some((href, label)) = matching_links.first() else {
            continue;
        };
        let Some(task_id) = href.rsplit('/').next() else {
            continue;
        };
        if task_id.is_empty() || !seen.insert(task_id.to_owned()) {
            continue;
        }
        let title = matching_links
            .last()
            .map_or_else(String::new, |(_, text)| text.clone());
        tasks.push(ContestTask {
            label: label.clone(),
            task_id: task_id.to_owned(),
            title,
            url: format!("{BASE_URL}{href}"),
        });
    }

    if tasks.is_empty() {
        bail!("問題一覧が見つかりません: {page_url}");
    }
    Ok(tasks)
}

pub fn parse_task_page(html: &str) -> TaskPage {
    let document = Html::parse_document(html);
    let samples = [
        "#task-statement .lang-en section",
        "#task-statement .lang-ja section",
        "#task-statement section",
    ]
    .into_iter()
    .find_map(|query| {
        let parsed = extract_samples(&document, query);
        (!parsed.is_empty()).then_some(parsed)
    })
    .unwrap_or_default();

    let all_text = document.root_element().text().collect::<String>();
    let lower_text = all_text.to_ascii_lowercase();
    let interactive =
        lower_text.contains("interactive task") || all_text.contains("インタラクティブ");
    let time_limit_ms = parse_time_limit_ms(&all_text).unwrap_or(2_000);

    TaskPage {
        samples,
        time_limit_ms,
        interactive,
    }
}

fn extract_samples(document: &Html, section_query: &str) -> Vec<Sample> {
    let section_selector = selector(section_query);
    let heading_selector = selector("h3");
    let pre_selector = selector("pre");
    let mut samples: BTreeMap<u32, (Option<String>, Option<String>)> = BTreeMap::new();

    for section in document.select(&section_selector) {
        let Some(heading) = section.select(&heading_selector).next() else {
            continue;
        };
        let Some(pre) = section.select(&pre_selector).next() else {
            continue;
        };
        let Some((kind, number)) = classify_sample_heading(&element_text(heading)) else {
            continue;
        };
        let value = normalize_sample(&pre.text().collect::<String>());
        let pair = samples.entry(number).or_default();
        match kind {
            SampleKind::Input => pair.0 = Some(value),
            SampleKind::Output => pair.1 = Some(value),
        }
    }

    samples
        .into_iter()
        .filter_map(|(number, (input, output))| {
            Some(Sample {
                number,
                input: input?,
                output: output?,
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
enum SampleKind {
    Input,
    Output,
}

fn classify_sample_heading(heading: &str) -> Option<(SampleKind, u32)> {
    let lower = heading.to_ascii_lowercase();
    let kind = if lower.contains("sample input")
        || lower.contains("input example")
        || heading.contains("入力例")
    {
        SampleKind::Input
    } else if lower.contains("sample output")
        || lower.contains("output example")
        || heading.contains("出力例")
    {
        SampleKind::Output
    } else {
        return None;
    };
    let digits = heading
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    let number = digits.parse().unwrap_or(1);
    Some((kind, number))
}

fn normalize_sample(value: &str) -> String {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    format!("{}\n", normalized.trim_end_matches('\n'))
}

fn parse_time_limit_ms(text: &str) -> Option<u64> {
    ["Time Limit:", "実行時間制限:", "実行時間制限："]
        .into_iter()
        .find_map(|marker| {
            let rest = text.split_once(marker)?.1.trim_start();
            let number = rest
                .chars()
                .take_while(|character| character.is_ascii_digit() || *character == '.')
                .collect::<String>();
            decimal_seconds_to_milliseconds(&number)
        })
}

fn decimal_seconds_to_milliseconds(value: &str) -> Option<u64> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let seconds = whole.parse::<u64>().ok()?;
    if !fraction.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let mut milliseconds = fraction.chars().take(3).collect::<String>();
    while milliseconds.len() < 3 {
        milliseconds.push('0');
    }
    seconds
        .checked_mul(1_000)?
        .checked_add(milliseconds.parse::<u64>().unwrap_or(0))
}

pub fn replace_samples(problem_dir: &Path, samples: &[Sample]) -> Result<()> {
    let tests_dir = problem_dir.join("tests");
    fs::create_dir_all(&tests_dir).with_context(|| {
        format!(
            "テストディレクトリを作成できません: {}",
            tests_dir.display()
        )
    })?;

    for entry in fs::read_dir(&tests_dir)
        .with_context(|| format!("テストディレクトリを読めません: {}", tests_dir.display()))?
    {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let is_sample_file = name.starts_with("sample-")
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("in") || extension.eq_ignore_ascii_case("out")
                });
        if is_sample_file {
            fs::remove_file(&path)
                .with_context(|| format!("古いサンプルを削除できません: {}", path.display()))?;
        }
    }

    for sample in samples {
        let input = tests_dir.join(format!("sample-{}.in", sample.number));
        let output = tests_dir.join(format!("sample-{}.out", sample.number));
        fs::write(&input, &sample.input)
            .with_context(|| format!("サンプル入力を書き込めません: {}", input.display()))?;
        fs::write(&output, &sample.output)
            .with_context(|| format!("サンプル出力を書き込めません: {}", output.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{parse_contest_tasks, parse_task_page, replace_samples};

    #[test]
    fn parses_contest_task_rows_once() {
        let html = r#"
            <table><tbody><tr>
              <td><a href="/contests/abc300/tasks/abc300_a">A</a></td>
              <td><a href="/contests/abc300/tasks/abc300_a">N-choice question</a></td>
            </tr></tbody></table>
        "#;
        let tasks = parse_contest_tasks(html, "abc300", "test").unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].label, "A");
        assert_eq!(tasks[0].task_id, "abc300_a");
        assert_eq!(tasks[0].title, "N-choice question");
    }

    #[test]
    fn prefers_english_samples_and_parses_metadata() {
        let html = r#"
          <div>Time Limit: 1.5 sec / Memory Limit: 1024 MiB</div>
          <div id="task-statement">
            <span class="lang-ja">
              <section><h3>入力例 1</h3><pre>ja input</pre></section>
              <section><h3>出力例 1</h3><pre>ja output</pre></section>
            </span>
            <span class="lang-en">
              <section><h3>Sample Input 1</h3><pre>1 2</pre></section>
              <section><h3>Sample Output 1</h3><pre>3</pre></section>
            </span>
          </div>
        "#;
        let page = parse_task_page(html);
        assert_eq!(page.time_limit_ms, 1_500);
        assert!(!page.interactive);
        assert_eq!(page.samples.len(), 1);
        assert_eq!(page.samples[0].input, "1 2\n");
        assert_eq!(page.samples[0].output, "3\n");
    }

    #[test]
    fn sample_replacement_preserves_custom_cases() {
        let temp = tempdir().unwrap();
        let tests = temp.path().join("tests");
        fs::create_dir(&tests).unwrap();
        fs::write(tests.join("sample-9.in"), "old").unwrap();
        fs::write(tests.join("sample-9.out"), "old").unwrap();
        fs::write(tests.join("my-1.in"), "mine").unwrap();
        let page = parse_task_page(
            r#"<div id="task-statement"><span class="lang-en">
              <section><h3>Sample Input 1</h3><pre>new</pre></section>
              <section><h3>Sample Output 1</h3><pre>ok</pre></section>
            </span></div>"#,
        );

        replace_samples(temp.path(), &page.samples).unwrap();

        assert!(!tests.join("sample-9.in").exists());
        assert_eq!(
            fs::read_to_string(tests.join("sample-1.in")).unwrap(),
            "new\n"
        );
        assert_eq!(fs::read_to_string(tests.join("my-1.in")).unwrap(), "mine");
    }
}
