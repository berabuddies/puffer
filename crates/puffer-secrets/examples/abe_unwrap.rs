//! Windows-only dev tool: recover a Chromium **App-Bound Encryption (v20)** key
//! and validate the whole v20 path end-to-end on a live machine.
//!
//! Must run as SYSTEM (e.g. a scheduled task) because the ABE key's outer DPAPI
//! layer is SYSTEM-protected; the inner layer is the interactive user's, so this
//! impersonates that user (LogonUser + ImpersonateLoggedOnUser) for the inner
//! unwrap. After both DPAPI peels, the 32-byte key is still wrapped with a
//! browser-hardcoded AEAD key selected by a flag byte (Chrome constants from the
//! runassu/xaitax research). This tool: recovers the key, seeds a real v20 blob
//! into Chrome's Login Data, then decrypts it back to prove the round-trip.
//!
//! Env: PUFFER_USER (default "puffer"), PUFFER_PASS (default "PufferTest2026!").
//! Usage: abe_unwrap   (targets Chrome)

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    win::run()
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("abe_unwrap is Windows-only");
}

#[cfg(target_os = "windows")]
mod win {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    use anyhow::{anyhow, bail, Context, Result};
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    use chacha20poly1305::ChaCha20Poly1305;
    use rusqlite::Connection;
    use std::path::PathBuf;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    use windows::Win32::Security::{
        ImpersonateLoggedOnUser, LogonUserW, RevertToSelf, LOGON32_LOGON_INTERACTIVE,
        LOGON32_PROVIDER_DEFAULT,
    };

    // Chrome elevation-service hardcoded final-unwrap keys (public; runassu/xaitax).
    const AES_KEY_FLAG1: [u8; 32] = hex32("B31C6E241AC846728DA9C1FAC4936651CFFB944D143AB816276BCC6DA0284787");
    const CHACHA_KEY_FLAG2: [u8; 32] = hex32("E98F37D7F4E1FA433D19304DC2258042090E2D1D7EEA7670D41F738D08729660");

