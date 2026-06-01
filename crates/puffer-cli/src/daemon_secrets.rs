use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_secrets::{ChromeImportReport, SecretUpsert, SecretVault};
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
pub(crate) fn import_chrome_secrets(paths: &ConfigPaths) -> Result<ChromeImportReport> {
    vault(paths)?.import_chrome_saved_credentials()
}

fn vault(paths: &ConfigPaths) -> Result<SecretVault> {
    SecretVault::open(SecretVault::default_path(&paths.user_config_dir))
        .context("open encrypted secret store")
}
