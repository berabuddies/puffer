use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_secrets::{
    BrowserSource, ImportReport, SecretUpsert, SecretVault, SourceAvailability,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveSecretParams {
    #[serde(default)]
    id: Option<String>,
    label: String,
    value: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    origin: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteSecretParams {
    id: String,
}

/// Saves one encrypted secret from a desktop settings request.
pub(crate) fn save_secret(paths: &ConfigPaths, params: &Value) -> Result<()> {
    let input: SaveSecretParams =
        serde_json::from_value(params.clone()).context("invalid secret save params")?;
    vault(paths)?.put(SecretUpsert {
        id: input.id,
        label: input.label,
        description: input.description,
        value: input.value,
        username: input.username,
        origin: input.origin,
        source: "manual".to_string(),
    })?;
    Ok(())
}

/// Deletes one encrypted secret from a desktop settings request.
pub(crate) fn delete_secret(paths: &ConfigPaths, params: &Value) -> Result<bool> {
    let input: DeleteSecretParams =
        serde_json::from_value(params.clone()).context("invalid secret delete params")?;
    vault(paths)?.delete(&input.id)
}

/// Imports saved Chrome credentials into the encrypted Puffer secret vault.
pub(crate) fn import_chrome_secrets(paths: &ConfigPaths) -> Result<ImportReport> {
    vault(paths)?.import_chrome_saved_credentials()
}

/// Imports saved credentials from one named source (browser or 1Password).
/// 1Password imports every accessible vault.
pub(crate) fn import_browser_secrets(
    paths: &ConfigPaths,
    source_id: &str,
) -> Result<ImportReport> {
    if source_id == "1password" {
        return vault(paths)?.sync_onepassword_references();
    }
    let source = BrowserSource::from_id(source_id)
        .with_context(|| format!("unknown import source `{source_id}`"))?;
    #[allow(unused_mut)]
    let mut report = vault(paths)?.sync_browser_source(source)?;
    // On Windows, Chromium v20 (App-Bound Encryption) keys are SYSTEM-protected,
    // so the user-context daemon cannot decrypt them in-process. Launch the
    // self-elevating helper (one user-consented UAC prompt) which imports v20
    // into the SAME vault; elevation lasts only for the import.
    #[cfg(target_os = "windows")]
    if source_id == "chrome" {
        if let Some(total) = run_windows_v20_helper(paths) {
            report.imported = total;
        }
    }
    Ok(report)
}

/// Spawns `puffer __win-chrome-import` (which self-elevates via UAC and imports
/// v10+v20 into the vault). Returns the helper's total imported count, or None if
/// it could not run / the user declined elevation.
#[cfg(target_os = "windows")]
fn run_windows_v20_helper(paths: &ConfigPaths) -> Option<usize> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe().ok()?;
    let vault_dir = paths.user_config_dir.to_string_lossy().to_string();
    std::process::Command::new(exe)
        .args(["__win-chrome-import", "--vault-dir", &vault_dir])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .ok()?;
    let user = std::env::var("USERNAME").ok()?;
    let text = std::fs::read_to_string(format!(
        "C:\\Users\\{user}\\AppData\\Local\\Temp\\puffer_chrome_import.txt"
    ))
    .ok()?;
    text.split_whitespace()
        .find_map(|tok| tok.strip_prefix("imported=").and_then(|n| n.parse::<usize>().ok()))
}

/// Lists every browser import source and whether it is currently available.
pub(crate) fn list_secret_sources() -> Vec<SourceAvailability> {
    puffer_secrets::available_browser_sources()
}

/// Imports 1Password logins from a `.1pux` export file (no `op` CLI), every vault
/// in the file.
pub(crate) fn import_onepassword_export(paths: &ConfigPaths, path: &str) -> Result<ImportReport> {
    vault(paths)?.sync_onepassword_export(std::path::Path::new(path))
}

fn vault(paths: &ConfigPaths) -> Result<SecretVault> {
    SecretVault::open(SecretVault::default_path(&paths.user_config_dir))
        .context("open encrypted secret store")
}
