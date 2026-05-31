use anyhow::Result;

/// Decrypted Chrome credential ready for import into the Puffer secret vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChromeCredential {
    pub(crate) origin_url: String,
    pub(crate) username: String,
    pub(crate) password: String,
}

#[cfg(target_os = "macos")]
/// Loads decryptable saved Chrome credentials from local macOS profiles.
pub(crate) fn load_saved_credentials() -> Result<Vec<ChromeCredential>> {
    macos::load_saved_credentials()
}

#[cfg(not(target_os = "macos"))]
/// Reports that Chrome credential import is unsupported on this platform.
pub(crate) fn load_saved_credentials() -> Result<Vec<ChromeCredential>> {
    anyhow::bail!("Chrome saved credential import is currently supported on macOS")
}

#[cfg(target_os = "macos")]
mod macos {
    use super::ChromeCredential;
    use aes::Aes128;
    use anyhow::{bail, Context, Result};
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use pbkdf2::pbkdf2_hmac;
    use rusqlite::Connection;
    use sha1::Sha1;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    type Aes128CbcDec = cbc::Decryptor<Aes128>;

    const CHROME_SAFE_STORAGE_SERVICE: &str = "Chrome Safe Storage";
    const CHROME_SAFE_STORAGE_ACCOUNT: &str = "Chrome";
    const CHROME_SALT: &[u8] = b"saltysalt";
    const CHROME_ITERATIONS: u32 = 1003;
    const CHROME_IV: [u8; 16] = [b' '; 16];

    pub(crate) fn load_saved_credentials() -> Result<Vec<ChromeCredential>> {
        let safe_storage_key = chrome_safe_storage_key()?;
        let mut rows = Vec::new();
        for login_db in chrome_login_databases()? {
            let profile_rows = read_login_database(&login_db, &safe_storage_key)
                .with_context(|| format!("read Chrome Login Data {}", login_db.display()))?;
            rows.extend(profile_rows);
        }
        Ok(rows)
    }

    fn chrome_login_databases() -> Result<Vec<PathBuf>> {
        let home = dirs::home_dir().context("resolve home directory")?;
        let root = home.join("Library/Application Support/Google/Chrome");
        let mut out = Vec::new();
        for profile in [
            "Default",
            "Profile 1",
            "Profile 2",
            "Profile 3",
            "Profile 4",
        ] {
            let path = root.join(profile).join("Login Data");
            if path.exists() {
                out.push(path);
            }
        }
        Ok(out)
    }

    fn read_login_database(path: &Path, safe_storage_key: &str) -> Result<Vec<ChromeCredential>> {
        let temp_dir = tempfile::tempdir().context("create Chrome import temp dir")?;
        let copy_path = temp_dir.path().join("Login Data");
        fs::copy(path, &copy_path).with_context(|| {
            format!(
                "copy Chrome Login Data from {} to {}",
                path.display(),
                copy_path.display()
            )
        })?;
        let conn = Connection::open(&copy_path).context("open copied Chrome Login Data")?;
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
            credentials.push(ChromeCredential {
                origin_url,
                username,
                password,
            });
        }
        Ok(credentials)
    }

    fn chrome_safe_storage_key() -> Result<String> {
        let output = Command::new("security")
            .args([
                "find-generic-password",
                "-w",
                "-s",
                CHROME_SAFE_STORAGE_SERVICE,
                "-a",
                CHROME_SAFE_STORAGE_ACCOUNT,
            ])
            .output()
            .context("read Chrome Safe Storage key from macOS Keychain")?;
        if !output.status.success() {
            bail!("Chrome Safe Storage key is unavailable in macOS Keychain");
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn decrypt_password(encrypted: &[u8], safe_storage_key: &str) -> Result<String> {
        if !encrypted.starts_with(b"v10") && !encrypted.starts_with(b"v11") {
            return String::from_utf8(encrypted.to_vec())
                .context("decode legacy Chrome password value");
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
            .map_err(|_| anyhow::anyhow!("decrypt Chrome password value"))?;
        String::from_utf8(decrypted).context("Chrome password value is not UTF-8")
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
            pbkdf2_hmac::<Sha1>(
                keychain.as_bytes(),
                CHROME_SALT,
                CHROME_ITERATIONS,
                &mut key,
            );
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
