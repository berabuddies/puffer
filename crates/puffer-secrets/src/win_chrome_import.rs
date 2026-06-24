//! Windows-only: import saved Chrome passwords (v10/v11 + v20 incl. flag 0x03
//! CNG) into the Puffer SecretVault, behind a single user-consented UAC prompt.
//!
//! The v20 App-Bound key's outer layer is SYSTEM-DPAPI-protected and the CNG key
//! is ACL'd to SYSTEM, so extraction needs SYSTEM. To keep the daemon running as
//! the normal user and elevate ONLY for the import, this is a three-stage,
//! self-elevating flow driven through the `puffer __win-chrome-import` subcommand:
//!
//!   (user)     `Start-Process -Verb RunAs` -> UAC prompt -> wait
//!   --elevated create a TRANSIENT SYSTEM task -> run -> delete (no standing component)
//!   --system   do the import -> write a result file the parent stages read
//!
//! Config flows by ARG (env does not cross the elevation boundary); the user
//! stage passes its own live PID as the inner-DPAPI impersonation token source.

#![cfg(target_os = "windows")]

use crate::win_chrome_args::{
    normalize_vault_dir, parse_required_pid, ps_single_quote, validate_account_name,
    validate_vault_dir,
};
use crate::{SecretUpsert, SecretVault};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chacha20poly1305::ChaCha20Poly1305;
use rusqlite::Connection;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptUnprotectData, NCryptDecrypt, NCryptFreeObject, NCryptOpenKey, NCryptOpenStorageProvider,
    CERT_KEY_SPEC, CRYPT_INTEGER_BLOB, NCRYPT_FLAGS, NCRYPT_HANDLE, NCRYPT_KEY_HANDLE,
    NCRYPT_PROV_HANDLE, NCRYPT_SILENT_FLAG,
};
use windows::Win32::Security::{
    DuplicateTokenEx, ImpersonateLoggedOnUser, RevertToSelf, SecurityImpersonation,
    TokenImpersonation, TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// The subcommand name `puffer.exe` exposes for re-invocation across stages.
const SUBCOMMAND: &str = "__win-chrome-import";
/// Suppress console windows for the helper subprocesses (the import runs
/// silently; only the UAC consent dialog is shown).
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const AES_KEY_FLAG1: [u8; 32] =
    hex32("B31C6E241AC846728DA9C1FAC4936651CFFB944D143AB816276BCC6DA0284787");
const CHACHA_KEY_FLAG2: [u8; 32] =
    hex32("E98F37D7F4E1FA433D19304DC2258042090E2D1D7EEA7670D41F738D08729660");
const XOR_KEY_FLAG3: [u8; 32] =
    hex32("CCF8A1CEC56605B8517552BA1A2D061C03A29E90274FB2FCF59BA4B75C392390");

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

/// Run `f` while impersonating the interactive user's token (no password).
fn impersonating<T>(token_pid: u32, f: impl FnOnce() -> T) -> Result<T> {
    unsafe {
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, token_pid)
            .context("OpenProcess(token pid)")?;
        let mut ptok = HANDLE::default();
        OpenProcessToken(proc, TOKEN_DUPLICATE | TOKEN_QUERY, &mut ptok)
            .context("OpenProcessToken")?;
        let mut dup = HANDLE::default();
        DuplicateTokenEx(
            ptok,
            TOKEN_QUERY | TOKEN_IMPERSONATE | TOKEN_DUPLICATE,
            None,
            SecurityImpersonation,
            TokenImpersonation,
            &mut dup,
        )
        .context("DuplicateTokenEx")?;
        ImpersonateLoggedOnUser(dup).context("ImpersonateLoggedOnUser")?;
        let result = f();
        let _ = RevertToSelf();
        let _ = CloseHandle(dup);
        let _ = CloseHandle(ptok);
        let _ = CloseHandle(proc);
        Ok(result)
    }
}

fn cng_decrypt(encrypted_aes_key: &[u8; 32]) -> Result<[u8; 32]> {
    unsafe {
        let mut prov = NCRYPT_PROV_HANDLE::default();
        NCryptOpenStorageProvider(&mut prov, w!("Microsoft Software Key Storage Provider"), 0)
            .map_err(|e| anyhow!("NCryptOpenStorageProvider: {e}"))?;
        let mut key = NCRYPT_KEY_HANDLE::default();
        if let Err(e) = NCryptOpenKey(
            prov,
            &mut key,
            w!("Google Chromekey1"),
            CERT_KEY_SPEC(0),
            NCRYPT_FLAGS(0),
        ) {
            let _ = NCryptFreeObject(NCRYPT_HANDLE(prov.0));
            return Err(anyhow!("NCryptOpenKey: {e}"));
        }
        let mut cb = 0u32;
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
        let out = result.map_err(|e| anyhow!("NCryptDecrypt (run as SYSTEM): {e}"))?;
        out.as_slice()
            .try_into()
            .map_err(|_| anyhow!("CNG output not 32 bytes"))
    }
}

