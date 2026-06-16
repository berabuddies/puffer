//! Chromium-family (Chrome / Edge / Brave) saved-login extraction.
//!
//! All Chromium browsers share the same on-disk layout — a per-profile SQLite
//! `Login Data` database plus a browser-level key — and differ only by install
//! paths and the OS key store that protects the master key:
//! - **macOS**: a "<Browser> Safe Storage" Keychain item → PBKDF2-SHA1 →
//!   AES-128-CBC (`v10`/`v11` blobs).
//! - **Windows**: `Local State` → DPAPI-wrapped AES key → AES-256-GCM (`v10`),
//!   plus App-Bound Encryption (`v20`) — implemented in the `windows` module.
//! - **Linux**: Secret Service / `peanuts` fallback → AES-128-CBC (`v10`).
//!
//! This module exposes the per-OS pieces behind one [`Chromium`] selector so the
//! source registry can treat every variant uniformly.

use crate::ImportedCredential;
use anyhow::Result;

/// One supported Chromium-family browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chromium {
    Chrome,
    Edge,
    Brave,
}

impl Chromium {
    /// Profile-root path relative to the user profile/home for this OS.
    #[cfg(target_os = "macos")]
    fn user_data_root(self) -> &'static str {
        match self {
            Chromium::Chrome => "Library/Application Support/Google/Chrome",
            Chromium::Edge => "Library/Application Support/Microsoft Edge",
            Chromium::Brave => "Library/Application Support/BraveSoftware/Brave-Browser",
        }
    }

    /// macOS Keychain service name holding the "Safe Storage" key.
    #[cfg(target_os = "macos")]
    fn keychain_service(self) -> &'static str {
        match self {
            Chromium::Chrome => "Chrome Safe Storage",
            Chromium::Edge => "Microsoft Edge Safe Storage",
            Chromium::Brave => "Brave Safe Storage",
        }
    }

    /// macOS Keychain account name for the "Safe Storage" key.
    #[cfg(target_os = "macos")]
    fn keychain_account(self) -> &'static str {
        match self {
            Chromium::Chrome => "Chrome",
            Chromium::Edge => "Microsoft Edge",
            Chromium::Brave => "Brave",
        }
    }

    /// `User Data` root relative to `%LOCALAPPDATA%` on Windows.
    #[cfg(target_os = "windows")]
    fn user_data_root(self) -> &'static str {
        match self {
            Chromium::Chrome => "Google/Chrome/User Data",
            Chromium::Edge => "Microsoft/Edge/User Data",
            Chromium::Brave => "BraveSoftware/Brave-Browser/User Data",
        }
    }

    /// `User Data` root relative to the Linux config dir.
    #[cfg(target_os = "linux")]
    fn user_data_root(self) -> &'static str {
        match self {
            Chromium::Chrome => "google-chrome",
            Chromium::Edge => "microsoft-edge",
            Chromium::Brave => "BraveSoftware/Brave-Browser",
        }
    }
}

/// Loads decryptable saved credentials for one Chromium-family browser.
pub(crate) fn load_saved_credentials(variant: Chromium) -> Result<Vec<ImportedCredential>> {
    #[cfg(target_os = "macos")]
    {
        macos::load_saved_credentials(variant)
    }
    #[cfg(target_os = "windows")]
    {
        windows::load_saved_credentials(variant)
    }
    #[cfg(target_os = "linux")]
    {
        linux::load_saved_credentials(variant)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = variant;
        anyhow::bail!("Chromium credential import is unsupported on this platform")
    }
}