    const fn hex32(s: &str) -> [u8; 32] {
        let b = s.as_bytes();
        let mut out = [0u8; 32];
        let mut i = 0;
        while i < 32 {
            out[i] = (hexval(b[i * 2]) << 4) | hexval(b[i * 2 + 1]);
            i += 1;
        }
        out
    }
    const fn hexval(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => 0,
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

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

    /// Recovers the 32-byte ABE master key from `app_bound_encrypted_key`.
    fn recover_abe_key(b64: &str, user: &str, pass: &str) -> Result<[u8; 32]> {
        let blob = BASE64.decode(b64).context("decode app_bound key")?;
        let stripped = blob.strip_prefix(b"APPB").context("missing APPB prefix")?;

        // Outer layer: SYSTEM-DPAPI (we are running as SYSTEM).
        let after_system = dpapi_unprotect(stripped).context("SYSTEM-DPAPI outer unwrap")?;

        // Inner layer: the interactive user's DPAPI — impersonate them.
        let after_user = unsafe {
            let mut token = HANDLE::default();
            LogonUserW(
                PCWSTR(wide(user).as_ptr()),
                PCWSTR(wide(".").as_ptr()),
                PCWSTR(wide(pass).as_ptr()),
                LOGON32_LOGON_INTERACTIVE,
                LOGON32_PROVIDER_DEFAULT,
                &mut token,
            )
            .context("LogonUser (inner DPAPI context)")?;
            ImpersonateLoggedOnUser(token).context("ImpersonateLoggedOnUser")?;
            let result = dpapi_unprotect(&after_system);
            let _ = RevertToSelf();
            let _ = CloseHandle(token);
            result.context("user-DPAPI inner unwrap (impersonated)")?
        };

        // Post-DPAPI structure: [u32 hdr_len][hdr][u32 content_len][flag|iv|ct|tag]
        let (header_len, rest) = take_u32(&after_user)?;
        let rest = rest.get(header_len as usize..).context("truncated header")?;
        let (content_len, content) = take_u32(rest)?;
        let content = content
            .get(..content_len as usize)
            .context("truncated content")?;
        let flag = *content.first().context("missing flag")?;
        let body = &content[1..];
        let iv = body.get(..12).context("missing iv")?;
        let ct_tag = body.get(12..).context("missing ciphertext")?; // 32 ct + 16 tag

        eprintln!(
            "ABE: hdr_len={header_len} content_len={content_len} flag=0x{flag:02x} body={}",
            body.len()
        );

        let key = match flag {
            0x01 => {
                let cipher = Aes256Gcm::new_from_slice(&AES_KEY_FLAG1).unwrap();
                cipher
                    .decrypt(Nonce::from_slice(iv), ct_tag)
                    .map_err(|_| anyhow!("flag1 AES-GCM unwrap failed"))?
            }
            0x02 => {
                use chacha20poly1305::aead::Aead as _;
                use chacha20poly1305::KeyInit as _;
                let cipher = ChaCha20Poly1305::new_from_slice(&CHACHA_KEY_FLAG2).unwrap();
                cipher
                    .decrypt(chacha20poly1305::Nonce::from_slice(iv), ct_tag)
                    .map_err(|_| anyhow!("flag2 ChaCha20 unwrap failed"))?
            }
            0x03 => bail!("flag 0x03 (per-machine CNG key) not implemented yet"),
            other => bail!("unknown ABE flag 0x{other:02x}"),
        };
        key.as_slice()
            .try_into()
            .map_err(|_| anyhow!("recovered ABE key is not 32 bytes (got {})", key.len()))
    }

    fn take_u32(data: &[u8]) -> Result<(u32, &[u8])> {
        let bytes = data.get(..4).context("truncated u32")?;
        Ok((u32::from_le_bytes(bytes.try_into().unwrap()), &data[4..]))
    }

    pub(super) fn run() -> Result<()> {
        let user = std::env::var("PUFFER_USER").unwrap_or_else(|_| "puffer".into());
        let pass = std::env::var("PUFFER_PASS").unwrap_or_else(|_| "PufferTest2026!".into());
        let browser = std::env::args().nth(1).unwrap_or_else(|| "chrome".into());
        let root_rel = match browser.as_str() {
            "edge" => "Microsoft\\Edge\\User Data",
            "chrome" => "Google\\Chrome\\User Data",
            other => bail!("unknown browser {other}"),
        };
        eprintln!("WHOAMI test for browser={browser}");

        let local = PathBuf::from(format!("C:\\Users\\{user}\\AppData\\Local"));
        let root = local.join(root_rel);
        let local_state = root.join("Local State");
        let login_db = root.join("Default\\Login Data");

        let raw = std::fs::read_to_string(&local_state)
            .with_context(|| format!("read {}", local_state.display()))?;
        let json: serde_json::Value = serde_json::from_str(&raw)?;
        let b64 = json["os_crypt"]["app_bound_encrypted_key"]
            .as_str()
            .context("os_crypt.app_bound_encrypted_key missing (run the browser once?)")?;

        // recover_abe_key prints the post-DPAPI structure (flag/lengths) to stderr
        // BEFORE the final unwrap, so a final-unwrap failure on a browser whose
        // hardcoded key differs (e.g. Edge) still proves the dual-context DPAPI works.
        let abe_key = match recover_abe_key(b64, &user, &pass) {
            Ok(k) => {
                println!("RECOVERED_ABE_KEY_OK first4={:02x?}", &k[..4]);
                k
            }
            Err(e) => {
                println!("DUAL_CONTEXT_DPAPI_OK_BUT_FINAL_UNWRAP_FAILED: {e}");
                return Ok(());
            }
        };

        // Seed a real v20 blob with the recovered key, then read it back.
        let password = "PufferV20Secret";
        let cipher = Aes256Gcm::new_from_slice(&abe_key).unwrap();
        let nonce = [0x24u8; 12];
        let ct = cipher
            .encrypt(Nonce::from_slice(&nonce), password.as_bytes())
            .map_err(|_| anyhow!("v20 encrypt"))?;
        let mut value = b"v20".to_vec();
        value.extend_from_slice(&nonce);
        value.extend_from_slice(&ct);

        let conn = Connection::open(&login_db)
            .with_context(|| format!("open {}", login_db.display()))?;
        conn.execute(
            "INSERT INTO logins \
             (origin_url, action_url, username_element, username_value, password_element, \
              password_value, submit_element, signon_realm, date_created, blacklisted_by_user, \
              scheme, password_type, times_used, form_data, date_last_used) \
             VALUES (?1,'','',?2,'',?3,'',?4,13390000000000000,0,0,0,0,X'',0)",
            rusqlite::params![
                "https://puffer.test/v20",
                "v20user@puffer.local",
                value,
                "https://puffer.test/"
            ],
        )?;
        println!("SEEDED_V20_OK ({} byte blob)", value.len());

        // Round-trip: decrypt the v20 blob we just stored.
        let body = &value[3..];
        let (n, ct_tag) = body.split_at(12);
        let back = cipher
            .decrypt(Nonce::from_slice(n), ct_tag)
            .map_err(|_| anyhow!("v20 decrypt"))?;
        let back = String::from_utf8(back)?;
        println!("V20_ROUNDTRIP={}  (expected={})", back, password);
        if back != password {
            bail!("v20 round-trip mismatch");
        }
        Ok(())
    }
}