fn take_u32(data: &[u8]) -> Result<(u32, &[u8])> {
    let b = data.get(..4).context("truncated u32")?;
    Ok((u32::from_le_bytes(b.try_into().unwrap()), &data[4..]))
}

fn unwrap_abe(after_user: &[u8]) -> Result<[u8; 32]> {
    let (hdr_len, rest) = take_u32(after_user)?;
    let rest = rest.get(hdr_len as usize..).context("truncated header")?;
    let (content_len, content) = take_u32(rest)?;
    let content = content
        .get(..content_len as usize)
        .context("truncated content")?;
    if content_len == 32 {
        return content.try_into().map_err(|_| anyhow!("bad 32B content"));
    }
    let flag = *content.first().context("missing flag")?;
    let key = match flag {
        0x01 => {
            let iv = content.get(1..13).context("iv")?;
            let ct = content.get(13..).context("ct")?;
            Aes256Gcm::new_from_slice(&AES_KEY_FLAG1)
                .unwrap()
                .decrypt(Nonce::from_slice(iv), ct)
                .map_err(|_| anyhow!("flag1 unwrap"))?
        }
        0x02 => {
            use chacha20poly1305::aead::Aead as _;
            use chacha20poly1305::KeyInit as _;
            let iv = content.get(1..13).context("iv")?;
            let ct = content.get(13..).context("ct")?;
            ChaCha20Poly1305::new_from_slice(&CHACHA_KEY_FLAG2)
                .unwrap()
                .decrypt(chacha20poly1305::Nonce::from_slice(iv), ct)
                .map_err(|_| anyhow!("flag2 unwrap"))?
        }
        0x03 => {
            let enc: [u8; 32] = content.get(1..33).context("enc_key")?.try_into().unwrap();
            let iv = content.get(33..45).context("iv")?;
            let ct = content.get(45..).context("ct")?;
            let decrypted = cng_decrypt(&enc)?;
            let mut xored = [0u8; 32];
            for i in 0..32 {
                xored[i] = decrypted[i] ^ XOR_KEY_FLAG3[i];
            }
            Aes256Gcm::new_from_slice(&xored)
                .unwrap()
                .decrypt(Nonce::from_slice(iv), ct)
                .map_err(|_| anyhow!("flag3 GCM unwrap"))?
        }
        other => bail!("unknown flag 0x{other:02x}"),
    };
    key.as_slice().try_into().map_err(|_| anyhow!("not 32B"))
}

fn gcm_decrypt(key: &[u8; 32], value: &[u8]) -> Result<String> {
    // Layout: 3-byte version tag ("v10"/"v11"/"v20") + 12-byte GCM nonce + ct||tag.
    // Validate before slicing so a short/malformed row returns an error (counted and
    // skipped by the caller) instead of panicking and aborting the whole import.
    let body = value
        .get(3..)
        .context("encrypted value shorter than its version prefix")?;
    if body.len() < 12 {
        bail!("encrypted value too short for a 12-byte GCM nonce");
    }
    let (nonce, ct) = body.split_at(12);
    let pt = Aes256Gcm::new_from_slice(key)
        .unwrap()
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| anyhow!("v-blob GCM decrypt"))?;
    Ok(String::from_utf8_lossy(&pt).into_owned())
}

/// Refuses to proceed if the vault directory is itself a reparse point. The
/// per-file writes use `write_file_no_follow` (which only guards the LEAF
/// component), so a same-user process pre-planting `.puffer` as a junction could
/// otherwise redirect every SYSTEM-context vault write out of the profile. This
/// catches the persistent-junction case; it is not a complete TOCTOU fix (a racer
/// could plant after this check — a handle-relative write would fully close it).
/// A not-yet-created dir is allowed (normal first run); any other stat failure
/// fails closed rather than silently skipping the check.
fn ensure_vault_dir_not_reparse(vault_dir: &str) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    match std::fs::symlink_metadata(std::path::Path::new(vault_dir)) {
        Ok(meta) if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
            bail!("refusing to import: vault directory is a reparse point");
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => bail!("refusing to import: cannot stat vault directory: {e}"),
    }
}

