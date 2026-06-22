//! Firefox (and Gecko-family) saved-login extraction.
//!
//! Firefox keeps logins in two files inside each profile:
//! - `key4.db` — a SQLite NSS database holding the *encrypted* master key,
//!   plus the salts and a `password-check` sentinel used to validate the
//!   (optional) Primary Password.
//! - `logins.json` — the per-site `encryptedUsername` / `encryptedPassword`
//!   blobs, each a base64 DER structure encrypted with the master key. The
//!   per-field cipher is named by an OID: legacy profiles use 3DES-CBC (24-byte
//!   key), modern profiles use AES-256-CBC (32-byte key) — we branch on it.
//!
//! The recovery chain is: derive a PBE key from `globalSalt` (+ Primary
//! Password) → decrypt the master key in `nssPrivate.a11` → decrypt every
//! `logins.json` blob with the cipher its OID names. The master-key wrapping in
//! `key4.db` likewise uses one of two PBE schemes (legacy
//! `pbeWithSha1AndTripleDES-CBC`, or modern `PBES2` = AES-256-CBC +
//! PBKDF2-HMAC-SHA256), selected by an OID. This is a direct port of lclevy's
//! `firepwd.py`, the algorithm of record.
//!
//! Unlike the Chromium path this is entirely platform-independent (no Keychain /
//! DPAPI / Secret Service), so it covers macOS, Windows, and Linux uniformly —
//! only the profile-discovery paths differ per OS.

use crate::der::{DerReader, TAG_INTEGER, TAG_OCTET_STRING, TAG_OID, TAG_SEQUENCE};
use crate::ImportedCredential;
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use cbc::cipher::{
    block_padding::{NoPadding, Pkcs7},
    BlockDecryptMut, KeyIvInit,
};
use hmac::{Hmac, Mac};
use pbkdf2::pbkdf2_hmac;
use rusqlite::{Connection, OpenFlags};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::fs;
use std::path::{Path, PathBuf};

type Tdes3CbcDec = cbc::Decryptor<des::TdesEde3>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;
type HmacSha1 = Hmac<Sha1>;

/// `pbeWithSha1AndTripleDES-CBC` — OID 1.2.840.113549.1.12.5.1.3.
const OID_PBE_SHA1_3DES: &[u8] = &[
    0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x0c, 0x05, 0x01, 0x03,
];
/// `PBES2` — OID 1.2.840.113549.1.5.13.
const OID_PBES2: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x05, 0x0d];
/// `des-ede3-cbc` — OID 1.2.840.113549.3.7 (legacy `logins.json` field cipher).
const OID_DES_EDE3_CBC: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x03, 0x07];
/// `aes256-CBC` — OID 2.16.840.1.101.3.4.1.42 (modern `logins.json` field cipher).
const OID_AES256_CBC: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01, 0x2a];
/// Fixed NSS key id tying a `logins.json` blob to the master key in `nssPrivate`.
const CKA_ID: &[u8] = &[
    0xf8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
];
/// Sentinel proving the (empty or correct) Primary Password decrypted `item2`.
const PASSWORD_CHECK_PREFIX: &[u8] = b"password-check";

/// Loads decryptable saved Firefox credentials from every local profile.
pub(crate) fn load_saved_credentials() -> Result<Vec<ImportedCredential>> {
    let mut out = Vec::new();
    for profile in profile_dirs() {
        // A locked profile (Primary Password set) or a transient read error on
        // one profile must not abort the rest; skip it and continue.
        if let Ok(creds) = load_profile(&profile) {
            out.extend(creds);
        }
    }
    Ok(out)
}

/// Reports whether at least one Firefox profile with a key database exists.
pub(crate) fn is_available() -> bool {
    !profile_dirs().is_empty()
}

/// Resolves every local profile directory that contains a `key4.db`.
fn profile_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for base in base_dirs() {
        collect_profiles(&base, &mut out);
        collect_profiles(&base.join("Profiles"), &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Adds immediate child directories of `root` that contain a `key4.db`.
fn collect_profiles(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("key4.db").is_file() {
            out.push(path);
        }
    }
}

/// Returns the Gecko base directories (containing `profiles.ini` + `Profiles/`).
fn base_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        out.push(home.join("Library/Application Support/Firefox"));
    }
    #[cfg(target_os = "linux")]
    if let Some(home) = dirs::home_dir() {
        out.push(home.join(".mozilla/firefox"));
        out.push(home.join("snap/firefox/common/.mozilla/firefox"));
        out.push(home.join(".var/app/org.mozilla.firefox/.mozilla/firefox"));
    }
    #[cfg(target_os = "windows")]
    if let Some(appdata) = dirs::config_dir() {
        // dirs::config_dir() == %APPDATA% (Roaming) on Windows.
        out.push(appdata.join("Mozilla/Firefox"));
    }
    out
}

