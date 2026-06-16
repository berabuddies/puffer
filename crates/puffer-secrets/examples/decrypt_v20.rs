//! Windows dev tool: AES-256-GCM-decrypt the `v20` password blobs in a Chromium
//! Login Data using an already-recovered App-Bound Encryption key.
//!
//! The 32-byte ABE key is recovered separately (SYSTEM-DPAPI outer + user-DPAPI
//! inner, written to abe_key.bin) — this tool just does the final v20 decrypt,
//! proving the recovered key decrypts a REAL browser-encrypted password.
//!
//! Usage: decrypt_v20 <abe_key.bin> <Login Data path>

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    use anyhow::{anyhow, bail, Context};
    use rusqlite::Connection;

    let key_path = std::env::args().nth(1).context("arg1 = abe_key.bin")?;
    let ld_path = std::env::args().nth(2).context("arg2 = Login Data path")?;
    let key = std::fs::read(&key_path)?;
    if key.len() != 32 {
        bail!("ABE key must be 32 bytes, got {}", key.len());
    }
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("init GCM"))?;

    let tmp = std::env::temp_dir().join("ld_copy_v20");
    std::fs::copy(&ld_path, &tmp).with_context(|| format!("copy {ld_path}"))?;
    let conn = Connection::open(&tmp)?;
    let mut stmt =
        conn.prepare("select origin_url, username_value, password_value from logins")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    let mut any = false;
    for row in rows {
        let (origin, user, enc) = row?;
        any = true;
        if enc.starts_with(b"v20") {
            let body = &enc[3..];
            if body.len() < 12 + 16 {
                println!("V20_TOO_SHORT origin={origin}");
                continue;
            }
            let (nonce, ct_tag) = body.split_at(12);
            match cipher.decrypt(Nonce::from_slice(nonce), ct_tag) {
                Ok(pt) => println!(
                    "V20_DECRYPT_OK origin={origin} user={user} password={}",
                    String::from_utf8_lossy(&pt)
                ),
                Err(_) => println!("V20_DECRYPT_FAIL origin={origin} user={user}"),
            }
        } else {
            let p = String::from_utf8_lossy(&enc[..enc.len().min(3)]);
            println!("NON_V20 prefix={p} origin={origin}");
        }
    }
    if !any {
        println!("NO_LOGINS");
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("decrypt_v20 is Windows-only");
}
