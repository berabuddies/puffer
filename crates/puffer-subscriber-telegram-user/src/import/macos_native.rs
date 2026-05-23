//! Native macOS Telegram.app Postbox import support.
//!
//! The official macOS app does not use Telegram Desktop `tdata`. It stores
//! account records in a Postbox directory under the app group container and
//! encrypts account databases with SQLCipher. This module imports only the
//! MTProto auth keys needed to build a grammers session.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use aes::Aes256;
use anyhow::Context as _;
use cbc::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use grammers_client::session::Session;
use plist::Value;
use rusqlite::{params, Connection, OpenFlags};
use serde::Deserialize;
use sha2::{Digest, Sha512};

use super::{
    expand_home, persist_import_credentials_pair, save_imported_session, ImportSourceKind,
    TdataImportOptions, TdataImportOutcome,
};
use crate::state::SkillEnv;

const MACOS_API_ID: i32 = 9;
const MACOS_API_HASH: &str = "3975f648bb682ee889f35483bc618d1c";
const DEFAULT_APP_PASSCODE: &str = "no-matter-key";
const AUTH_INFO_KEY: &[u8] = b"persistent:datacenterAuthInfoById";
const PREFIXES: [&str; 4] = ["beta", "stable", "appstore", "debug"];
const DC_ADDRESSES: [(i32, &str, u16); 5] = [
    (1, "149.154.175.53", 443),
    (2, "149.154.167.51", 443),
    (3, "149.154.175.100", 443),
    (4, "149.154.167.92", 443),
    (5, "91.108.56.190", 443),
];

type Aes256CbcDec = cbc::Decryptor<Aes256>;

#[derive(Debug, Deserialize)]
struct AtomicState {
    #[serde(rename = "currentRecordId")]
    current_record_id: Option<String>,
    #[serde(default)]
    records: Vec<AtomicRecord>,
}

#[derive(Debug, Deserialize)]
struct AtomicRecord {
    id: String,
    #[serde(default)]
    attributes: Vec<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct NativePrefix {
    root: PathBuf,
    score: SystemTime,
}

#[derive(Debug, Clone)]
struct NativeAccount {
    prefix_root: PathBuf,
    account_dir: PathBuf,
    index: usize,
    count: usize,
    is_current: bool,
    modified: SystemTime,
}

/// Returns true when `import-desktop` should try native macOS Telegram.app
/// storage after Telegram Desktop `tdata` import fails.
pub(super) fn should_try_native_fallback(options: &TdataImportOptions) -> bool {
    if options.key_file.is_some() {
        return false;
    }
    match options.path.as_deref() {
        None => true,
        Some(path) => expand_home(path)
            .map(|path| looks_like_native_path(&path))
            .unwrap_or(false),
    }
}

/// Imports the selected native macOS Telegram.app account into the grammers
/// session file used by the subscriber.
pub(super) fn import_native(
    env: &SkillEnv,
    options: &TdataImportOptions,
) -> anyhow::Result<TdataImportOutcome> {
    let prefixes = discover_prefixes(options.path.as_deref())?;
    let mut errors = Vec::new();

    for prefix in prefixes {
        match import_from_prefix(env, options, &prefix.root) {
            Ok(outcome) => return Ok(outcome),
            Err(error) => errors.push(format!("{}: {error}", prefix.root.display())),
        }
    }

    if errors.is_empty() {
        anyhow::bail!("no native macOS Telegram.app account stores were found");
    }
    anyhow::bail!(
        "no native macOS Telegram.app account could be imported: {}",
        errors.join("; ")
    )
}

fn import_from_prefix(
    env: &SkillEnv,
    options: &TdataImportOptions,
    prefix_root: &Path,
) -> anyhow::Result<TdataImportOutcome> {
    let accounts = list_accounts(prefix_root)?;
    let selected = select_account(&accounts, options.account_index)?;
    let passcode = options.passcode.as_deref().unwrap_or(DEFAULT_APP_PASSCODE);
    let key = decrypt_storage_key(prefix_root, passcode)?;
    let auth_by_dc = read_auth_keys(&selected.account_dir, &key)
        .with_context(|| format!("read auth keys from {}", selected.account_dir.display()))?;
    if auth_by_dc.is_empty() {
        anyhow::bail!("selected account has no permanent datacenter auth keys");
    }

    let mut candidate_dc_ids = auth_by_dc.keys().copied().collect::<Vec<_>>();
    candidate_dc_ids.sort_unstable();
    let initial_dc = candidate_dc_ids[0];
    let session = build_session(&auth_by_dc, initial_dc)?;
    save_imported_session(&env.session_path, &session)?;
    persist_import_credentials_pair(env, MACOS_API_ID, MACOS_API_HASH)?;

    Ok(TdataImportOutcome {
        source_kind: ImportSourceKind::MacosNative,
        source_path: selected.prefix_root.clone(),
        account_index: selected.index,
        accounts_count: selected.count,
        user_id: None,
        dc_id: initial_dc,
        candidate_dc_ids,
        session_path: env.session_path.clone(),
    })
}

fn discover_prefixes(path: Option<&str>) -> anyhow::Result<Vec<NativePrefix>> {
    let roots = match path {
        Some(path) if !path.trim().is_empty() => {
            let path = expand_home(path.trim())?;
            if looks_like_prefix_root(&path) {
                vec![path]
            } else {
                native_prefixes_under(&path)
            }
        }
        _ => default_group_root()
            .map(|root| native_prefixes_under(&root))
            .unwrap_or_default(),
    };

    let mut prefixes = roots
        .into_iter()
        .filter(|root| looks_like_prefix_root(root))
        .map(|root| {
            let score = prefix_score(&root);
            NativePrefix { root, score }
        })
        .collect::<Vec<_>>();
    prefixes.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.root.cmp(&b.root)));
    prefixes.dedup_by(|a, b| a.root == b.root);
    Ok(prefixes)
}