/// The actual privileged import (SYSTEM context). Returns a human summary.
fn do_import(user: &str, token_pid: u32, vault_dir: &str) -> Result<String> {
    let root = PathBuf::from(format!(
        "C:\\Users\\{user}\\AppData\\Local\\Google\\Chrome\\User Data"
    ));
    let local_state = root.join("Local State");

    let raw = std::fs::read_to_string(&local_state)
        .with_context(|| format!("read {}", local_state.display()))?;
    let json: serde_json::Value = serde_json::from_str(&raw)?;
    let os_crypt = &json["os_crypt"];

    let abe_b64 = os_crypt["app_bound_encrypted_key"].as_str();
    let after_system = abe_b64
        .map(|b| -> Result<Vec<u8>> {
            let blob = BASE64.decode(b)?;
            let stripped = blob.strip_prefix(b"APPB").context("APPB")?;
            dpapi_unprotect(stripped).context("SYSTEM-DPAPI outer")
        })
        .transpose()?;

    let v10_b64 = os_crypt["encrypted_key"].as_str().map(|s| s.to_string());
    let (after_user, os_key) = impersonating(token_pid, || {
        let after_user = after_system.as_ref().map(|a| dpapi_unprotect(a)).transpose();
        let os_key = v10_b64
            .as_ref()
            .map(|b| -> Result<[u8; 32]> {
                let blob = BASE64.decode(b)?;
                let stripped = blob.strip_prefix(b"DPAPI").context("DPAPI prefix")?;
                dpapi_unprotect(stripped)?
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow!("os key not 32B"))
            })
            .transpose();
        (after_user, os_key)
    })?;

    let abe_key = match after_user {
        Ok(Some(au)) => unwrap_abe(&au).ok(),
        _ => None,
    };
    let os_key = os_key.ok().flatten();

    // Read BOTH the device store ("Login Data") and the signed-in account store
    // ("Login Data For Account") — passwords saved to the Google account live in
    // the latter. Same schema + same os_crypt/ABE key; union the rows.
    let mut rows: Vec<(String, String, Vec<u8>)> = Vec::new();
    for name in ["Default\\Login Data", "Default\\Login Data For Account"] {
        let db = root.join(name);
        if !db.exists() {
            continue;
        }
        // Copy the locked SQLite store into a fresh, random, auto-cleaned temp
        // directory (not a fixed shared-temp filename): the copy carries Chrome's
        // encrypted credential rows, so it must not sit at a predictable path another
        // process could read/squat, and it must be removed even on early return. The
        // `conn` is dropped before `tmpdir` (reverse declaration order), so the file
        // is closed before TempDir's Drop removes the directory.
        let Ok(tmpdir) = tempfile::tempdir() else {
            continue;
        };
        let tmp = tmpdir.path().join("logins.db");
        if std::fs::copy(&db, &tmp).is_err() {
            continue;
        }
        if let Ok(conn) = Connection::open(&tmp) {
            if let Ok(mut stmt) =
                conn.prepare("SELECT origin_url, username_value, password_value FROM logins")
            {
                if let Ok(mapped) = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))) {
                    rows.extend(mapped.filter_map(|r| r.ok()));
                }
            }
        }
    }

    // Defense-in-depth before this SYSTEM-context process writes inside the
    // user-writable vault dir: refuse if the dir is itself a reparse point.
    ensure_vault_dir_not_reparse(vault_dir)?;

    let path = SecretVault::default_path(std::path::Path::new(vault_dir));
    let vault = SecretVault::open(&path)?;
    let (mut imported, mut skipped, mut errors) = (0usize, 0usize, 0usize);
    for (origin, username, blob) in &rows {
        if blob.len() < 4 {
            skipped += 1;
            continue;
        }
        let plain = match &blob[..3] {
            b"v10" | b"v11" => os_key.as_ref().map(|k| gcm_decrypt(k, blob)),
            b"v20" => abe_key.as_ref().map(|k| gcm_decrypt(k, blob)),
            _ => None,
        };
        match plain {
            Some(Ok(pw)) if !pw.is_empty() => {
                match vault.put(SecretUpsert {
                    id: None,
                    label: format!("Chrome {username} @ {origin}"),
                    description: Some("imported from Chrome (Windows)".into()),
                    value: pw,
                    username: Some(username.clone()).filter(|u| !u.is_empty()),
                    origin: Some(origin.clone()),
                    source: "chrome".into(),
                }) {
                    Ok(_) => imported += 1,
                    Err(_) => errors += 1,
                }
            }
            _ => skipped += 1,
        }
    }
    Ok(format!(
        "imported={imported} skipped={skipped} errors={errors} total_rows={}",
        rows.len()
    ))
}