/// Decrypts every login in one profile, returning an empty set if the profile
/// is locked behind a Primary Password.
fn load_profile(dir: &Path) -> Result<Vec<ImportedCredential>> {
    let Some(master_key) = load_master_key(&dir.join("key4.db"))? else {
        return Ok(Vec::new());
    };
    let logins_path = dir.join("logins.json");
    let raw = match fs::read_to_string(&logins_path) {
        Ok(raw) => raw,
        Err(_) => return Ok(Vec::new()),
    };
    let json: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", logins_path.display()))?;
    let mut out = Vec::new();
    let entries = json
        .get("logins")
        .and_then(|logins| logins.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();
    for entry in entries {
        let origin = entry
            .get("hostname")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        // Skip Firefox-internal credentials (e.g. `chrome://FirefoxAccounts`,
        // the sync/account token) — `about:logins` hides these, so importing
        // them would surface a credential the user never saved as a site login.
        if origin.starts_with("chrome://") {
            continue;
        }
        let (Some(enc_user), Some(enc_pass)) = (
            entry.get("encryptedUsername").and_then(|v| v.as_str()),
            entry.get("encryptedPassword").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let password = match decode_login_field(enc_pass, &master_key) {
            Ok(password) => password,
            Err(_) => continue,
        };
        if password.is_empty() {
            continue;
        }
        let username = decode_login_field(enc_user, &master_key).unwrap_or_default();
        out.push(ImportedCredential {
            origin_url: origin,
            username,
            password,
        });
    }
    Ok(out)
}

/// Recovers the NSS master key (24-byte 3DES or 32-byte AES-256), or `None` if a
/// Primary Password is set.
fn load_master_key(key4_db: &Path) -> Result<Option<Vec<u8>>> {
    let key4 = read_key4(key4_db)?;
    let primary = b""; // We only attempt the common empty-Primary-Password case.
    let check = decrypt_key_blob(&key4.item2, &key4.global_salt, primary)?;
    if !check.starts_with(PASSWORD_CHECK_PREFIX) {
        // Wrong/empty password against a set Primary Password: cannot decrypt.
        return Ok(None);
    }
    if key4.cka_id != CKA_ID {
        bail!("Firefox key database has an unexpected NSS key id");
    }
    let decrypted = decrypt_key_blob(&key4.a11, &key4.global_salt, primary)?;
    // The key blob is PKCS#7-padded under PBES2/AES (32-byte key ⇒ 48 bytes) but
    // stored UNPADDED under legacy 3DES (exactly 24 bytes). Only accept the
    // stripped form when it is a valid key length — otherwise the "padding" was
    // really the tail of an unpadded 24-byte 3DES key (which can legitimately end
    // in pad-like bytes), so fall back to the raw plaintext. The recovered length
    // selects the login-field cipher: 24 ⇒ 3DES, 32 ⇒ AES-256.
    let stripped = pkcs7_unpad(&decrypted);
    let key: &[u8] = if matches!(stripped.len(), 24 | 32) {
        stripped
    } else {
        &decrypted
    };
    if !matches!(key.len(), 24 | 32) {
        bail!("Firefox master key has an unexpected length ({} bytes)", key.len());
    }
    Ok(Some(key.to_vec()))
}

/// Strips PKCS#7 padding when the trailing bytes form a valid pad (1..=16),
/// else returns the input unchanged (NSS stores legacy 3DES keys unpadded).
fn pkcs7_unpad(data: &[u8]) -> &[u8] {
    let Some(&last) = data.last() else {
        return data;
    };
    let pad = last as usize;
    if (1..=16).contains(&pad)
        && pad <= data.len()
        && data[data.len() - pad..].iter().all(|&b| b as usize == pad)
    {
        &data[..data.len() - pad]
    } else {
        data
    }
}

/// Raw key material read from `key4.db`.
struct Key4 {
    global_salt: Vec<u8>,
    item2: Vec<u8>,
    a11: Vec<u8>,
    cka_id: Vec<u8>,
}

/// Reads the salts, validation blob, and encrypted master key from `key4.db`.
fn read_key4(path: &Path) -> Result<Key4> {
    // Firefox keeps key4.db locked (WAL) while running; read a private copy.
    let temp = tempfile::tempdir().context("create Firefox key import temp dir")?;
    let copy = temp.path().join("key4.db");
    copy_with_sidecars(path, &copy)?;
    let conn = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open copied {}", path.display()))?;
    let (global_salt, item2) = conn
        .query_row(
            "select item1, item2 from metadata where id = 'password'",
            [],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .context("read Firefox key metadata")?;
    let (a11, cka_id) = conn
        .query_row(
            "select a11, a102 from nssPrivate where a11 is not null",
            [],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .context("read Firefox encrypted master key")?;
    Ok(Key4 {
        global_salt,
        item2,
        a11,
        cka_id,
    })
}

/// Copies `src` (and its `-wal` / `-shm` sidecars when present) to `dst`.
fn copy_with_sidecars(src: &Path, dst: &Path) -> Result<()> {
    fs::copy(src, dst).with_context(|| format!("copy {}", src.display()))?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = with_suffix(src, suffix);
        if sidecar.is_file() {
            let _ = fs::copy(&sidecar, with_suffix(dst, suffix));
        }
    }
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Decrypts a `key4.db` key blob (`item2` or `a11`) via whichever PBE scheme its
/// embedded OID names. Returns the raw (still-padded) plaintext.
fn decrypt_key_blob(blob: &[u8], global_salt: &[u8], primary: &[u8]) -> Result<Vec<u8>> {
    let mut top = DerReader::new(blob);
    let outer = top.expect(TAG_SEQUENCE)?;
    let mut outer = outer.reader();
    let algorithm = outer.expect(TAG_SEQUENCE)?;
    let ciphertext = outer.expect(TAG_OCTET_STRING)?.contents;
    let mut algorithm = algorithm.reader();
    let oid = algorithm.expect(TAG_OID)?.contents;
    if oid == OID_PBE_SHA1_3DES {
        let params = algorithm.expect(TAG_SEQUENCE)?;
        let mut params = params.reader();
        let entry_salt = params.expect(TAG_OCTET_STRING)?.contents;
        let _iteration = params.expect(TAG_INTEGER)?; // folded into the chain
        let (key, iv) = derive_moz_3des(global_salt, primary, entry_salt);
        decrypt_3des_cbc(&key, &iv, ciphertext)
    } else if oid == OID_PBES2 {
        let params = algorithm.expect(TAG_SEQUENCE)?;
        let mut params = params.reader();
        let kdf = params.expect(TAG_SEQUENCE)?;
        let scheme = params.expect(TAG_SEQUENCE)?;
        // keyDerivationFunc: SEQUENCE { OID pbkdf2, SEQUENCE { salt, iter, .. } }
        let mut kdf = kdf.reader();
        let _pbkdf2_oid = kdf.expect(TAG_OID)?;
        let kdf_params = kdf.expect(TAG_SEQUENCE)?;
        let mut kdf_params = kdf_params.reader();
        let entry_salt = kdf_params.expect(TAG_OCTET_STRING)?.contents;
        let iterations = kdf_params.expect(TAG_INTEGER)?.as_usize()? as u32;
        // `key4.db` is attacker-influenceable (a synced/dropped-in profile), and
        // the iteration count drives PBKDF2 directly — bound it so a crafted blob
        // cannot turn import into an unbounded-CPU hang. Real profiles use ~10k–650k.
        if !(1..=10_000_000).contains(&iterations) {
            bail!("Firefox PBES2 iteration count {iterations} is out of range");
        }
        // encryptionScheme: SEQUENCE { OID aes256-CBC, OCTET STRING iv }
        let mut scheme = scheme.reader();
        let _aes_oid = scheme.expect(TAG_OID)?;
        let iv = scheme.expect(TAG_OCTET_STRING)?.contents;
        let key = derive_pbes2_key(global_salt, primary, entry_salt, iterations);
        decrypt_pbes2_aes(&key, iv, ciphertext)
    } else {
        bail!("Firefox key blob uses an unsupported encryption algorithm")
    }
}

/// Decrypts one `logins.json` field (3DES-CBC under the master key, PKCS#7).
fn decode_login_field(b64: &str, master_key: &[u8]) -> Result<String> {
    let der = BASE64.decode(b64).context("decode Firefox login field")?;
    let mut top = DerReader::new(&der);
    let outer = top.expect(TAG_SEQUENCE)?;
    let mut outer = outer.reader();
    let _key_id = outer.expect(TAG_OCTET_STRING)?; // == CKA_ID
    let algorithm = outer.expect(TAG_SEQUENCE)?;
    let ciphertext = outer.expect(TAG_OCTET_STRING)?.contents;
    let mut algorithm = algorithm.reader();
    let oid = algorithm.expect(TAG_OID)?.contents;
    let iv = algorithm.expect(TAG_OCTET_STRING)?.contents;
    let plaintext = if oid == OID_AES256_CBC {
        // Modern Firefox: AES-256-CBC under the 32-byte master key, 16-byte IV.
        let key = master_key
            .get(..32)
            .context("Firefox AES master key too short")?;
        Aes256CbcDec::new_from_slices(key, iv)
            .map_err(|_| anyhow!("init Firefox AES login cipher"))?
            .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
            .map_err(|_| anyhow!("decrypt Firefox AES login field"))?
    } else if oid == OID_DES_EDE3_CBC {
        // Legacy Firefox: 3DES-CBC under the 24-byte master key, 8-byte IV.
        let key = master_key
            .get(..24)
            .context("Firefox 3DES master key too short")?;
        Tdes3CbcDec::new_from_slices(key, iv)
            .map_err(|_| anyhow!("init Firefox 3DES login cipher"))?
            .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
            .map_err(|_| anyhow!("decrypt Firefox 3DES login field"))?
    } else {
        bail!("Firefox login field uses an unsupported cipher OID");
    };
    String::from_utf8(plaintext).context("Firefox login field is not UTF-8")
}

/// NSS `decryptMoz3DES` key/IV derivation (legacy 3DES PBE).
fn derive_moz_3des(global_salt: &[u8], primary: &[u8], entry_salt: &[u8]) -> ([u8; 24], [u8; 8]) {
    let hp = sha1_concat(global_salt, primary);
    let chp = sha1_concat(&hp, entry_salt);
    let mut pes = entry_salt.to_vec();
    if pes.len() < 20 {
        pes.resize(20, 0);
    }
    let mut k1_msg = pes.clone();
    k1_msg.extend_from_slice(entry_salt);
    let k1 = hmac_sha1(&chp, &k1_msg);
    let tk = hmac_sha1(&chp, &pes);
    let mut k2_msg = tk;
    k2_msg.extend_from_slice(entry_salt);
    let k2 = hmac_sha1(&chp, &k2_msg);
    let mut k = k1;
    k.extend_from_slice(&k2); // 40 bytes
    let mut key = [0u8; 24];
    key.copy_from_slice(&k[..24]);
    let mut iv = [0u8; 8];
    iv.copy_from_slice(&k[k.len() - 8..]);
    (key, iv)
}

/// NSS `decryptPBES2` key derivation (PBKDF2-HMAC-SHA256 over `SHA1(salt||pw)`).
fn derive_pbes2_key(global_salt: &[u8], primary: &[u8], entry_salt: &[u8], iterations: u32) -> [u8; 32] {
    let pw_hash = sha1_concat(global_salt, primary);
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(&pw_hash, entry_salt, iterations, &mut key);
    key
}

fn decrypt_3des_cbc(key: &[u8], iv: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let cipher =
        Tdes3CbcDec::new_from_slices(key, iv).map_err(|_| anyhow!("init Firefox 3DES cipher"))?;
    cipher
        .decrypt_padded_vec_mut::<NoPadding>(ciphertext)
        .map_err(|_| anyhow!("decrypt Firefox 3DES blob"))
}

fn decrypt_pbes2_aes(key: &[u8], iv_struct: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    // Firefox frames the 14-byte stored IV as a DER OCTET STRING header
    // (0x04 0x0e) followed by the IV to form the 16-byte AES IV.
    let iv = match iv_struct.len() {
        14 => {
            let mut iv = vec![0x04, 0x0e];
            iv.extend_from_slice(iv_struct);
            iv
        }
        16 => iv_struct.to_vec(),
        other => bail!("Firefox PBES2 IV has unexpected length {other}"),
    };
    let cipher =
        Aes256CbcDec::new_from_slices(key, &iv).map_err(|_| anyhow!("init Firefox AES cipher"))?;
    cipher
        .decrypt_padded_vec_mut::<NoPadding>(ciphertext)
        .map_err(|_| anyhow!("decrypt Firefox AES blob"))
}

fn sha1_concat(a: &[u8], b: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(a);
    hasher.update(b);
    hasher.finalize().into()
}

fn hmac_sha1(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha1::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbc::cipher::block_padding::Pkcs7 as Pkcs7Enc;
    use cbc::cipher::{BlockEncryptMut, KeyIvInit};
    use rusqlite::params;

    type Tdes3CbcEnc = cbc::Encryptor<des::TdesEde3>;
    type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

    // --- tiny DER encoder for building fixtures ---
    fn tlv(tag: u8, contents: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        let len = contents.len();
        if len < 0x80 {
            out.push(len as u8);
        } else {
            let mut bytes = Vec::new();
            let mut n = len;
            while n > 0 {
                bytes.insert(0, (n & 0xff) as u8);
                n >>= 8;
            }
            out.push(0x80 | bytes.len() as u8);
            out.extend(bytes);
        }
        out.extend_from_slice(contents);
        out
    }
    fn seq(children: &[Vec<u8>]) -> Vec<u8> {
        let mut body = Vec::new();
        for child in children {
            body.extend_from_slice(child);
        }
        tlv(TAG_SEQUENCE, &body)
    }
    fn oid(bytes: &[u8]) -> Vec<u8> {
        tlv(TAG_OID, bytes)
    }
    fn octet(bytes: &[u8]) -> Vec<u8> {
        tlv(TAG_OCTET_STRING, bytes)
    }
    fn int(value: u32) -> Vec<u8> {
        let mut bytes = value.to_be_bytes().to_vec();
        while bytes.len() > 1 && bytes[0] == 0 {
            bytes.remove(0);
        }
        if bytes[0] & 0x80 != 0 {
            bytes.insert(0, 0);
        }
        tlv(TAG_INTEGER, &bytes)
    }

    const OID_DES_EDE3_CBC: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x03, 0x07];
    const OID_PBKDF2: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x05, 0x0c];
    const OID_AES256_CBC: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01, 0x2a];
    const OID_HMAC_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x02, 0x09];

    fn enc_3des(key: &[u8], iv: &[u8], plaintext: &[u8]) -> Vec<u8> {
        // plaintext is already a block multiple; NoPadding mirrors NSS storage.
        Tdes3CbcEnc::new_from_slices(key, iv)
            .unwrap()
            .encrypt_padded_vec_mut::<NoPadding>(plaintext)
    }
    fn enc_3des_pkcs7(key: &[u8], iv: &[u8], plaintext: &[u8]) -> Vec<u8> {
        Tdes3CbcEnc::new_from_slices(key, iv)
            .unwrap()
            .encrypt_padded_vec_mut::<Pkcs7Enc>(plaintext)
    }
    fn enc_aes(key: &[u8], iv: &[u8], plaintext: &[u8]) -> Vec<u8> {
        Aes256CbcEnc::new_from_slices(key, iv)
            .unwrap()
            .encrypt_padded_vec_mut::<Pkcs7Enc>(plaintext)
    }

    /// Legacy 3DES key-wrap blob for `item2` / `a11`.
    fn legacy_blob(global_salt: &[u8], entry_salt: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let (key, iv) = derive_moz_3des(global_salt, b"", entry_salt);
        let ciphertext = enc_3des(&key, &iv, plaintext);
        seq(&[
            seq(&[oid(OID_PBE_SHA1_3DES), seq(&[octet(entry_salt), int(1)])]),
            octet(&ciphertext),
        ])
    }

    /// Modern PBES2 (AES-256-CBC) key-wrap blob for `item2` / `a11`.
    fn pbes2_blob(global_salt: &[u8], entry_salt: &[u8], iv14: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let key = derive_pbes2_key(global_salt, b"", entry_salt, 10_000);
        let mut iv = vec![0x04, 0x0e];
        iv.extend_from_slice(iv14);
        let ciphertext = enc_aes(&key, &iv, plaintext);
        seq(&[
            seq(&[
                oid(OID_PBES2),
                seq(&[
                    seq(&[
                        oid(OID_PBKDF2),
                        seq(&[
                            octet(entry_salt),
                            int(10_000),
                            int(32),
                            seq(&[oid(OID_HMAC_SHA256)]),
                        ]),
                    ]),
                    seq(&[oid(OID_AES256_CBC), octet(iv14)]),
                ]),
            ]),
            octet(&ciphertext),
        ])
    }

    fn login_blob(master_key: &[u8], iv8: &[u8], plaintext: &[u8]) -> String {
        let ciphertext = enc_3des_pkcs7(master_key, iv8, plaintext);
        let der = seq(&[
            octet(CKA_ID),
            seq(&[oid(OID_DES_EDE3_CBC), octet(iv8)]),
            octet(&ciphertext),
        ]);
        BASE64.encode(der)
    }

    /// Modern AES-256-CBC `logins.json` field (32-byte key, 16-byte IV).
    fn aes_login_blob(master_key: &[u8], iv16: &[u8], plaintext: &[u8]) -> String {
        let ciphertext = enc_aes(master_key, iv16, plaintext);
        let der = seq(&[
            octet(CKA_ID),
            seq(&[oid(OID_AES256_CBC), octet(iv16)]),
            octet(&ciphertext),
        ]);
        BASE64.encode(der)
    }

    fn write_key4(dir: &Path, global_salt: &[u8], item2: &[u8], a11: &[u8]) {
        let conn = Connection::open(dir.join("key4.db")).unwrap();
        conn.execute(
            "create table metadata (id text primary key, item1 blob, item2 blob)",
            [],
        )
        .unwrap();
        conn.execute(
            "insert into metadata (id, item1, item2) values ('password', ?1, ?2)",
            params![global_salt, item2],
        )
        .unwrap();
        conn.execute("create table nssPrivate (a11 blob, a102 blob)", [])
            .unwrap();
        conn.execute(
            "insert into nssPrivate (a11, a102) values (?1, ?2)",
            params![a11, CKA_ID],
        )
        .unwrap();
    }

    fn write_logins(dir: &Path, host: &str, user_b64: &str, pass_b64: &str) {
        let json = serde_json::json!({
            "logins": [{
                "hostname": host,
                "encryptedUsername": user_b64,
                "encryptedPassword": pass_b64,
            }]
        });
        fs::write(dir.join("logins.json"), serde_json::to_string(&json).unwrap()).unwrap();
    }

    #[test]
    fn decrypts_legacy_3des_profile_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let global_salt = [0x11u8; 32];
        let entry_salt_item2 = [0x22u8; 20];
        let entry_salt_a11 = [0x33u8; 20];
        let master_key = [0x44u8; 24];

        let item2 = legacy_blob(&global_salt, &entry_salt_item2, b"password-check\x02\x02");
        let a11 = legacy_blob(&global_salt, &entry_salt_a11, &master_key);
        write_key4(dir.path(), &global_salt, &item2, &a11);

        let iv8 = [0x55u8; 8];
        let user_b64 = login_blob(&master_key, &iv8, b"alice@example.com");
        let pass_b64 = login_blob(&master_key, &iv8, b"hunter2");
        write_logins(dir.path(), "https://example.com", &user_b64, &pass_b64);

        let creds = load_profile(dir.path()).unwrap();
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].origin_url, "https://example.com");
        assert_eq!(creds[0].username, "alice@example.com");
        assert_eq!(creds[0].password, "hunter2");
    }

    #[test]
    fn legacy_3des_master_key_ending_in_pad_byte_is_not_truncated() {
        // Regression: the 24-byte legacy 3DES key is stored UNPADDED, so a key
        // whose last byte looks like PKCS#7 padding (here 0x01) must NOT be
        // pkcs7-stripped to 23 bytes (which would bail "too short" and silently
        // drop the whole profile).
        let dir = tempfile::tempdir().unwrap();
        let global_salt = [0x11u8; 32];
        let entry_salt_item2 = [0x22u8; 20];
        let entry_salt_a11 = [0x33u8; 20];
        let mut master_key = [0x12u8; 24];
        master_key[23] = 0x01; // pad-like trailing byte

        let item2 = legacy_blob(&global_salt, &entry_salt_item2, b"password-check\x02\x02");
        let a11 = legacy_blob(&global_salt, &entry_salt_a11, &master_key);
        write_key4(dir.path(), &global_salt, &item2, &a11);

        let iv8 = [0x55u8; 8];
        let user_b64 = login_blob(&master_key, &iv8, b"dave@example.com");
        let pass_b64 = login_blob(&master_key, &iv8, b"tr0ub4dour");
        write_logins(dir.path(), "https://legacy.example", &user_b64, &pass_b64);

        let creds = load_profile(dir.path()).unwrap();
        assert_eq!(creds.len(), 1, "pad-like-ending 3DES key must still decrypt");
        assert_eq!(creds[0].username, "dave@example.com");
        assert_eq!(creds[0].password, "tr0ub4dour");
    }

    #[test]
    fn decrypts_modern_pbes2_profile_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let global_salt = [0xA1u8; 32];
        let entry_salt_item2 = [0xB2u8; 32];
        let entry_salt_a11 = [0xC3u8; 32];
        let iv14_item2 = [0xD4u8; 14];
        let iv14_a11 = [0xE5u8; 14];
        let master_key = [0x46u8; 24];

        let item2 = pbes2_blob(
            &global_salt,
            &entry_salt_item2,
            &iv14_item2,
            b"password-check\x02\x02",
        );
        let a11 = pbes2_blob(&global_salt, &entry_salt_a11, &iv14_a11, &master_key);
        write_key4(dir.path(), &global_salt, &item2, &a11);

        let iv8 = [0x57u8; 8];
        let user_b64 = login_blob(&master_key, &iv8, b"bob@example.org");
        let pass_b64 = login_blob(&master_key, &iv8, b"correct horse");
        write_logins(dir.path(), "https://login.example.org", &user_b64, &pass_b64);

        let creds = load_profile(dir.path()).unwrap();
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].username, "bob@example.org");
        assert_eq!(creds[0].password, "correct horse");
    }

    #[test]
    fn decrypts_modern_aes256_logins_end_to_end() {
        // Real modern Firefox: PBES2 key blobs AND AES-256-CBC login fields with
        // a 32-byte master key (the case the live VM test exposed).
        let dir = tempfile::tempdir().unwrap();
        let global_salt = [0x9Au8; 32];
        let entry_salt_item2 = [0x8Bu8; 32];
        let entry_salt_a11 = [0x7Cu8; 32];
        let iv14_item2 = [0x6Du8; 14];
        let iv14_a11 = [0x5Eu8; 14];
        let master_key = [0x4Fu8; 32];

        let item2 = pbes2_blob(
            &global_salt,
            &entry_salt_item2,
            &iv14_item2,
            b"password-check\x02\x02",
        );
        let a11 = pbes2_blob(&global_salt, &entry_salt_a11, &iv14_a11, &master_key);
        write_key4(dir.path(), &global_salt, &item2, &a11);

        let iv16 = [0x3Au8; 16];
        let user_b64 = aes_login_blob(&master_key, &iv16, b"carol@example.net");
        let pass_b64 = aes_login_blob(&master_key, &iv16, b"battery staple");
        write_logins(dir.path(), "https://aes.example.net", &user_b64, &pass_b64);

        let creds = load_profile(dir.path()).unwrap();
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].username, "carol@example.net");
        assert_eq!(creds[0].password, "battery staple");
    }

    #[test]
    fn skips_profile_with_primary_password() {
        // item2 that does NOT decrypt to the sentinel under the empty password
        // simulates a set Primary Password: the profile is skipped, not errored.
        let dir = tempfile::tempdir().unwrap();
        let global_salt = [0x11u8; 32];
        let entry_salt = [0x22u8; 20];
        let master_key = [0x44u8; 24];
        let item2 = legacy_blob(&global_salt, &entry_salt, b"not-the-sentinel"); // 16 bytes
        let a11 = legacy_blob(&global_salt, &entry_salt, &master_key);
        write_key4(dir.path(), &global_salt, &item2, &a11);
        write_logins(dir.path(), "https://x.example", "AA==", "AA==");

        assert!(load_profile(dir.path()).unwrap().is_empty());
    }
}
