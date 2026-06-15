//! Windows-only dev helper: seed a real `v10` credential into a Chromium
//! browser's Login Data using the browser's *real* DPAPI-protected os_crypt key.
//!
//! This exists purely to create a realistic test fixture on Windows so the
//! `sync_probe` decryptor can be validated end-to-end (real DPAPI key unwrap +
//! real v10 AES-256-GCM blob + real SQLite Login Data) without driving the
//! browser GUI. Run it in the guest, then run `sync_probe edge`.
//!
//! Usage: seed_win_login <edge|chrome>  (default edge)

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    use anyhow::{anyhow, bail, Context, Result};
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    use rusqlite::Connection;
    use std::path::PathBuf;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: data.len() as u32,
                pbData: data.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptUnprotectData(&input, None, None, None, None, 0, &mut output)
                .map_err(|e| anyhow!("CryptUnprotectData: {e}"))?;
            let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(HLOCAL(output.pbData as *mut core::ffi::c_void));
            Ok(out)
        }
    }

    let variant = std::env::args().nth(1).unwrap_or_else(|| "edge".into());
    let root_rel = match variant.as_str() {
        "edge" => "Microsoft/Edge/User Data",
        "chrome" => "Google/Chrome/User Data",
        other => bail!("unknown variant {other}"),
    };
    let local = PathBuf::from(std::env::var("LOCALAPPDATA").context("LOCALAPPDATA")?);
    let root = local.join(root_rel);
    let local_state = root.join("Local State");
    let login_db = root.join("Default").join("Login Data");

    // 1. Real os_crypt key: base64 -> strip "DPAPI" -> user DPAPI unwrap.
    let raw = std::fs::read_to_string(&local_state)
        .with_context(|| format!("read {}", local_state.display()))?;
    let json: serde_json::Value = serde_json::from_str(&raw)?;
    let b64 = json["os_crypt"]["encrypted_key"]
        .as_str()
        .context("os_crypt.encrypted_key missing")?;
    let blob = BASE64.decode(b64)?;
    let stripped = blob.strip_prefix(b"DPAPI").context("missing DPAPI prefix")?;
    let key = dpapi_unprotect(stripped)?;
    if key.len() != 32 {
        bail!("os_crypt key len {}", key.len());
    }

    // 2. Build a real v10 blob: "v10" + 12B nonce + ciphertext + 16B tag.
    let password = "PufferSecret42";
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("init gcm"))?;
    let nonce = [0x42u8; 12];
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), password.as_bytes())
        .map_err(|_| anyhow!("gcm encrypt"))?;
    let mut value = b"v10".to_vec();
    value.extend_from_slice(&nonce);
    value.extend_from_slice(&ct);

    // 3. Insert into Login Data.
    let conn = Connection::open(&login_db).with_context(|| format!("open {}", login_db.display()))?;
    conn.execute(
        "INSERT INTO logins \
         (origin_url, action_url, username_element, username_value, password_element, \
          password_value, submit_element, signon_realm, date_created, blacklisted_by_user, \
          scheme, password_type, times_used, form_data, date_last_used) \
         VALUES (?1, '', '', ?2, '', ?3, '', ?4, 13390000000000000, 0, 0, 0, 0, X'', 0)",
        rusqlite::params![
            "https://puffer.test/login",
            "testuser@puffer.local",
            value,
            "https://puffer.test/"
        ],
    )?;
    println!("seeded v10 credential into {} (key OK, {} byte blob)", login_db.display(), value.len());
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("seed_win_login is Windows-only");
}
