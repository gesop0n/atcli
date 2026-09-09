use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const STATE_DIR_ENV: &str = "ATCLI_STATE_DIR";

#[derive(Clone, Deserialize, Serialize)]
pub struct Session {
    revel_session: String,
}

impl Session {
    pub fn new(revel_session: &str) -> Result<Self> {
        let revel_session = revel_session.trim().to_owned();
        if revel_session.is_empty()
            || revel_session
                .chars()
                .any(|character| character.is_control() || character == ';')
        {
            bail!("REVEL_SESSION の値が不正です");
        }
        Ok(Self { revel_session })
    }

    pub fn value(&self) -> &str {
        &self.revel_session
    }
}

#[derive(Debug)]
pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn discover() -> Result<Self> {
        let path = match env::var_os(STATE_DIR_ENV) {
            Some(base) => PathBuf::from(base).join("atcli/session.json"),
            None => dirs::home_dir()
                .context("ホームディレクトリを取得できません。ATCLI_STATE_DIR を設定してください")?
                .join(".atcli/session.json"),
        };
        Ok(Self { path })
    }

    #[cfg(test)]
    fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<Session>> {
        let source = match fs::read_to_string(&self.path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("セッションを読めません: {}", self.path.display()));
            }
        };
        let session: Session = serde_json::from_str(&source)
            .with_context(|| format!("セッションの形式が不正です: {}", self.path.display()))?;
        Session::new(&session.revel_session).map(Some)
    }

    pub fn save(&self, session: &Session) -> Result<()> {
        let parent = self
            .path
            .parent()
            .context("セッション保存先の親ディレクトリがありません")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("セッション保存先を作成できません: {}", parent.display()))?;

        set_directory_permissions(parent)?;
        let source = serde_json::to_string_pretty(session).context("セッションを変換できません")?;
        write_private_file(&self.path, format!("{source}\n").as_bytes())
    }

    pub fn remove(&self) -> Result<bool> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error)
                .with_context(|| format!("セッションを削除できません: {}", self.path.display())),
        }
    }
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("保存先の権限を設定できません: {}", path.display()))
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("セッションを書き込めません: {}", path.display()))?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .with_context(|| format!("セッションの権限を設定できません: {}", path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("セッションを書き込めません: {}", path.display()))
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    fs::write(path, contents)
        .with_context(|| format!("セッションを書き込めません: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{Session, SessionStore};

    #[test]
    fn saves_loads_and_removes_session() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".atcli/session.json");
        let store = SessionStore::at(path.clone());
        let session = Session::new("secret-cookie").unwrap();

        store.save(&session).unwrap();
        assert_eq!(store.load().unwrap().unwrap().value(), "secret-cookie");
        assert!(store.remove().unwrap());
        assert!(!store.remove().unwrap());
        assert!(store.load().unwrap().is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            store.save(&session).unwrap();
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn rejects_values_that_can_escape_a_cookie() {
        assert!(Session::new("").is_err());
        assert!(Session::new("value; other=cookie").is_err());
        assert!(Session::new("value\nheader").is_err());
    }
}
