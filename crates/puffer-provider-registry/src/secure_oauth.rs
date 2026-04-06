use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

const TEST_PLAINTEXT_BACKEND_ENV: &str = "PUFFER_TEST_PLAINTEXT_OAUTH_STORAGE";
#[cfg(target_os = "macos")]
const MACOS_CREDENTIALS_SERVICE_SUFFIX: &str = "-credentials";
#[cfg(target_os = "macos")]
const SECURITY_STDIN_LINE_LIMIT: usize = 4096 - 64;

/// Stores the secret OAuth token payload for one provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OAuthSecret {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct OAuthSecretStore {
    #[serde(default)]
    providers: BTreeMap<String, OAuthSecret>,
}

/// Loads the OAuth secret payload for one provider from the configured secure backend.
pub(crate) fn load_oauth_secret(path: &Path, provider_id: &str) -> Result<Option<OAuthSecret>> {
    let store = read_secret_store(path)?;
    Ok(store.providers.get(provider_id).cloned())
}

/// Saves the OAuth secret payload for one provider using the configured secure backend.
pub(crate) fn store_oauth_secret(
    path: &Path,
    provider_id: &str,
    secret: &OAuthSecret,
) -> Result<()> {
    let primary_before = read_primary_secret_store(path)?;
    let mut store = read_secret_store(path)?;
    store
        .providers
        .insert(provider_id.to_string(), secret.clone());
    if write_primary_secret_store(path, &store).is_ok() {
        if primary_before.is_none() {
            delete_plaintext_secret_store(path)?;
        }
        return Ok(());
    }

    write_plaintext_secret_store(path, &store)?;
    if primary_before.is_some() {
        let _ = delete_primary_secret_store(path);
    }
    Ok(())
}

/// Deletes the OAuth secret payload for one provider from all configured backends.
pub(crate) fn delete_oauth_secret(path: &Path, provider_id: &str) -> Result<()> {
    let mut changed = false;
    if let Some(mut primary) = read_primary_secret_store(path)? {
        if primary.providers.remove(provider_id).is_some() {
            changed = true;
            if primary.providers.is_empty() {
                let _ = delete_primary_secret_store(path);
            } else {
                let _ = write_primary_secret_store(path, &primary);
            }
        }
    }
    let mut plaintext = read_plaintext_secret_store(path)?;
    if plaintext.providers.remove(provider_id).is_some() {
        changed = true;
        if plaintext.providers.is_empty() {
            delete_plaintext_secret_store(path)?;
        } else {
            write_plaintext_secret_store(path, &plaintext)?;
        }
    }
    if !changed {
        return Ok(());
    }
    Ok(())
}

fn read_secret_store(path: &Path) -> Result<OAuthSecretStore> {
    if let Some(primary) = read_primary_secret_store(path)? {
        return Ok(primary);
    }
    read_plaintext_secret_store(path)
}

fn read_primary_secret_store(path: &Path) -> Result<Option<OAuthSecretStore>> {
    if !uses_macos_keychain_backend() {
        return Ok(None);
    }
    read_macos_keychain_secret_store(path)
}

fn write_primary_secret_store(path: &Path, store: &OAuthSecretStore) -> Result<()> {
    if !uses_macos_keychain_backend() {
        anyhow::bail!("macOS keychain backend is unavailable")
    }
    write_macos_keychain_secret_store(path, store)
}

fn delete_primary_secret_store(path: &Path) -> Result<()> {
    if !uses_macos_keychain_backend() {
        return Ok(());
    }
    delete_macos_keychain_secret_store(path)
}