fn native_prefixes_under(root: &Path) -> Vec<PathBuf> {
    PREFIXES
        .iter()
        .map(|prefix| root.join(prefix))
        .filter(|path| looks_like_prefix_root(path))
        .collect()
}

fn looks_like_native_path(path: &Path) -> bool {
    looks_like_prefix_root(path) || !native_prefixes_under(path).is_empty()
}

fn looks_like_prefix_root(path: &Path) -> bool {
    path.join(".tempkeyEncrypted").exists() && path.join("accounts-metadata/atomic-state").exists()
}

fn default_group_root() -> Option<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join("Library/Group Containers/6N38VWS5BX.ru.keepcoder.Telegram"))
}

fn prefix_score(prefix_root: &Path) -> SystemTime {
    list_accounts(prefix_root)
        .ok()
        .and_then(|accounts| accounts.into_iter().find(|account| account.is_current))
        .map(|account| account.modified)
        .or_else(|| modified_time(&prefix_root.join("accounts-metadata/atomic-state")))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn list_accounts(prefix_root: &Path) -> anyhow::Result<Vec<NativeAccount>> {
    let state_path = prefix_root.join("accounts-metadata/atomic-state");
    let raw = std::fs::read_to_string(&state_path)
        .with_context(|| format!("read {}", state_path.display()))?;
    let state: AtomicState =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", state_path.display()))?;
    let current_id = state
        .current_record_id
        .as_deref()
        .and_then(|id| id.parse::<i64>().ok());

    let mut records = state
        .records
        .iter()
        .filter_map(|record| {
            let id = record.id.parse::<i64>().ok()?;
            let order = sort_order(record).unwrap_or(i64::MAX);
            Some((id, order))
        })
        .collect::<Vec<_>>();
    records.sort_by_key(|(_, order)| *order);

    let count = records.len();
    let mut accounts = Vec::new();
    for (index, (id, _)) in records.into_iter().enumerate() {
        let account_dir = prefix_root.join(account_dir_name(id));
        let db_path = account_dir.join("postbox/db/db_sqlite");
        if !db_path.exists() {
            continue;
        }
        accounts.push(NativeAccount {
            prefix_root: prefix_root.to_path_buf(),
            account_dir,
            index,
            count,
            is_current: Some(id) == current_id,
            modified: modified_time(&db_path).unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }
    if accounts.is_empty() {
        anyhow::bail!("no account postbox databases are listed in atomic-state");
    }
    Ok(accounts)
}

fn sort_order(record: &AtomicRecord) -> Option<i64> {
    record
        .attributes
        .iter()
        .find_map(|attribute| attribute.get("sortOrder")?.get("order")?.as_i64())
}

fn select_account(
    accounts: &[NativeAccount],
    account_index: Option<usize>,
) -> anyhow::Result<NativeAccount> {
    if let Some(index) = account_index {
        return accounts
            .iter()
            .find(|account| account.index == index)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "native macOS Telegram.app account index {index} is unavailable; found {} account(s)",
                    accounts.first().map(|account| account.count).unwrap_or(0)
                )
            });
    }
    accounts
        .iter()
        .find(|account| account.is_current)
        .or_else(|| accounts.first())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no native macOS Telegram.app account is available"))
}