/// Chrome elevation-service hardcoded final-unwrap keys for App-Bound Encryption
/// (public constants from the runassu/xaitax research). After both DPAPI layers
/// are peeled, the 32-byte ABE key is still AEAD-encrypted under one of these,
/// selected by a flag byte. FLAG 0x01 = AES-256-GCM, 0x02 = ChaCha20-Poly1305.
/// (0x03 derives a per-machine key via CNG and is not handled here.) These are
/// Chrome-specific; Edge uses a COM-only route with a different key.
const ABE_AES_KEY_FLAG1: [u8; 32] = [
    0xB3, 0x1C, 0x6E, 0x24, 0x1A, 0xC8, 0x46, 0x72, 0x8D, 0xA9, 0xC1, 0xFA, 0xC4, 0x93, 0x66, 0x51,
    0xCF, 0xFB, 0x94, 0x4D, 0x14, 0x3A, 0xB8, 0x16, 0x27, 0x6B, 0xCC, 0x6D, 0xA0, 0x28, 0x47, 0x87,
];
const ABE_CHACHA_KEY_FLAG2: [u8; 32] = [
    0xE9, 0x8F, 0x37, 0xD7, 0xF4, 0xE1, 0xFA, 0x43, 0x3D, 0x19, 0x30, 0x4D, 0xC2, 0x25, 0x80, 0x42,
    0x09, 0x0E, 0x2D, 0x1D, 0x7E, 0xEA, 0x76, 0x70, 0xD4, 0x1F, 0x73, 0x8D, 0x08, 0x72, 0x96, 0x60,
];

/// Recovers the 32-byte App-Bound Encryption (`v20`) master key from the blob
/// left after both DPAPI layers (SYSTEM outer + interactive-user inner) have
/// been peeled off the `app_bound_encrypted_key`.
///
/// Post-DPAPI layout: `[u32 hdr_len][hdr][u32 content_len][flag(1)][iv(12)][ct(32)][tag(16)]`.
/// The key is AEAD-decrypted from `ct||tag` under the flag's hardcoded key with
/// `iv` as the nonce. This is pure crypto (no OS calls), so it is unit-tested
/// with synthetic fixtures; the DPAPI peeling that produces `post_dpapi` is the
/// OS-specific, privilege-gated step handled by the caller.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn unwrap_abe_key_material(post_dpapi: &[u8]) -> anyhow::Result<[u8; 32]> {
    use aes_gcm::aead::{Aead, KeyInit};
    use anyhow::{anyhow, bail, Context};

    fn take_u32(data: &[u8]) -> anyhow::Result<(usize, &[u8])> {
        let bytes = data.get(..4).context("ABE: truncated length")?;
        Ok((u32::from_le_bytes(bytes.try_into().unwrap()) as usize, &data[4..]))
    }
    let (hdr_len, rest) = take_u32(post_dpapi)?;
    let rest = rest.get(hdr_len..).context("ABE: truncated header")?;
    let (content_len, content) = take_u32(rest)?;
    let content = content.get(..content_len).context("ABE: truncated content")?;
    // Edge stores the 32-byte key directly after the two DPAPI layers (no
    // flag/AEAD wrap) — verified live decrypting a real Edge v20 blob. Chrome
    // wraps it under a flag-selected hardcoded key.
    if content_len == 32 {
        return content
            .try_into()
            .map_err(|_| anyhow!("ABE: 32-byte content is not a valid key"));
    }
    let flag = *content.first().context("ABE: missing flag")?;
    let iv = content.get(1..13).context("ABE: missing iv")?;
    let ct_tag = content.get(13..).context("ABE: missing ciphertext")?;
    let key = match flag {
        0x01 => aes_gcm::Aes256Gcm::new_from_slice(&ABE_AES_KEY_FLAG1)
            .unwrap()
            .decrypt(aes_gcm::Nonce::from_slice(iv), ct_tag)
            .map_err(|_| anyhow!("ABE flag1 AES-GCM unwrap failed"))?,
        0x02 => chacha20poly1305::ChaCha20Poly1305::new_from_slice(&ABE_CHACHA_KEY_FLAG2)
            .unwrap()
            .decrypt(chacha20poly1305::Nonce::from_slice(iv), ct_tag)
            .map_err(|_| anyhow!("ABE flag2 ChaCha20 unwrap failed"))?,
        0x03 => bail!("ABE flag 0x03 (per-machine CNG key) is not supported"),
        other => bail!("ABE: unknown flag 0x{other:02x}"),
    };
    key.as_slice()
        .try_into()
        .map_err(|_| anyhow!("ABE key is not 32 bytes"))
}