fn read_plaintext_secret_store(path: &Path) -> Result<OAuthSecretStore> {
    let storage_path = plaintext_storage_path(path);
    if !storage_path.exists() {
        return Ok(OAuthSecretStore::default());
    }
    let raw = fs::read_to_string(&storage_path)
        .with_context(|| format!("failed to read {}", storage_path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", storage_path.display()))
}

fn write_plaintext_secret_store(path: &Path, store: &OAuthSecretStore) -> Result<()> {
    let storage_path = plaintext_storage_path(path);
    if let Some(parent) = storage_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&storage_path, serde_json::to_string(store)?)
        .with_context(|| format!("failed to write {}", storage_path.display()))?;
    set_owner_only_permissions(&storage_path)?;
    Ok(())
}

fn delete_plaintext_secret_store(path: &Path) -> Result<()> {
    let storage_path = plaintext_storage_path(path);
    match fs::remove_file(&storage_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("failed to delete {}", storage_path.display()))
        }
    }
}

fn plaintext_storage_path(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".credentials.json")
}

fn uses_macos_keychain_backend() -> bool {
    cfg!(target_os = "macos") && std::env::var_os(TEST_PLAINTEXT_BACKEND_ENV).is_none()
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("failed to chmod {}", path.display()))
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn read_macos_keychain_secret_store(path: &Path) -> Result<Option<OAuthSecretStore>> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-a",
            &macos_keychain_username(),
            "-w",
            "-s",
            &macos_keychain_service_name(path),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .context("failed to invoke security find-generic-password")?;
    if !output.status.success() {
        return Ok(None);
    }
    let raw = match String::from_utf8(output.stdout) {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    match serde_json::from_str(raw.trim()) {
        Ok(store) => Ok(Some(store)),
        Err(_) => Ok(None),
    }
}

#[cfg(not(target_os = "macos"))]
fn read_macos_keychain_secret_store(_path: &Path) -> Result<Option<OAuthSecretStore>> {
    Ok(None)
}

#[cfg(target_os = "macos")]
fn write_macos_keychain_secret_store(path: &Path, store: &OAuthSecretStore) -> Result<()> {
    let raw =
        serde_json::to_string(store).context("failed to serialize keychain OAuth secret store")?;
    let hex = hex_encode(raw.as_bytes());
    let username = macos_keychain_username();
    let service = macos_keychain_service_name(path);
    let command = format!(
        "add-generic-password -U -a \"{}\" -s \"{}\" -X \"{}\"\n",
        escape_security_argument(&username),
        escape_security_argument(&service),
        hex,
    );

    let status = if command.len() <= SECURITY_STDIN_LINE_LIMIT {
        Command::new("security")
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write as _;
                if let Some(stdin) = child.stdin.as_mut() {
                    stdin.write_all(command.as_bytes())?;
                }
                child.wait()
            })
            .context("failed to invoke security -i for OAuth secret storage")?
    } else {
        Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-a",
                &username,
                "-s",
                &service,
                "-X",
                &hex,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("failed to invoke security add-generic-password")?
    };

    if !status.success() {
        anyhow::bail!("security add-generic-password failed")
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn write_macos_keychain_secret_store(_path: &Path, _store: &OAuthSecretStore) -> Result<()> {
    anyhow::bail!("macOS keychain backend is unavailable")
}

#[cfg(target_os = "macos")]
fn delete_macos_keychain_secret_store(path: &Path) -> Result<()> {
    let status = Command::new("security")
        .args([
            "delete-generic-password",
            "-a",
            &macos_keychain_username(),
            "-s",
            &macos_keychain_service_name(path),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to invoke security delete-generic-password")?;
    if status.success() {
        return Ok(());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn delete_macos_keychain_secret_store(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_keychain_username() -> String {
    std::env::var("USER").unwrap_or_else(|_| "puffer-code-user".to_string())
}

#[cfg(target_os = "macos")]
fn macos_keychain_service_name(path: &Path) -> String {
    let config_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let default_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".puffer");
    let dir_hash = if std::env::var_os("PUFFER_HOME").is_none() && config_dir == default_dir {
        String::new()
    } else {
        format!("-{}", short_path_hash(config_dir))
    };
    format!("Puffer Code{MACOS_CREDENTIALS_SERVICE_SUFFIX}{dir_hash}")
}

#[cfg(target_os = "macos")]
fn short_path_hash(path: &Path) -> String {
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

#[cfg(target_os = "macos")]
fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

#[cfg(target_os = "macos")]
fn escape_security_argument(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