fn account_dir_name(id: i64) -> String {
    format!("account-{}", id as u64)
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn decrypt_storage_key(prefix_root: &Path, passcode: &str) -> anyhow::Result<Vec<u8>> {
    let encrypted_path = prefix_root.join(".tempkeyEncrypted");
    let encrypted = std::fs::read(&encrypted_path)
        .with_context(|| format!("read {}", encrypted_path.display()))?;
    if encrypted.len() % 16 != 0 {
        anyhow::bail!(
            "{} has invalid AES-CBC length {}",
            encrypted_path.display(),
            encrypted.len()
        );
    }

    let digest = Sha512::digest(passcode.as_bytes());
    let mut buffer = encrypted;
    let decrypted = Aes256CbcDec::new_from_slices(&digest[..32], &digest[48..64])
        .map_err(|error| anyhow::anyhow!("initialize Telegram.app key decryptor: {error}"))?
        .decrypt_padded_mut::<NoPadding>(&mut buffer)
        .map_err(|_| anyhow::anyhow!("decrypt Telegram.app storage key; passcode may be wrong"))?;
    if decrypted.len() < 48 {
        anyhow::bail!("decrypted Telegram.app storage key is too short");
    }
    Ok(decrypted[..48].to_vec())
}

fn read_auth_keys(
    account_dir: &Path,
    sqlcipher_key: &[u8],
) -> anyhow::Result<BTreeMap<i32, [u8; 256]>> {
    let db_path = account_dir.join("postbox/db/db_sqlite");
    let connection = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {}", db_path.display()))?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA cipher_plaintext_header_size=32;\
         PRAGMA cipher_default_plaintext_header_size=32;",
    )?;
    connection.execute_batch(&format!("PRAGMA key=\"x'{}'\";", hex_lower(sqlcipher_key)))?;
    connection.execute_batch("PRAGMA cipher_memory_security=OFF;")?;
    let value: Vec<u8> = connection
        .query_row(
            "SELECT value FROM t1 WHERE key=?1",
            params![AUTH_INFO_KEY],
            |row| row.get(0),
        )
        .context("read datacenter auth info from keychain table")?;
    decode_auth_info_archive(&value)
}

fn decode_auth_info_archive(data: &[u8]) -> anyhow::Result<BTreeMap<i32, [u8; 256]>> {
    let value = Value::from_reader(Cursor::new(data)).context("parse datacenter auth archive")?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| anyhow::anyhow!("datacenter auth archive root is not a dictionary"))?;
    let objects = dict
        .get("$objects")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("datacenter auth archive has no $objects array"))?;
    let top = dict
        .get("$top")
        .and_then(Value::as_dictionary)
        .ok_or_else(|| anyhow::anyhow!("datacenter auth archive has no $top dictionary"))?;
    let root = top
        .get("root")
        .and_then(|value| object_for_uid(objects, value))
        .and_then(Value::as_dictionary)
        .ok_or_else(|| anyhow::anyhow!("datacenter auth archive root object is invalid"))?;
    let keys = root
        .get("NS.keys")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("datacenter auth archive has no NS.keys"))?;
    let values = root
        .get("NS.objects")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("datacenter auth archive has no NS.objects"))?;

    let mut auth_by_dc = BTreeMap::new();
    for (key_ref, value_ref) in keys.iter().zip(values) {
        let Some(raw_key) = object_for_uid(objects, key_ref).and_then(plist_i64) else {
            continue;
        };
        if raw_key >> 32 != 0 {
            continue;
        }
        let dc_id = raw_key as i32;
        if dc_address(dc_id).is_none() {
            continue;
        }
        let Some(auth_info) = object_for_uid(objects, value_ref).and_then(Value::as_dictionary)
        else {
            continue;
        };
        let Some(auth_key_ref) = auth_info.get("authKey") else {
            continue;
        };
        let Some(auth_key) = object_for_uid(objects, auth_key_ref)
            .and_then(Value::as_dictionary)
            .and_then(|data| data.get("NS.data"))
            .and_then(Value::as_data)
        else {
            continue;
        };
        if auth_key.len() != 256 {
            continue;
        }
        let mut key = [0_u8; 256];
        key.copy_from_slice(auth_key);
        auth_by_dc.insert(dc_id, key);
    }
    Ok(auth_by_dc)
}

