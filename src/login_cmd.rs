use std::{
    env,
    io::{self, Write},
};

use anyhow::{Context, Result, bail};

use crate::{
    atcoder::AtCoderClient,
    session::{Session, SessionStore},
};

const USERNAME_ENV: &str = "ATCODER_USERNAME";
const PASSWORD_ENV: &str = "ATCODER_PASSWORD";
const SESSION_ENV: &str = "ATCODER_REVEL_SESSION";

pub fn login(import_session: bool) -> Result<()> {
    let store = SessionStore::discover()?;
    let session = if import_session || env::var_os(SESSION_ENV).is_some() {
        imported_session()?
    } else {
        credential_login()?
    };
    let client = AtCoderClient::with_session(&session)?;
    if !client.session_is_valid()? {
        bail!("REVEL_SESSION が無効か期限切れです");
    }
    store.save(&session)?;
    println!("Logged in; session saved to {}", store.path().display());
    Ok(())
}

pub fn logout() -> Result<()> {
    let store = SessionStore::discover()?;
    if store.remove()? {
        println!("Logged out; removed {}", store.path().display());
    } else {
        println!("Already logged out");
    }
    Ok(())
}

fn imported_session() -> Result<Session> {
    let value = match env::var(SESSION_ENV) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => rpassword::prompt_password("REVEL_SESSION: ")
            .context("REVEL_SESSION を読み取れません")?,
        Err(env::VarError::NotUnicode(_)) => bail!("{SESSION_ENV} が UTF-8 ではありません"),
    };
    Session::new(&value)
}

fn credential_login() -> Result<Session> {
    let username = match env::var(USERNAME_ENV) {
        Ok(username) => username,
        Err(env::VarError::NotPresent) => prompt("AtCoder username: ")?,
        Err(env::VarError::NotUnicode(_)) => bail!("{USERNAME_ENV} が UTF-8 ではありません"),
    };
    if username.trim().is_empty() {
        bail!("AtCoder username を空にはできません");
    }
    let password = match env::var(PASSWORD_ENV) {
        Ok(password) => password,
        Err(env::VarError::NotPresent) => rpassword::prompt_password("AtCoder password: ")
            .context("AtCoder password を読み取れません")?,
        Err(env::VarError::NotUnicode(_)) => bail!("{PASSWORD_ENV} が UTF-8 ではありません"),
    };
    if password.is_empty() {
        bail!("AtCoder password を空にはできません");
    }
    AtCoderClient::new()?.login(username.trim(), &password)
}

fn prompt(message: &str) -> Result<String> {
    print!("{message}");
    io::stdout().flush().context("プロンプトを表示できません")?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .context("標準入力を読めません")?;
    Ok(value.trim().to_owned())
}
