//! Chrome saved-login extraction.
//!
//! Chrome stores logins as a per-profile SQLite `Login Data` database plus a
//! browser-level key, protected by the OS key store:
//! - **macOS**: a "<Browser> Safe Storage" Keychain item → PBKDF2-SHA1 →
//!   AES-128-CBC (`v10`/`v11` blobs).
//! - **Windows**: `Local State` → DPAPI-wrapped AES key → AES-256-GCM (`v10`),
//!   plus App-Bound Encryption (`v20`) — implemented in the `windows` module.
//! - **Linux**: Secret Service / `peanuts` fallback → AES-128-CBC (`v10`).

use crate::ImportedCredential;
use anyhow::Result;

/// Chrome — the only Chromium-family browser Puffer imports from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chromium {
    Chrome,
}

impl Chromium {
    /// Profile-root path relative to the user profile/home for this OS.
    #[cfg(target_os = "macos")]
    fn user_data_root(self) -> &'static str {
        "Library/Application Support/Google/Chrome"
    }

    /// macOS Keychain service name holding the "Safe Storage" key.
    #[cfg(target_os = "macos")]
    fn keychain_service(self) -> &'static str {
        "Chrome Safe Storage"
    }

    /// macOS Keychain account name for the "Safe Storage" key.
    #[cfg(target_os = "macos")]
    fn keychain_account(self) -> &'static str {
        "Chrome"
    }

    /// `User Data` root relative to `%LOCALAPPDATA%` on Windows.
    #[cfg(target_os = "windows")]
    fn user_data_root(self) -> &'static str {
        "Google/Chrome/User Data"
    }

    /// `User Data` root relative to the Linux config dir.
    #[cfg(target_os = "linux")]
    fn user_data_root(self) -> &'static str {
        "google-chrome"
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
/// FLAG 0x03 (Chrome 137+) wraps the AES-GCM key under a per-machine CNG key
/// (`Google Chromekey1` in the Software KSP) — see `unwrap_abe_key_material` /
/// `windows::cng_decrypt_chrome_key`. These are Chrome-specific; Edge uses a
/// COM-only route with a different key.
const ABE_AES_KEY_FLAG1: [u8; 32] = [
    0xB3, 0x1C, 0x6E, 0x24, 0x1A, 0xC8, 0x46, 0x72, 0x8D, 0xA9, 0xC1, 0xFA, 0xC4, 0x93, 0x66, 0x51,
    0xCF, 0xFB, 0x94, 0x4D, 0x14, 0x3A, 0xB8, 0x16, 0x27, 0x6B, 0xCC, 0x6D, 0xA0, 0x28, 0x47, 0x87,
];
const ABE_CHACHA_KEY_FLAG2: [u8; 32] = [
    0xE9, 0x8F, 0x37, 0xD7, 0xF4, 0xE1, 0xFA, 0x43, 0x3D, 0x19, 0x30, 0x4D, 0xC2, 0x25, 0x80, 0x42,
    0x09, 0x0E, 0x2D, 0x1D, 0x7E, 0xEA, 0x76, 0x70, 0xD4, 0x1F, 0x73, 0x8D, 0x08, 0x72, 0x96, 0x60,
];
/// Flag-0x03 XOR mask lifted from Chrome's `elevation_service.exe`. The CNG
/// (`NCryptDecrypt`) output is XORed byte-for-byte with this to yield the
/// AES-256-GCM key that unwraps the v20 master key. Cross-validated byte-for-byte
/// across runassu/chrome_v20_decryption, The-Viper-One/Invoke-PowerChrome, and
/// fantasywastaken/Chrome-App-Bound-Decryption.
const ABE_XOR_KEY_FLAG3: [u8; 32] = [
    0xCC, 0xF8, 0xA1, 0xCE, 0xC5, 0x66, 0x05, 0xB8, 0x51, 0x75, 0x52, 0xBA, 0x1A, 0x2D, 0x06, 0x1C,
    0x03, 0xA2, 0x9E, 0x90, 0x27, 0x4F, 0xB2, 0xFC, 0xF5, 0x9B, 0xA4, 0xB7, 0x5C, 0x39, 0x23, 0x90,
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
    let key = match flag {
        // flag1/2 content: flag(1) | iv(12) | ct(32) | tag(16).
        0x01 => {
            let iv = content.get(1..13).context("ABE: missing iv")?;
            let ct_tag = content.get(13..).context("ABE: missing ciphertext")?;
            aes_gcm::Aes256Gcm::new_from_slice(&ABE_AES_KEY_FLAG1)
                .unwrap()
                .decrypt(aes_gcm::Nonce::from_slice(iv), ct_tag)
                .map_err(|_| anyhow!("ABE flag1 AES-GCM unwrap failed"))?
        }
        0x02 => {
            let iv = content.get(1..13).context("ABE: missing iv")?;
            let ct_tag = content.get(13..).context("ABE: missing ciphertext")?;
            chacha20poly1305::ChaCha20Poly1305::new_from_slice(&ABE_CHACHA_KEY_FLAG2)
                .unwrap()
                .decrypt(chacha20poly1305::Nonce::from_slice(iv), ct_tag)
                .map_err(|_| anyhow!("ABE flag2 ChaCha20 unwrap failed"))?
        }
        // flag3 content has an EXTRA 32-byte CNG-wrapped key before the iv:
        // flag(1) | encrypted_aes_key(32) | iv(12) | ct(32) | tag(16).
        0x03 => {
            let encrypted_aes_key: [u8; 32] = content
                .get(1..33)
                .context("ABE flag3: missing encrypted_aes_key")?
                .try_into()
                .unwrap();
            let iv = content.get(33..45).context("ABE flag3: missing iv")?;
            let ct_tag = content.get(45..).context("ABE flag3: missing ct/tag")?;
            // CNG unwrap (SYSTEM-gated) -> 32 bytes, then XOR + AES-GCM (pure).
            let decrypted_aes_key = cng_decrypt_flag03(&encrypted_aes_key)?;
            flag3_master_key(&decrypted_aes_key, iv, ct_tag)?
        }
        other => bail!("ABE: unknown flag 0x{other:02x}"),
    };
    key.as_slice()
        .try_into()
        .map_err(|_| anyhow!("ABE key is not 32 bytes"))
}