/// Reports whether this browser has at least one profile with a login database.
pub(crate) fn is_available(variant: Chromium) -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        !login_databases(variant).is_empty()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = variant;
        false
    }
}

/// Enumerates the `Login Data` SQLite files across every profile of `variant`.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn login_databases(variant: Chromium) -> Vec<std::path::PathBuf> {
    use std::fs;
    let Some(root) = user_data_dir(variant) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let login_data = path.join("Login Data");
        if login_data.is_file() {
            out.push(login_data);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Resolves the absolute `User Data` directory for `variant` on this OS.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn user_data_dir(variant: Chromium) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|home| home.join(variant.user_data_root()))
    }
    #[cfg(target_os = "windows")]
    {
        dirs::data_local_dir().map(|local| local.join(variant.user_data_root()))
    }
    #[cfg(target_os = "linux")]
    {
        dirs::config_dir().map(|config| config.join(variant.user_data_root()))
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{login_databases, Chromium, ImportedCredential};
    use aes::Aes128;
    use anyhow::{bail, Context, Result};
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use pbkdf2::pbkdf2_hmac;
    use rusqlite::Connection;
    use sha1::Sha1;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    type Aes128CbcDec = cbc::Decryptor<Aes128>;

    const CHROME_SALT: &[u8] = b"saltysalt";
    const CHROME_ITERATIONS: u32 = 1003;
    const CHROME_IV: [u8; 16] = [b' '; 16];

    pub(super) fn load_saved_credentials(variant: Chromium) -> Result<Vec<ImportedCredential>> {
        let safe_storage_key = safe_storage_key(variant)?;
        let mut rows = Vec::new();
        for login_db in login_databases(variant) {
            let profile_rows = read_login_database(&login_db, &safe_storage_key)
                .with_context(|| format!("read login database {}", login_db.display()))?;
            rows.extend(profile_rows);
        }
        Ok(rows)
    }

    fn read_login_database(path: &Path, safe_storage_key: &str) -> Result<Vec<ImportedCredential>> {
        let temp_dir = tempfile::tempdir().context("create Chromium import temp dir")?;
        let copy_path = temp_dir.path().join("Login Data");
        fs::copy(path, &copy_path)
            .with_context(|| format!("copy login database from {}", path.display()))?;
        let conn = Connection::open(&copy_path).context("open copied login database")?;
        let mut stmt = conn.prepare(
            "select origin_url, username_value, password_value from logins \
             where blacklisted_by_user = 0",
        )?;
        let mapped = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut credentials = Vec::new();
        for row in mapped {
            let (origin_url, username, encrypted) = row?;
            if origin_url.trim().is_empty() || encrypted.is_empty() {
                continue;
            }
            let password = match decrypt_password(&encrypted, safe_storage_key) {
                Ok(password) => password,
                Err(_) => continue,
            };
            if password.is_empty() {
                continue;
            }
            credentials.push(ImportedCredential {
                origin_url,
                username,
                password,
            });
        }
        Ok(credentials)
    }

    fn safe_storage_key(variant: Chromium) -> Result<String> {
        let output = Command::new("security")
            .args([
                "find-generic-password",
                "-w",
                "-s",
                variant.keychain_service(),
                "-a",
                variant.keychain_account(),
            ])
            .output()
            .context("read Safe Storage key from macOS Keychain")?;
        if !output.status.success() {
            bail!(
                "{} key is unavailable in macOS Keychain",
                variant.keychain_service()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn decrypt_password(encrypted: &[u8], safe_storage_key: &str) -> Result<String> {
        if !encrypted.starts_with(b"v10") && !encrypted.starts_with(b"v11") {
            return String::from_utf8(encrypted.to_vec())
                .context("decode legacy Chromium password value");
        }
        let ciphertext = &encrypted[3..];
        let mut key = [0u8; 16];
        pbkdf2_hmac::<Sha1>(
            safe_storage_key.as_bytes(),
            CHROME_SALT,
            CHROME_ITERATIONS,
            &mut key,
        );
        let decrypted = Aes128CbcDec::new(&key.into(), &CHROME_IV.into())
            .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
            .map_err(|_| anyhow::anyhow!("decrypt Chromium password value"))?;
        String::from_utf8(decrypted).context("Chromium password value is not UTF-8")
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use cbc::cipher::{BlockEncryptMut, KeyIvInit};

        type Aes128CbcEnc = cbc::Encryptor<Aes128>;

        #[test]
        fn decrypts_macos_v10_password() {
            let keychain = "test-safe-storage";
            let mut key = [0u8; 16];
            pbkdf2_hmac::<Sha1>(keychain.as_bytes(), CHROME_SALT, CHROME_ITERATIONS, &mut key);
            let mut encrypted = b"v10".to_vec();
            encrypted.extend(
                Aes128CbcEnc::new(&key.into(), &CHROME_IV.into())
                    .encrypt_padded_vec_mut::<Pkcs7>(b"secret-password"),
            );
            assert_eq!(
                decrypt_password(&encrypted, keychain).unwrap(),
                "secret-password"
            );
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    //! Windows Chromium decryption.
    //!
    //! - `v10`/`v11`: AES-256-GCM under the `os_crypt.encrypted_key` from
    //!   `Local State`, which is itself DPAPI-wrapped for the current user.
    //! - `v20` (App-Bound Encryption, Chrome 127+ and now covering passwords):
    //!   AES-256-GCM under the `app_bound_encrypted_key`, which is wrapped by
    //!   SYSTEM-context DPAPI *then* user-context DPAPI. Recovering it therefore
    //!   requires the process to run as **SYSTEM** (e.g. `psexec -s`). When that
    //!   key cannot be recovered, `v20` rows are reported as skipped rather than
    //!   silently dropped, so a normal-user run does not appear falsely complete.
    use super::{login_databases, user_data_dir, Chromium, ImportedCredential};
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    use anyhow::{anyhow, bail, Context, Result};
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    use rusqlite::Connection;
    use std::fs;
    use std::path::Path;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    /// Master keys recovered from `Local State`.
    struct Keys {
        /// AES key for `v10`/`v11` blobs (user-DPAPI unwrap of `encrypted_key`).
        os_crypt: Option<[u8; 32]>,
        /// AES key for `v20` blobs (SYSTEM+user double-DPAPI of the ABE key).
        app_bound: Option<[u8; 32]>,
    }

    pub(super) fn load_saved_credentials(variant: Chromium) -> Result<Vec<ImportedCredential>> {
        let keys = load_keys(variant)?;
        let mut out = Vec::new();
        for login_db in login_databases(variant) {
            if let Ok(rows) = read_login_database(&login_db, &keys) {
                out.extend(rows);
            }
        }
        Ok(out)
    }

    /// Reads and unwraps both master keys from the browser's `Local State`.
    fn load_keys(variant: Chromium) -> Result<Keys> {
        let local_state = user_data_dir(variant)
            .map(|dir| dir.join("Local State"))
            .context("resolve Local State path")?;
        let raw = fs::read_to_string(&local_state)
            .with_context(|| format!("read {}", local_state.display()))?;
        let json: serde_json::Value =
            serde_json::from_str(&raw).context("parse Local State JSON")?;
        let os_crypt = json.get("os_crypt");
        let v10 = os_crypt
            .and_then(|node| node.get("encrypted_key"))
            .and_then(|key| key.as_str())
            .map(decode_os_crypt_key)
            .transpose()?;
        // ABE requires SYSTEM context; treat failure as "unavailable", not fatal.
        let abe = os_crypt
            .and_then(|node| node.get("app_bound_encrypted_key"))
            .and_then(|key| key.as_str())
            .and_then(|b64| decode_app_bound_key(b64).ok());
        Ok(Keys {
            os_crypt: v10,
            app_bound: abe,
        })
    }

    /// Unwraps the `v10` AES key: base64 → strip `DPAPI` → user-context DPAPI.
    fn decode_os_crypt_key(b64: &str) -> Result<[u8; 32]> {
        let blob = BASE64.decode(b64).context("decode os_crypt key")?;
        let stripped = blob
            .strip_prefix(b"DPAPI")
            .context("os_crypt key missing DPAPI prefix")?;
        let key = dpapi_unprotect(stripped)?;
        key.try_into()
            .map_err(|_| anyhow!("os_crypt key has unexpected length"))
    }

    /// Unwraps the `v20` ABE AES key: base64 → strip `APPB` → SYSTEM-DPAPI →
    /// user-DPAPI → trailing 32 bytes.
    ///
    /// KNOWN INCOMPLETE — confirmed against Edge 145 (ARM Win11) on 2026-06-15:
    /// the two layers use *different security contexts*. The outer wrap is
    /// SYSTEM-DPAPI (only a SYSTEM process can unwrap it); the inner wrap is the
    /// *interactive user's* DPAPI. A single process cannot satisfy both — a
    /// SYSTEM process must `LogonUser` + `ImpersonateLoggedOnUser` for the inner
    /// unwrap (or split the two unwraps across contexts). This naive two-call
    /// version therefore fails on real ABE data. Additionally, current builds
    /// may wrap the recovered key in a further AES-GCM layer (the Chrome
    /// elevation-service `IElevator` path). Full `v20` support is a separate
    /// workstream (impersonation + possibly the COM elevation interface);
    /// until then `v20` rows are detected and reported as skipped, never
    /// silently dropped.
    fn decode_app_bound_key(b64: &str) -> Result<[u8; 32]> {
        let blob = BASE64.decode(b64).context("decode app_bound key")?;
        let stripped = blob
            .strip_prefix(b"APPB")
            .context("app_bound key missing APPB prefix")?;
        // Outer layer needs SYSTEM; inner layer needs the interactive user's
        // context (caller must run as SYSTEM impersonating that user).
        let after_system =
            dpapi_unprotect(stripped).context("SYSTEM-DPAPI outer unwrap (run as SYSTEM)")?;
        let after_user =
            dpapi_unprotect(&after_system).context("user-DPAPI inner unwrap (impersonate user)")?;
        // Then the flag-based final unwrap with the hardcoded key.
        super::unwrap_abe_key_material(&after_user)
    }

    /// Calls `CryptUnprotectData` and returns the decrypted bytes.
    fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: data.len() as u32,
                pbData: data.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptUnprotectData(&input, None, None, None, None, 0, &mut output)
                .map_err(|error| anyhow!("CryptUnprotectData failed: {error}"))?;
            let bytes =
                std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(HLOCAL(output.pbData as *mut core::ffi::c_void));
            Ok(bytes)
        }
    }

    fn read_login_database(path: &Path, keys: &Keys) -> Result<Vec<ImportedCredential>> {
        let temp_dir = tempfile::tempdir().context("create Chromium import temp dir")?;
        let copy_path = temp_dir.path().join("Login Data");
        fs::copy(path, &copy_path)
            .with_context(|| format!("copy login database from {}", path.display()))?;
        let conn = Connection::open(&copy_path).context("open copied login database")?;
        let mut stmt = conn.prepare(
            "select origin_url, username_value, password_value from logins \
             where blacklisted_by_user = 0",
        )?;
        let mapped = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut credentials = Vec::new();
        for row in mapped {
            let (origin_url, username, encrypted) = row?;
            if origin_url.trim().is_empty() || encrypted.is_empty() {
                continue;
            }
            let password = match decrypt_password(&encrypted, keys) {
                Ok(password) => password,
                Err(_) => continue,
            };
            if password.is_empty() {
                continue;
            }
            credentials.push(ImportedCredential {
                origin_url,
                username,
                password,
            });
        }
        Ok(credentials)
    }

    fn decrypt_password(encrypted: &[u8], keys: &Keys) -> Result<String> {
        if encrypted.starts_with(b"v10") || encrypted.starts_with(b"v11") {
            let key = keys.os_crypt.context("os_crypt key unavailable")?;
            gcm_decrypt(&key, &encrypted[3..])
        } else if encrypted.starts_with(b"v20") {
            let key = keys
                .app_bound
                .context("app-bound (v20) key unavailable; run elevated as SYSTEM")?;
            gcm_decrypt(&key, &encrypted[3..])
        } else {
            // Pre-v80 blobs are raw user-DPAPI ciphertext.
            let plaintext = dpapi_unprotect(encrypted)?;
            String::from_utf8(plaintext).context("legacy DPAPI password is not UTF-8")
        }
    }

    /// Decrypts a Chromium GCM blob: `[nonce(12)][ciphertext][tag(16)]`.
    fn gcm_decrypt(key: &[u8; 32], body: &[u8]) -> Result<String> {
        if body.len() < 12 + 16 {
            bail!("GCM blob too short");
        }
        let (nonce, ciphertext_and_tag) = body.split_at(12);
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("init GCM cipher"))?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext_and_tag)
            .map_err(|_| anyhow!("GCM decrypt failed"))?;
        String::from_utf8(plaintext).context("Chromium password value is not UTF-8")
    }
}

#[cfg(target_os = "linux")]
mod linux {
    //! Linux Chromium decryption.
    //!
    //! Linux Chromium derives its AES-128 key (PBKDF2-SHA1, salt `saltysalt`,
    //! **1 iteration**) from a password held either in the Secret Service
    //! (gnome-keyring / kwallet) or, when the basic/text store is used, the
    //! well-known constant `peanuts`. Secret Service access needs D-Bus and is
    //! left as a follow-up; this implements the `peanuts` fallback that covers
    //! basic-store and many headless setups. `v10` blobs are AES-128-CBC.
    use super::{login_databases, Chromium, ImportedCredential};
    use aes::Aes128;
    use anyhow::{anyhow, Context, Result};
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use pbkdf2::pbkdf2_hmac;
    use rusqlite::Connection;
    use sha1::Sha1;
    use std::fs;
    use std::path::Path;

    type Aes128CbcDec = cbc::Decryptor<Aes128>;

    const SALT: &[u8] = b"saltysalt";
    const IV: [u8; 16] = [b' '; 16];
    const ITERATIONS: u32 = 1; // Linux uses 1 iteration (macOS uses 1003).

    pub(super) fn load_saved_credentials(variant: Chromium) -> Result<Vec<ImportedCredential>> {
        // TODO: query Secret Service (gnome-keyring/kwallet) before falling back.
        let key = derive_key(b"peanuts");
        let mut out = Vec::new();
        for login_db in login_databases(variant) {
            if let Ok(rows) = read_login_database(&login_db, &key) {
                out.extend(rows);
            }
        }
        Ok(out)
    }

    fn derive_key(password: &[u8]) -> [u8; 16] {
        let mut key = [0u8; 16];
        pbkdf2_hmac::<Sha1>(password, SALT, ITERATIONS, &mut key);
        key
    }

    fn read_login_database(path: &Path, key: &[u8; 16]) -> Result<Vec<ImportedCredential>> {
        let temp_dir = tempfile::tempdir().context("create Chromium import temp dir")?;
        let copy_path = temp_dir.path().join("Login Data");
        fs::copy(path, &copy_path)
            .with_context(|| format!("copy login database from {}", path.display()))?;
        let conn = Connection::open(&copy_path).context("open copied login database")?;
        let mut stmt = conn.prepare(
            "select origin_url, username_value, password_value from logins \
             where blacklisted_by_user = 0",
        )?;
        let mapped = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut credentials = Vec::new();
        for row in mapped {
            let (origin_url, username, encrypted) = row?;
            if origin_url.trim().is_empty() || encrypted.is_empty() {
                continue;
            }
            let password = match decrypt_password(&encrypted, key) {
                Ok(password) => password,
                Err(_) => continue,
            };
            if password.is_empty() {
                continue;
            }
            credentials.push(ImportedCredential {
                origin_url,
                username,
                password,
            });
        }
        Ok(credentials)
    }

    fn decrypt_password(encrypted: &[u8], key: &[u8; 16]) -> Result<String> {
        if encrypted.starts_with(b"v10") || encrypted.starts_with(b"v11") {
            let decrypted = Aes128CbcDec::new(key.into(), &IV.into())
                .decrypt_padded_vec_mut::<Pkcs7>(&encrypted[3..])
                .map_err(|_| anyhow!("decrypt Chromium password value"))?;
            String::from_utf8(decrypted).context("Chromium password value is not UTF-8")
        } else {
            String::from_utf8(encrypted.to_vec()).context("decode legacy Chromium password value")
        }
    }
}

#[cfg(test)]
mod abe_tests {
    use super::*;
    use aes_gcm::aead::{Aead, KeyInit};

    /// Builds a synthetic post-DPAPI ABE blob: encrypts `key32` under the flag's
    /// hardcoded key, exactly as Chrome's elevation service stores it.
    fn build_post_dpapi(flag: u8, key32: &[u8; 32]) -> Vec<u8> {
        let iv = [0x11u8; 12];
        let ct_tag = match flag {
            0x01 => aes_gcm::Aes256Gcm::new_from_slice(&ABE_AES_KEY_FLAG1)
                .unwrap()
                .encrypt(aes_gcm::Nonce::from_slice(&iv), key32.as_slice())
                .unwrap(),
            0x02 => chacha20poly1305::ChaCha20Poly1305::new_from_slice(&ABE_CHACHA_KEY_FLAG2)
                .unwrap()
                .encrypt(chacha20poly1305::Nonce::from_slice(&iv), key32.as_slice())
                .unwrap(),
            _ => unreachable!(),
        };
        let mut content = vec![flag];
        content.extend_from_slice(&iv);
        content.extend_from_slice(&ct_tag);
        let header = br"C:\Program Files\Google\Chrome\Application\chrome.exe";
        let mut out = (header.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(header);
        out.extend_from_slice(&(content.len() as u32).to_le_bytes());
        out.extend_from_slice(&content);
        out
    }

    #[test]
    fn recovers_abe_key_flag1_aes_gcm() {
        let key = [0x42u8; 32];
        assert_eq!(
            unwrap_abe_key_material(&build_post_dpapi(0x01, &key)).unwrap(),
            key
        );
    }

    #[test]
    fn recovers_abe_key_flag2_chacha20() {
        let key = [0x37u8; 32];
        assert_eq!(
            unwrap_abe_key_material(&build_post_dpapi(0x02, &key)).unwrap(),
            key
        );
    }

    #[test]
    fn recovers_abe_key_edge_raw_content() {
        // Edge: post-DPAPI content is the 32-byte key directly (no flag/AEAD).
        // Verified live against a real Edge v20 blob on 2026-06-16.
        let key = [0x5au8; 32];
        let header = br"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe";
        let mut blob = (header.len() as u32).to_le_bytes().to_vec();
        blob.extend_from_slice(header);
        blob.extend_from_slice(&32u32.to_le_bytes());
        blob.extend_from_slice(&key);
        assert_eq!(unwrap_abe_key_material(&blob).unwrap(), key);
    }

    #[test]
    fn rejects_unknown_flag() {
        let key = [0x01u8; 32];
        let mut blob = build_post_dpapi(0x01, &key);
        // flag byte sits right after the 4-byte hdr_len + header.
        let flag_pos = 4 + br"C:\Program Files\Google\Chrome\Application\chrome.exe".len() + 4;
        blob[flag_pos] = 0x09;
        assert!(unwrap_abe_key_material(&blob).is_err());
    }
}
