use std::path::Path;

use anyhow::Result;

use atcli_atcoder::client::{AtCoderClient, replace_samples};
use atcli_core::model::ProblemMeta;

pub fn run(problem_dir: &Path) -> Result<()> {
    let mut meta = ProblemMeta::read(problem_dir)?;
    let client = AtCoderClient::with_saved_session()?;
    println!("Fetching {}...", meta.url);
    let page = client.task_page(&meta.url)?;
    replace_samples(problem_dir, &page.samples)?;
    meta.time_limit_ms = page.time_limit_ms;
    meta.interactive = page.interactive;
    meta.write(problem_dir)?;
    println!(
        "Updated {} samples in {}",
        page.samples.len(),
        problem_dir.display()
    );
    Ok(())
}
