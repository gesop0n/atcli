use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::{
    Url,
    blocking::{Client, Response},
    cookie::Jar,
    header::{LOCATION, SET_COOKIE},
    redirect,
};
use scraper::{ElementRef, Html, Selector};

use crate::{
    model::{ContestTask, Sample, TaskPage},
    session::Session,
};

const BASE_URL: &str = "https://atcoder.jp";

pub struct AtCoderClient {
    client: Client,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Language {
    pub id: String,
    pub name: String,
}

pub struct SubmitPage {
    pub csrf_token: String,
    pub languages: Vec<Language>,
    pub requires_captcha: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Submission {
    pub id: u64,
    pub url: String,
    pub language: String,
    pub result: String,
}

impl Submission {
    pub fn is_finished(&self) -> bool {
        let status = self.result.trim().to_ascii_uppercase();
        !status.is_empty()
            && status != "WJ"
            && status != "WR"
            && !status.contains("JUDGING")
            && !status.contains("WAITING")
    }
}

impl AtCoderClient {
    pub fn new() -> Result<Self> {
        Self::build(None)
    }

    pub fn with_session(session: &Session) -> Result<Self> {
        Self::build(Some(session))
    }

    fn build(session: Option<&Session>) -> Result<Self> {
        let jar = Arc::new(Jar::default());
        if let Some(session) = session {
            let base_url = Url::parse(BASE_URL).expect("BASE_URL must be valid");
            jar.add_cookie_str(
                &format!(
                    "REVEL_SESSION={}; Domain=atcoder.jp; Path=/; Secure; HttpOnly",
                    session.value()
                ),
                &base_url,
            );
        }
        let client = Client::builder()
            .user_agent(concat!("atcli/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .redirect(redirect::Policy::none())
            .cookie_provider(jar)
            .build()
            .context("HTTP クライアントを初期化できません")?;
        Ok(Self { client })
    }

    pub fn login(&self, username: &str, password: &str) -> Result<Session> {
        let url = format!("{BASE_URL}/login?lang=en");
        let page = self.get(&url)?;
        if page.contains("cf-challenge") || page.contains("challenges.cloudflare.com/turnstile") {
            bail!(
                "AtCoder のログインにブラウザ認証が必要です。ブラウザでログイン後、`atcli login --session` で REVEL_SESSION を取り込んでください"
            );
        }
        let csrf_token = parse_csrf_token(&page)?;
        let response = self
            .client
            .post(format!("{BASE_URL}/login"))
            .form(&[
                ("username", username),
                ("password", password),
                ("csrf_token", csrf_token.as_str()),
            ])
            .send()
            .context("AtCoder へのログイン要求に失敗しました")?;

        if !response.status().is_redirection() || is_login_redirect(&response) {
            bail!("AtCoder にログインできません。ユーザー名とパスワードを確認してください");
        }
        let cookie = extract_revel_session(&response)
            .context("ログイン応答に REVEL_SESSION がありません")?;
        let session = Session::new(&cookie)?;
        if !self.session_is_valid()? {
            bail!("AtCoder のログインセッションを確認できませんでした");
        }
        Ok(session)
    }

    pub fn session_is_valid(&self) -> Result<bool> {
        let response = self
            .client
            .get(format!("{BASE_URL}/settings"))
            .send()
            .context("AtCoder のログイン状態を確認できません")?;
        if response.status().is_redirection() {
            return Ok(false);
        }
        Ok(response.status().is_success())
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

    pub fn submit_page(&self, contest: &str, task_id: &str) -> Result<SubmitPage> {
        let url = format!("{BASE_URL}/contests/{contest}/submit?lang=en");
        let html = self.authenticated_get(&url)?;
        parse_submit_page(&html, task_id)
    }

    pub fn submit(
        &self,
        contest: &str,
        task_id: &str,
        language_id: &str,
        source: &str,
        csrf_token: &str,
    ) -> Result<()> {
        let url = format!("{BASE_URL}/contests/{contest}/submit");
        let response = self
            .client
            .post(&url)
            .form(&[
                ("data.TaskScreenName", task_id),
                ("data.LanguageId", language_id),
                ("sourceCode", source),
                ("csrf_token", csrf_token),
            ])
            .send()
            .with_context(|| format!("AtCoder への提出に失敗しました: {url}"))?;

        if response.status().is_redirection() && !is_login_redirect(&response) {
            return Ok(());
        }
        if is_login_redirect(&response) {
            bail!("AtCoder のセッションが無効です。`atcli login` を実行してください");
        }
        let status = response.status();
        let body = response.text().unwrap_or_default();
        let message = parse_alert_message(&body).map_or_else(
            || format!("HTTP {status}"),
            |message| format!("HTTP {status}: {message}"),
        );
        bail!("AtCoder が提出を受け付けませんでした（{message}）");
    }

    pub fn latest_submission(&self, contest: &str, task_id: &str) -> Result<Option<Submission>> {
        let url = format!("{BASE_URL}/contests/{contest}/submissions/me?lang=en");
        let response = self
            .client
            .get(&url)
            .query(&[("f.Task", task_id)])
            .send()
            .with_context(|| format!("AtCoder の提出一覧を取得できません: {url}"))?;
        if is_login_redirect(&response) {
            bail!("AtCoder のセッションが無効です。`atcli login` を実行してください");
        }
        let response = response
            .error_for_status()
            .with_context(|| format!("AtCoder がエラーを返しました: {url}"))?;
        let html = response
            .text()
            .with_context(|| format!("AtCoder のレスポンスを読めません: {url}"))?;
        Ok(parse_latest_submission(&html, contest, task_id))
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

    fn authenticated_get(&self, url: &str) -> Result<String> {
        let response = self
            .client
            .get(url)
            .send()
            .with_context(|| format!("AtCoder への接続に失敗しました: {url}"))?;
        if is_login_redirect(&response) {
            bail!("AtCoder のセッションが無効です。`atcli login` を実行してください");
        }
        let response = response
            .error_for_status()
            .with_context(|| format!("AtCoder がエラーを返しました: {url}"))?;
        response
            .text()
            .with_context(|| format!("AtCoder のレスポンスを読めません: {url}"))
    }
}

fn is_login_redirect(response: &Response) -> bool {
    response.status().is_redirection()
        && response
            .headers()
            .get(LOCATION)
            .and_then(|location| location.to_str().ok())
            .is_some_and(|location| {
                location == "/login"
                    || location.starts_with("/login?")
                    || location.starts_with("https://atcoder.jp/login")
            })
}

fn extract_revel_session(response: &Response) -> Option<String> {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .filter_map(|cookie| cookie.split(';').next())
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(name, value)| (name.trim() == "REVEL_SESSION").then(|| value.trim().to_owned()))
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static CSS selector must be valid")
}

fn element_text(element: ElementRef<'_>) -> String {
    element.text().collect::<String>().trim().to_owned()
}

fn normalized_element_text(element: ElementRef<'_>) -> String {
    element
        .text()
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_csrf_token(html: &str) -> Result<String> {
    let document = Html::parse_document(html);
    let input_selector = selector("input[name=\"csrf_token\"]");
    document
        .select(&input_selector)
        .find_map(|input| input.value().attr("value"))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .context("AtCoder のページに csrf_token がありません")
}

pub fn parse_submit_page(html: &str, task_id: &str) -> Result<SubmitPage> {
    let document = Html::parse_document(html);
    let csrf_token = parse_csrf_token(html)?;
    let targeted_selector = Selector::parse(&format!("[id=\"select-lang-{task_id}\"] option")).ok();
    let generic_selector = selector("select[name=\"data.LanguageId\"] option");
    let targeted = targeted_selector
        .as_ref()
        .map_or_else(Vec::new, |query| parse_languages(&document, query));
    let languages = if targeted.is_empty() {
        parse_languages(&document, &generic_selector)
    } else {
        targeted
    };
    if languages.is_empty() {
        bail!("{task_id} の提出言語が見つかりません");
    }
    let captcha_selector = selector(
        ".cf-challenge, .cf-turnstile, [name=\"cf-turnstile-response\"], \
         script[src*=\"challenges.cloudflare.com/turnstile\"]",
    );
    let requires_captcha = document.select(&captcha_selector).next().is_some();
    Ok(SubmitPage {
        csrf_token,
        languages,
        requires_captcha,
    })
}

fn parse_languages(document: &Html, option_selector: &Selector) -> Vec<Language> {
    let mut seen = HashSet::new();
    document
        .select(option_selector)
        .filter_map(|option| {
            let id = option.value().attr("value")?.trim();
            if id.is_empty() || !seen.insert(id.to_owned()) {
                return None;
            }
            Some(Language {
                id: id.to_owned(),
                name: normalized_element_text(option),
            })
        })
        .collect()
}

fn parse_alert_message(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let alert_selector = selector(".alert-danger, .alert-warning");
    document
        .select(&alert_selector)
        .map(normalized_element_text)
        .find(|message| !message.is_empty())
}

pub fn parse_latest_submission(html: &str, contest: &str, task_id: &str) -> Option<Submission> {
    let document = Html::parse_document(html);
    let row_selector = selector("table tbody tr");
    let anchor_selector = selector("a[href]");
    let cell_selector = selector("td");
    let label_selector = selector("span.label");
    let task_path = format!("/contests/{contest}/tasks/{task_id}");
    let submission_prefix = format!("/contests/{contest}/submissions/");

    document.select(&row_selector).find_map(|row| {
        let matches_task = row.select(&anchor_selector).any(|anchor| {
            anchor
                .value()
                .attr("href")
                .is_some_and(|href| href == task_path)
        });
        if !matches_task {
            return None;
        }

        let (id, path) = row.select(&anchor_selector).find_map(|anchor| {
            let href = anchor.value().attr("href")?;
            let suffix = href.strip_prefix(&submission_prefix)?;
            let id = suffix.trim_end_matches('/').parse::<u64>().ok()?;
            Some((id, href.to_owned()))
        })?;
        let cells = row.select(&cell_selector).collect::<Vec<_>>();
        let result = row
            .select(&label_selector)
            .map(normalized_element_text)
            .find(|value| !value.is_empty())
            .or_else(|| cells.get(6).map(|cell| normalized_element_text(*cell)))
            .unwrap_or_default();
        let language = cells
            .get(3)
            .map_or_else(String::new, |cell| normalized_element_text(*cell));

        Some(Submission {
            id,
            url: format!("{BASE_URL}{path}"),
            language,
            result,
        })
    })
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

    use super::{
        parse_contest_tasks, parse_csrf_token, parse_latest_submission, parse_submit_page,
        parse_task_page, replace_samples,
    };

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

    #[test]
    fn parses_csrf_and_languages_for_requested_task() {
        let html = r#"
          <form name="form_logout">
            <input type="hidden" name="csrf_token" value="token&#43;value=" />
          </form>
          <div id="select-lang-abc300_a">
            <select name="data.LanguageId">
              <option value="">-</option>
              <option value="5001">C++20 (GCC 12.2)</option>
              <option value="5002"> C++23
                (GCC 15.2.0) </option>
            </select>
          </div>
          <div id="select-lang-abc300_b">
            <select name="data.LanguageId">
              <option value="5078">Python (CPython 3.11.4)</option>
            </select>
          </div>
        "#;

        assert_eq!(parse_csrf_token(html).unwrap(), "token+value=");
        let page = parse_submit_page(html, "abc300_a").unwrap();
        assert_eq!(page.csrf_token, "token+value=");
        assert_eq!(page.languages.len(), 2);
        assert_eq!(page.languages[0].id, "5001");
        assert_eq!(page.languages[1].name, "C++23 (GCC 15.2.0)");
        assert!(!page.requires_captcha);
    }

    #[test]
    fn detects_turnstile_while_preserving_submit_page_data() {
        let html = r#"
          <form>
            <input type="hidden" name="csrf_token" value="token" />
            <div id="select-lang-abc300_a">
              <select><option value="6017">C++23 (GCC 15.2.0)</option></select>
            </div>
            <script src="https://challenges.cloudflare.com/turnstile/v0/api.js"></script>
            <div class="cf-challenge" data-sitekey="site-key"></div>
          </form>
        "#;

        let page = parse_submit_page(html, "abc300_a").unwrap();
        assert!(page.requires_captcha);
        assert_eq!(page.csrf_token, "token");
        assert_eq!(page.languages.len(), 1);
        assert_eq!(page.languages[0].id, "6017");
    }

    #[test]
    fn rejects_submit_pages_missing_required_data() {
        let without_csrf = r#"
          <div id="select-lang-abc300_a">
            <select><option value="6017">C++23 (GCC 15.2.0)</option></select>
          </div>
        "#;
        let without_languages = r#"
          <input type="hidden" name="csrf_token" value="token" />
          <div id="select-lang-abc300_a"><select><option value=""></option></select></div>
        "#;

        assert!(
            parse_submit_page(without_csrf, "abc300_a")
                .err()
                .unwrap()
                .to_string()
                .contains("csrf_token")
        );
        assert!(
            parse_submit_page(without_languages, "abc300_a")
                .err()
                .unwrap()
                .to_string()
                .contains("提出言語")
        );
    }

    #[test]
    fn parses_latest_submission_and_judging_state() {
        let html = r#"
          <table><tbody>
            <tr>
              <td><a href="/contests/abc300/submissions/999">2026-09-09</a></td>
              <td><a href="/contests/abc300/tasks/abc300_b">B</a></td>
              <td>user</td><td>Python</td><td>0</td><td>100 Byte</td>
              <td><span class="label label-danger">WA</span></td>
            </tr>
            <tr>
              <td><a href="/contests/abc300/submissions/123">2026-09-09</a></td>
              <td><a href="/contests/abc300/tasks/abc300_a">A</a></td>
              <td>user</td><td>C++ 23 (gcc 12.2)</td><td>0</td><td>100 Byte</td>
              <td><span class="label label-default">WJ</span></td>
            </tr>
          </tbody></table>
        "#;

        let submission = parse_latest_submission(html, "abc300", "abc300_a").unwrap();
        assert_eq!(submission.id, 123);
        assert_eq!(submission.result, "WJ");
        assert_eq!(submission.language, "C++ 23 (gcc 12.2)");
        assert_eq!(
            submission.url,
            "https://atcoder.jp/contests/abc300/submissions/123"
        );
        assert!(!submission.is_finished());

        let judged = html.replace("label-default\">WJ", "label-success\">AC");
        assert!(
            parse_latest_submission(&judged, "abc300", "abc300_a")
                .unwrap()
                .is_finished()
        );
    }
}
