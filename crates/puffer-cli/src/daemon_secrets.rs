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
        // The elevated helper re-imports every row (v10/v11 + v20) into the SAME
        // vault, so its counts — not the in-process pass's — describe the final
        // outcome. Replace imported AND skipped together so they stay consistent:
        // the in-process pass counted v20 rows as skipped, but the helper imported
        // them. If the helper didn't run (user declined UAC), keep the in-process
        // report, whose skipped count already reflects the un-imported v20 rows.
        if let Some((imported, skipped, errors)) = run_windows_v20_helper(paths) {
            report.imported = imported;
            report.skipped = skipped;
            if errors > 0 {
                report.errors.push(format!(
                    "{errors} Chrome credential(s) failed during the elevated v20 import"
                ));
            }
        }
    }
    Ok(report)
}

/// Spawns `puffer __win-chrome-import` (which self-elevates via UAC and imports
/// v10+v20 into the vault). Returns the helper's `(imported, skipped, errors)`
/// counts, or None if it could not run / the user declined elevation.
#[cfg(target_os = "windows")]
fn run_windows_v20_helper(paths: &ConfigPaths) -> Option<(usize, usize, usize)> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe().ok()?;
    let vault_dir = paths.user_config_dir.to_string_lossy().to_string();
    // Read the helper's STDOUT (where the user stage prints the SYSTEM result line)
    // rather than reading a predictable shared temp file ourselves: this drops the
    // daemon's dependence on a guessable path, and piped stdio keeps the child off
    // the daemon's handshake stdout pipe. (The user stage still relays the result via
    // its own per-pid temp file, so a racing same-user process could at most forge
    // the COUNTS reported here — never a secret; these are non-sensitive tallies.)
    let output = std::process::Command::new(exe)
        .args(["__win-chrome-import", "--vault-dir", &vault_dir])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    // The success line is "CHROME_IMPORT_OK imported=N skipped=N errors=N ...".
    // A declined/failed run leaves no fresh OK marker, so require it before
    // trusting any counts (otherwise the import is treated as not-run -> None).
    if !text.contains("CHROME_IMPORT_OK") {
        return None;
    }
    let field = |key: &str| -> Option<usize> {
        text.split_whitespace()
            .find_map(|tok| tok.strip_prefix(key).and_then(|n| n.parse::<usize>().ok()))
    };
    Some((
        field("imported=")?,
        field("skipped=").unwrap_or(0),
        field("errors=").unwrap_or(0),
    ))
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
