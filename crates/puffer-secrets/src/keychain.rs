use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rand::RngCore;
#[cfg(not(target_os = "macos"))]
use std::fs;
use std::path::Path;
#[cfg(not(target_os = "macos"))]
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

const KEY_BYTES: usize = 32;
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "Puffer Code Secrets";
#[cfg(target_os = "macos")]
const KEYCHAIN_ACCOUNT: &str = "puffer-code";
const OVERRIDE_KEY_ENV: &str = "PUFFER_SECRET_STORE_KEY";

/// Loads or creates the vault encryption key from the platform key source.
pub(crate) fn load_or_create_key(store_path: &Path) -> Result<[u8; KEY_BYTES]> {
    if let Ok(encoded) = std::env::var(OVERRIDE_KEY_ENV) {
        return decode_key(encoded.trim());
    }
    #[cfg(target_os = "macos")]
    {
        let _ = store_path;
        return load_or_create_macos_key();
    }
    #[cfg(not(target_os = "macos"))]
    {
        load_or_create_file_key(store_path)
    }
}

#[cfg(target_os = "macos")]
fn load_or_create_macos_key() -> Result<[u8; KEY_BYTES]> {
    if let Some(encoded) = read_keychain_password()? {
        return decode_key(&encoded);
    }
    let key = random_key();
    if let Err(error) = write_keychain_password(&BASE64.encode(key)) {
        if let Some(encoded) = read_keychain_password()? {
            return decode_key(&encoded);
        }
        return Err(error);
    }
    Ok(key)
}

#[cfg(target_os = "macos")]
fn read_keychain_password() -> Result<Option<String>> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-w",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            KEYCHAIN_ACCOUNT,
        ])
        .output()
        .context("read Puffer secret key from macOS Keychain")?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

#[cfg(target_os = "macos")]
fn write_keychain_password(encoded: &str) -> Result<()> {
    let status = Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            KEYCHAIN_ACCOUNT,
            "-w",
            encoded,
        ])
        .status()
        .context("write Puffer secret key to macOS Keychain")?;
    if !status.success() {
        anyhow::bail!("macOS Keychain rejected the Puffer secret key");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn load_or_create_file_key(store_path: &Path) -> Result<[u8; KEY_BYTES]> {
    let key_path = fallback_key_path(store_path);
    if key_path.exists() {
        let encoded = fs::read_to_string(&key_path)
            .with_context(|| format!("read fallback secret key {}", key_path.display()))?;
        return decode_key(encoded.trim());
    }
    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create fallback key dir {}", parent.display()))?;
    }
    let key = random_key();
    fs::write(&key_path, BASE64.encode(key))
        .with_context(|| format!("write fallback secret key {}", key_path.display()))?;
    set_private_permissions(&key_path);
    Ok(key)
}

#[cfg(not(target_os = "macos"))]
fn fallback_key_path(store_path: &Path) -> PathBuf {
    store_path.with_extension("key")
}

fn random_key() -> [u8; KEY_BYTES] {
    let mut key = [0u8; KEY_BYTES];
    rand::thread_rng().fill_bytes(&mut key);
    key
}

fn decode_key(encoded: &str) -> Result<[u8; KEY_BYTES]> {
    let bytes = BASE64.decode(encoded).context("decode Puffer secret key")?;
    if bytes.len() != KEY_BYTES {
        anyhow::bail!("Puffer secret key has invalid length");
    }
    let mut key = [0u8; KEY_BYTES];
    key.copy_from_slice(&bytes);
    Ok(key)
}

#[cfg(not(target_os = "macos"))]
fn set_private_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}