/// Per-user, per-run temp file the SYSTEM stage writes its (counts-only) result to
/// and the parent stages read up the internal elevation hops. The PID keeps
/// concurrent imports independent and the name non-fixed; the daemon does NOT trust
/// this file — it reads the helper's stdout (see daemon_secrets) — so this only
/// carries the result across the SYSTEM->elevated->user boundary that cannot pipe.
fn result_path(user: &str, pid: u32) -> PathBuf {
    PathBuf::from(format!(
        "C:\\Users\\{user}\\AppData\\Local\\Temp\\puffer_chrome_import_{pid}.txt"
    ))
}

/// Writes the SYSTEM-stage result line WITHOUT following a reparse point on the LEAF
/// path component, failing if anything already exists at the path. The result file
/// lives in the user's own (unprivileged-writable) Temp and this runs as SYSTEM, so
/// a concurrent same-user process could otherwise pre-plant a junction /
/// object-manager symlink at the predictable filename and redirect this privileged
/// write to an arbitrary target — the classic Temp TOCTOU local-privilege-escalation.
/// `create_new` fails if anything is already there and `FILE_FLAG_OPEN_REPARSE_POINT`
/// never traverses a reparse point at the final component, so a racer can at most
/// make the write fail (import reported as no-result), never redirect the leaf.
/// (Caveat: this flag only guards the final component — a junction on an
/// intermediate directory is not covered here; the standard Temp ancestors are not
/// realistically re-plantable, unlike the vault dir which do_import checks directly.)
fn write_result_no_follow(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    file.write_all(text.as_bytes())
}

/// Absolute path to a System32 tool, so the elevation path never resolves a
/// security-sensitive helper (schtasks / powershell) through PATH or the CWD.
/// The directory comes from the kernel via `GetSystemDirectoryW`, NOT from the
/// `SystemRoot`/`windir` environment (which a same-user caller of the hidden
/// subcommand could poison to redirect the helper to an attacker location).
fn system32(tool: &str) -> String {
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = [0u16; 260]; // MAX_PATH
    let len = unsafe { GetSystemDirectoryW(Some(&mut buf)) } as usize;
    let dir = if len > 0 && len <= buf.len() {
        String::from_utf16_lossy(&buf[..len])
    } else {
        "C:\\Windows\\System32".to_string()
    };
    format!("{dir}\\{tool}")
}

