//! Telegram Desktop `tdata` import support.
//!
//! Importing copies one local Telegram Desktop authorization key into the
//! grammers session file used by the `telegram-user` subscriber. The import is
//! local-only: no secret leaves the machine, and the caller verifies the
//! imported session by reconnecting to Telegram before reporting success.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use grammers_client::session::Session;
use hermes_tdata::TDesktopBuilder;

use crate::state::{PersistedCredentials, SkillEnv, DEFAULT_API_HASH, DEFAULT_API_ID};

#[cfg(target_os = "macos")]
mod macos_native;

/// Options for importing Telegram Desktop local authentication.
#[derive(Debug, Clone, Default)]
pub struct TdataImportOptions {
    /// Optional path to a Telegram Desktop `tdata` directory.
    pub path: Option<String>,
    /// Optional Telegram Desktop local passcode.
    pub passcode: Option<String>,
    /// Optional zero-based Telegram Desktop account slot.
    pub account_index: Option<usize>,
    /// Optional Telegram Desktop key file name.
    pub key_file: Option<String>,
}

/// Summary of a completed Telegram Desktop import.
#[derive(Debug, Clone)]
pub struct TdataImportOutcome {
    /// Source format used for the import.
    pub source_kind: ImportSourceKind,
    /// Path to the source directory.
    pub source_path: PathBuf,
    /// Zero-based Telegram Desktop account slot that was imported.
    pub account_index: usize,
    /// Number of accounts found in the source directory.
    pub accounts_count: usize,
    /// Telegram user id extracted from the selected account, when available
    /// before reconnect verification.
    pub user_id: Option<i64>,
    /// Telegram datacenter id for the selected account.
    pub dc_id: i32,
    /// Candidate datacenter ids to try when reconnect verification fails.
    pub candidate_dc_ids: Vec<i32>,
    /// Destination grammers session path.
    pub session_path: PathBuf,
}

/// Local Telegram storage format used for an import.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ImportSourceKind {
    /// Telegram Desktop `tdata` storage.
    Tdata,
    /// Native macOS Telegram.app Postbox storage.
    MacosNative,
}

impl ImportSourceKind {
    /// Returns the stable identifier used in JSON event payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tdata => "tdata",
            Self::MacosNative => "macos_native",
        }
    }
}

/// Imports one Telegram Desktop account into the subscriber session file.
pub fn import_tdata(
    env: &SkillEnv,
    options: TdataImportOptions,
) -> anyhow::Result<TdataImportOutcome> {
    match import_tdesktop(env, &options) {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            #[cfg(target_os = "macos")]
            {
                if macos_native::should_try_native_fallback(&options) {
                    return macos_native::import_native(env, &options).with_context(|| {
                        format!(
                            "Telegram Desktop import failed ({error}); native macOS Telegram.app import failed"
                        )
                    });
                }
            }
            Err(error)
        }
    }
}

fn import_tdesktop(
    env: &SkillEnv,
    options: &TdataImportOptions,
) -> anyhow::Result<TdataImportOutcome> {
    let source_path = resolve_tdata_path(options.path.as_deref())?;
    let mut builder = TDesktopBuilder::new(&source_path);
    if let Some(passcode) = options.passcode.as_deref() {
        builder = builder.passcode(passcode);
    }
    if let Some(key_file) = options.key_file.as_deref() {
        builder = builder.key_file(key_file);
    }

    let tdesktop = builder
        .build()
        .with_context(|| format!("read Telegram Desktop tdata at {}", source_path.display()))?;
    let account_index = options.account_index.unwrap_or(0);
    let account = tdesktop.account(account_index).ok_or_else(|| {
        anyhow::anyhow!(
            "Telegram Desktop account index {account_index} is unavailable; found {} account(s)",
            tdesktop.accounts_count()
        )
    })?;
    let session = account.to_grammers_session()?;
    save_imported_session(&env.session_path, &session)?;
    persist_import_credentials(env)?;

    Ok(TdataImportOutcome {
        source_kind: ImportSourceKind::Tdata,
        source_path,
        account_index,
        accounts_count: tdesktop.accounts_count(),
        user_id: Some(account.user_id()),
        dc_id: account.dc_id(),
        candidate_dc_ids: vec![account.dc_id()],
        session_path: env.session_path.clone(),
    })
}

fn resolve_tdata_path(path: Option<&str>) -> anyhow::Result<PathBuf> {
    let path = match path {
        Some(path) if !path.trim().is_empty() => expand_home(path.trim())?,
        _ => default_tdata_path().ok_or_else(|| {
            anyhow::anyhow!("could not resolve the default Telegram Desktop tdata path")
        })?,
    };
    if path.exists() {
        Ok(path)
    } else {
        anyhow::bail!(
            "Telegram Desktop tdata path does not exist: {}",
            path.display()
        )
    }
}

fn expand_home(path: &str) -> anyhow::Result<PathBuf> {
    if path == "~" {
        return dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve home directory"));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return dirs::home_dir()
            .map(|home| home.join(rest))
            .ok_or_else(|| anyhow::anyhow!("could not resolve home directory"));
    }
    Ok(PathBuf::from(path))
}

fn default_tdata_path() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        dirs::home_dir().map(|home| home.join(".local/share/TelegramDesktop/tdata"))
    }

    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|home| home.join("Library/Application Support/Telegram Desktop/tdata"))
    }

    #[cfg(target_os = "windows")]
    {
        dirs::data_local_dir().map(|data| data.join("Telegram Desktop/tdata"))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

fn save_imported_session(path: &Path, session: &Session) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create Telegram session parent {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, session.save()).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

fn persist_import_credentials(env: &SkillEnv) -> anyhow::Result<()> {
    persist_import_credentials_pair(env, DEFAULT_API_ID, DEFAULT_API_HASH)
}

fn persist_import_credentials_pair(
    env: &SkillEnv,
    api_id: i32,
    api_hash: &str,
) -> anyhow::Result<()> {
    PersistedCredentials {
        api_id: Some(api_id),
        api_hash: Some(api_hash.to_string()),
        phone: None,
    }
    .save(&env.credentials_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_home_paths() {
        let home = dirs::home_dir().unwrap();

        assert_eq!(expand_home("~").unwrap(), home);
        assert!(expand_home("~/Telegram/tdata")
            .unwrap()
            .ends_with("Telegram/tdata"));
        assert_eq!(
            expand_home("/tmp/tdata").unwrap(),
            PathBuf::from("/tmp/tdata")
        );
    }

    #[test]
    fn saves_imported_session_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("telegram.session");
        let session = Session::new();
        session.set_user(42, 2, false);

        save_imported_session(&path, &session).unwrap();

        let loaded = Session::load_file(&path).unwrap();
        assert_eq!(loaded.get_user().unwrap().id, 42);
    }
}