fn object_for_uid<'a>(objects: &'a [Value], value: &Value) -> Option<&'a Value> {
    let index = value.as_uid()?.get() as usize;
    objects.get(index)
}

fn plist_i64(value: &Value) -> Option<i64> {
    value.as_signed_integer().or_else(|| {
        value
            .as_unsigned_integer()
            .and_then(|value| i64::try_from(value).ok())
    })
}

fn build_session(
    auth_by_dc: &BTreeMap<i32, [u8; 256]>,
    initial_dc: i32,
) -> anyhow::Result<Session> {
    let session = Session::new();
    for (dc_id, auth_key) in auth_by_dc {
        let address = dc_address(*dc_id)
            .ok_or_else(|| anyhow::anyhow!("unsupported Telegram datacenter id {dc_id}"))?;
        session.insert_dc(*dc_id, address, *auth_key);
    }
    session.set_user(0, initial_dc, false);
    Ok(session)
}

fn dc_address(dc_id: i32) -> Option<SocketAddr> {
    DC_ADDRESSES
        .iter()
        .find(|(id, _, _)| *id == dc_id)
        .and_then(|(_, host, port)| format!("{host}:{port}").parse().ok())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use plist::{Dictionary, Uid};

    use super::*;

    #[test]
    fn account_dir_names_use_uint64_bit_pattern() {
        assert_eq!(
            account_dir_name(-1848512940229227965),
            "account-16598231133480323651"
        );
        assert_eq!(
            account_dir_name(-6992281810730024029),
            "account-11454462262979527587"
        );
    }

    #[test]
    fn auth_archive_decoder_keeps_permanent_dc_keys() {
        let permanent = vec![7_u8; 256];
        let temporary = vec![9_u8; 256];
        let archive = auth_archive_fixture(&permanent, &temporary);

        let decoded = decode_auth_info_archive(&archive).unwrap();

        assert_eq!(decoded.keys().copied().collect::<Vec<_>>(), vec![1]);
        assert_eq!(decoded.get(&1).unwrap().as_slice(), permanent.as_slice());
    }

    #[test]
    fn hex_encoding_is_lowercase() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xab, 0xff]), "000fabff");
    }

    fn auth_archive_fixture(permanent: &[u8], temporary: &[u8]) -> Vec<u8> {
        let objects = Value::Array(vec![
            Value::String("$null".to_string()),
            root_dict(),
            Value::Integer(1.into()),
            Value::Integer(4_294_967_297_i64.into()),
            auth_info(6),
            auth_info(7),
            data_object(permanent),
            data_object(temporary),
        ]);
        let mut top = Dictionary::new();
        top.insert("root".to_string(), Value::Uid(Uid::new(1)));
        let mut root = Dictionary::new();
        root.insert("$version".to_string(), Value::Integer(100000.into()));
        root.insert("$objects".to_string(), objects);
        root.insert("$top".to_string(), Value::Dictionary(top));

        let mut output = Vec::new();
        Value::Dictionary(root)
            .to_writer_binary(&mut output)
            .unwrap();
        output
    }

    fn root_dict() -> Value {
        let mut dict = Dictionary::new();
        dict.insert(
            "NS.keys".to_string(),
            Value::Array(vec![Value::Uid(Uid::new(2)), Value::Uid(Uid::new(3))]),
        );
        dict.insert(
            "NS.objects".to_string(),
            Value::Array(vec![Value::Uid(Uid::new(4)), Value::Uid(Uid::new(5))]),
        );
        Value::Dictionary(dict)
    }

    fn auth_info(auth_key_uid: u64) -> Value {
        let mut dict = Dictionary::new();
        dict.insert("authKey".to_string(), Value::Uid(Uid::new(auth_key_uid)));
        Value::Dictionary(dict)
    }

    fn data_object(data: &[u8]) -> Value {
        let mut dict = Dictionary::new();
        dict.insert("NS.data".to_string(), Value::Data(data.to_vec()));
        Value::Dictionary(dict)
    }
}