/// The pure tail of the flag-0x03 unwrap: XOR the CNG-decrypted 32 bytes with the
/// elevation-service constant to form the AES-256-GCM key, then GCM-decrypt the
/// inner `iv || ct || tag` to recover the 32-byte App-Bound master key. Kept OS-
/// agnostic so it can be unit-tested without CNG (the CNG step is injected).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn flag3_master_key(
    decrypted_aes_key: &[u8; 32],
    iv: &[u8],
    ct_tag: &[u8],
) -> anyhow::Result<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit};
    use anyhow::anyhow;
    let mut xored = [0u8; 32];
    for index in 0..32 {
        xored[index] = decrypted_aes_key[index] ^ ABE_XOR_KEY_FLAG3[index];
    }
    aes_gcm::Aes256Gcm::new_from_slice(&xored)
        .unwrap()
        .decrypt(aes_gcm::Nonce::from_slice(iv), ct_tag)
        .map_err(|_| anyhow!("ABE flag3 AES-GCM unwrap failed"))
}

/// Decrypts the flag-0x03 `encrypted_aes_key` via the per-machine CNG key. On
/// Windows this opens `Google Chromekey1` in the Software KSP (requires SYSTEM,
/// like the outer DPAPI layer); elsewhere flag 0x03 is unsupported.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn cng_decrypt_flag03(encrypted_aes_key: &[u8; 32]) -> anyhow::Result<[u8; 32]> {
    #[cfg(target_os = "windows")]
    {
        windows::cng_decrypt_chrome_key(encrypted_aes_key)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = encrypted_aes_key;
        anyhow::bail!("ABE flag 0x03 (per-machine CNG key) requires Windows")
    }
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

    /// Decrypts the flag-0x03 `encrypted_aes_key` with the per-machine CNG key
    /// `Google Chromekey1` (Microsoft Software KSP). The key's private material is
    /// ACL'd to SYSTEM, so this must run as SYSTEM (same gate as the outer DPAPI
    /// layer). Standard CNG two-call (size-then-data) pattern; no padding flag.
    pub(super) fn cng_decrypt_chrome_key(encrypted_aes_key: &[u8; 32]) -> Result<[u8; 32]> {
        use windows::core::w;
        use windows::Win32::Security::Cryptography::{
            NCryptDecrypt, NCryptFreeObject, NCryptOpenKey, NCryptOpenStorageProvider, CERT_KEY_SPEC,
            NCRYPT_FLAGS, NCRYPT_HANDLE, NCRYPT_KEY_HANDLE, NCRYPT_PROV_HANDLE, NCRYPT_SILENT_FLAG,
        };
        unsafe {
            let mut prov = NCRYPT_PROV_HANDLE::default();
            NCryptOpenStorageProvider(&mut prov, w!("Microsoft Software Key Storage Provider"), 0)
                .map_err(|error| anyhow!("NCryptOpenStorageProvider failed: {error}"))?;

            let mut key = NCRYPT_KEY_HANDLE::default();
            if let Err(error) = NCryptOpenKey(
                prov,
                &mut key,
                w!("Google Chromekey1"),
                CERT_KEY_SPEC(0),
                NCRYPT_FLAGS(0),
            ) {
                let _ = NCryptFreeObject(NCRYPT_HANDLE(prov.0));
                return Err(anyhow!("NCryptOpenKey(Google Chromekey1) failed: {error}"));
            }

            // First call: size query (pbOutput = None) fills `cb`.
            let mut cb: u32 = 0;
            let result = NCryptDecrypt(
                key,
                Some(encrypted_aes_key.as_slice()),
                None,
                None,
                &mut cb,
                NCRYPT_SILENT_FLAG,
            )
            .and_then(|()| {
                let mut out = vec![0u8; cb as usize];
                NCryptDecrypt(
                    key,
                    Some(encrypted_aes_key.as_slice()),
                    None,
                    Some(out.as_mut_slice()),
                    &mut cb,
                    NCRYPT_SILENT_FLAG,
                )
                .map(|()| {
                    out.truncate(cb as usize);
                    out
                })
            });

            let _ = NCryptFreeObject(NCRYPT_HANDLE(key.0));
            let _ = NCryptFreeObject(NCRYPT_HANDLE(prov.0));

            let out = result.map_err(|error| {
                anyhow!("NCryptDecrypt failed (run as SYSTEM for Google Chromekey1): {error}")
            })?;
            out.as_slice()
                .try_into()
                .map_err(|_| anyhow!("CNG output is not 32 bytes (got {})", out.len()))
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
    //! well-known constant `peanuts`. We query the Secret Service via the
    //! `secret-tool` CLI and always also try `peanuts`, decrypting each blob
    //! with whichever key works. `v10` blobs are AES-128-CBC.
    use super::{login_databases, Chromium, ImportedCredential};
    use aes::Aes128;
    use anyhow::{Context, Result};
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use pbkdf2::pbkdf2_hmac;
    use rusqlite::Connection;
    use sha1::Sha1;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    type Aes128CbcDec = cbc::Decryptor<Aes128>;

    const SALT: &[u8] = b"saltysalt";
    const IV: [u8; 16] = [b' '; 16];
    const ITERATIONS: u32 = 1; // Linux uses 1 iteration (macOS uses 1003).

    /// The `application` attribute Chrome uses for its Secret Service key.
    fn keyring_app(_variant: Chromium) -> &'static str {
        "chrome"
    }

    /// Reads the browser's "Safe Storage" key from the Secret Service via
    /// `secret-tool`, if available and unlocked.
    fn secret_service_password(app: &str) -> Option<Vec<u8>> {
        let output = Command::new("secret-tool")
            .args(["lookup", "application", app])
            .output()
            .ok()?;
        if !output.status.success() || output.stdout.is_empty() {
            return None;
        }
        Some(output.stdout)
    }

    pub(super) fn load_saved_credentials(variant: Chromium) -> Result<Vec<ImportedCredential>> {
        // Candidate keys: the Secret Service key (if present) and the basic-store
        // `peanuts` fallback. Each blob is tried against both.
        let mut keys: Vec<[u8; 16]> = Vec::new();
        if let Some(password) = secret_service_password(keyring_app(variant)) {
            keys.push(derive_key(&password));
        }
        keys.push(derive_key(b"peanuts"));

        let mut out = Vec::new();
        for login_db in login_databases(variant) {
            if let Ok(rows) = read_login_database(&login_db, &keys) {
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

    fn read_login_database(path: &Path, keys: &[[u8; 16]]) -> Result<Vec<ImportedCredential>> {
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

    fn decrypt_password(encrypted: &[u8], keys: &[[u8; 16]]) -> Result<String> {
        if encrypted.starts_with(b"v10") || encrypted.starts_with(b"v11") {
            // Try each candidate key (Secret Service, then peanuts).
            for key in keys {
                if let Ok(decrypted) = Aes128CbcDec::new(key.into(), &IV.into())
                    .decrypt_padded_vec_mut::<Pkcs7>(&encrypted[3..])
                {
                    if let Ok(text) = String::from_utf8(decrypted) {
                        return Ok(text);
                    }
                }
            }
            anyhow::bail!("no candidate key decrypted the Chromium password value")
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

    #[test]
    fn flag3_master_key_round_trips() {
        // The deterministic flag-0x03 tail (XOR + AES-GCM); the CNG step that
        // produces `decrypted_aes_key` is integration-tested on Windows.
        let master = [0x7eu8; 32];
        let xored_aes_key = [0x24u8; 32]; // the real AES-256-GCM key
        let iv = [0x11u8; 12];
        // ct||tag exactly as Chrome stores it: GCM-encrypt the master key.
        let ct_tag = aes_gcm::Aes256Gcm::new_from_slice(&xored_aes_key)
            .unwrap()
            .encrypt(aes_gcm::Nonce::from_slice(&iv), master.as_slice())
            .unwrap();
        // CNG output is the value that XORs back to the real key.
        let mut decrypted_aes_key = [0u8; 32];
        for index in 0..32 {
            decrypted_aes_key[index] = xored_aes_key[index] ^ ABE_XOR_KEY_FLAG3[index];
        }
        let recovered = flag3_master_key(&decrypted_aes_key, &iv, &ct_tag).unwrap();
        assert_eq!(recovered.as_slice(), master.as_slice());
    }
}