/// Entry point for the `puffer __win-chrome-import [args]` subcommand. `args` are
/// the tokens AFTER the subcommand name.
pub fn dispatch(args: &[String]) -> Result<()> {
    let exe = std::env::current_exe()?.to_string_lossy().to_string();
    match args.first().map(String::as_str) {
        // SYSTEM stage: do the import, write the result for the parent to read.
        Some("--system") => {
            let user = args.get(1).cloned().unwrap_or_default();
            let vault = normalize_vault_dir(&args.get(2).cloned().unwrap_or_default());
            let pid = parse_required_pid(args.get(3))?;
            // This stage builds SYSTEM-context filesystem paths from these args, so
            // validate them here too (the subcommand is directly invokable).
            validate_account_name(&user)?;
            validate_vault_dir(&vault)?;
            let result = do_import(&user, pid, &vault);
            let text = match &result {
                Ok(s) => format!("CHROME_IMPORT_OK {s}"),
                Err(e) => format!("CHROME_IMPORT_ERROR: {e:#}"),
            };
            let _ = write_result_no_follow(&result_path(&user, pid), &text);
            result.map(|_| ())
        }
        // ADMIN stage (post-UAC): import via a transient SYSTEM task.
        Some("--elevated") => {
            let user = args.get(1).cloned().unwrap_or_default();
            let vault = normalize_vault_dir(&args.get(2).cloned().unwrap_or_default());
            // Require a real PID (rejecting missing/invalid/0) so it can never carry
            // a metacharacter into the unquoted tail of the `/tr` action nor make the
            // task name / result file predictable; validate user/vault before building
            // the SYSTEM task command — don't trust the prior stage's argv (the hidden
            // subcommand is directly callable).
            let pid = parse_required_pid(args.get(3))?;
            validate_account_name(&user)?;
            validate_vault_dir(&vault)?;
            let tn = format!("PufferChromeImport_{pid}");
            // user/vault are double-quoted and, per validate_elevation_arg, cannot
            // contain `"` (the only breakout of a double-quoted token); pid is a
            // parsed integer; exe is our own absolute path. So no token can escape
            // the task action string.
            let tr = format!("\"{exe}\" {SUBCOMMAND} --system \"{user}\" \"{vault}\" {pid}");
            let out = result_path(&user, pid);
            let _ = std::fs::remove_file(&out);
            let schtasks = system32("schtasks.exe");
            Command::new(&schtasks)
                .args([
                    "/create", "/tn", &tn, "/tr", &tr, "/sc", "once", "/st", "00:00", "/ru",
                    "SYSTEM", "/f",
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .context("schtasks /create")?;
            Command::new(&schtasks)
                .args(["/run", "/tn", &tn])
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .context("schtasks /run")?;
            for _ in 0..120 {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if out.exists() {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            let _ = Command::new(&schtasks)
                .args(["/delete", "/tn", &tn, "/f"])
                .creation_flags(CREATE_NO_WINDOW)
                .status();
            println!(
                "{}",
                std::fs::read_to_string(&out).unwrap_or_else(|_| "no result produced".into())
            );
            Ok(())
        }
        // USER stage (default): ask Windows to elevate (UAC) and wait.
        _ => {
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "Administrator".into());
            let vault = normalize_vault_dir(
                &args
                    .iter()
                    .position(|a| a == "--vault-dir")
                    .and_then(|i| args.get(i + 1))
                    .cloned()
                    .or_else(|| std::env::var("PUFFER_VAULT_DIR").ok())
                    .unwrap_or_else(|| format!("C:\\Users\\{user}\\.puffer")),
            );
            validate_account_name(&user)?;
            validate_vault_dir(&vault)?;
            let pid = std::process::id();
            // Clear any stale result from a previous run BEFORE elevating: if the
            // user declines UAC (or PowerShell fails), the elevated stage never runs
            // and never rewrites this file, so a leftover "OK" must not be read as a
            // fresh success.
            let result = result_path(&user, pid);
            let _ = std::fs::remove_file(&result);
            // Each interpolated value goes inside a PowerShell single-quoted string
            // with embedded `'` doubled, so a username like `O'Brien` or a path with
            // an apostrophe is a literal argument and cannot inject PowerShell.
            let ps = format!(
                "Start-Process -Verb RunAs -WindowStyle Hidden -Wait -FilePath '{exe}' \
                 -ArgumentList '{SUBCOMMAND}','--elevated','{user}','{vault}','{pid}'",
                exe = ps_single_quote(&exe),
                user = ps_single_quote(&user),
                vault = ps_single_quote(&vault),
            );
            let powershell = system32("WindowsPowerShell\\v1.0\\powershell.exe");
            Command::new(&powershell)
                .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps])
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .context("launch elevated (UAC)")?;
            let summary = std::fs::read_to_string(&result)
                .unwrap_or_else(|_| "no result (was UAC approved?)".into());
            println!("{summary}");
            if summary.contains("CHROME_IMPORT_ERROR") {
                bail!("{summary}");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // #5: System32 is resolved from the kernel (GetSystemDirectoryW), so the path
    // is real and ends at the requested tool — not derived from %SystemRoot% env.
    #[test]
    fn system32_resolves_a_real_kernel_path() {
        let cmd = system32("cmd.exe");
        let lower = cmd.to_ascii_lowercase();
        assert!(lower.ends_with("\\system32\\cmd.exe"), "unexpected path: {cmd}");
        assert!(std::path::Path::new(&cmd).exists(), "cmd.exe should exist at {cmd}");
        // Poisoning %SystemRoot% must NOT change the kernel-resolved path.
        std::env::set_var("SystemRoot", "Z:\\attacker");
        assert_eq!(system32("cmd.exe"), cmd, "SystemRoot env must not influence system32()");
        std::env::remove_var("SystemRoot");
    }

    // #2: a junction planted as the vault dir is detected and refused; a normal dir
    // is allowed; a not-yet-created dir is allowed (first run).
    #[test]
    fn vault_dir_reparse_guard_detects_a_junction() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let normal = real.to_string_lossy().to_string();
        // A plain directory is accepted.
        assert!(ensure_vault_dir_not_reparse(&normal).is_ok());
        // A not-yet-created directory is accepted (SecretVault::open creates it).
        let missing = tmp.path().join("does-not-exist").to_string_lossy().to_string();
        assert!(ensure_vault_dir_not_reparse(&missing).is_ok());
        // Plant a junction (no admin needed) and confirm it is refused.
        let link = tmp.path().join("junction");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J", &link.to_string_lossy(), &real.to_string_lossy()])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .expect("run mklink");
        assert!(status.success(), "mklink /J should succeed without admin");
        let junction = link.to_string_lossy().to_string();
        let verdict = ensure_vault_dir_not_reparse(&junction);
        assert!(verdict.is_err(), "a junction vault dir must be refused, got {verdict:?}");
    }
}
