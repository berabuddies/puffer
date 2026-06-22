//! Encrypted local secret storage for Puffer.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

mod chromium;
mod der;
mod firefox;
mod keychain;
pub mod onepassword;
pub mod onepassword_1pux;
#[cfg(target_os = "windows")]
pub mod win_chrome_import;

pub use onepassword::{
    connect_onepassword, ensure_op_authorized, import_resolved_logins, is_op_reference,
    launch_onepassword_app, op_authorized, op_cli_available, resolve_op_reference, ResolvedLogin,
};

const STORE_VERSION: u32 = 1;
const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;

/// Non-secret metadata for one stored secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretSummary {
    /// Stable secret id used by tools and settings APIs.
    pub id: String,
    /// User-facing label for the secret.
    pub label: String,
    /// Optional non-secret description for display and search.
    pub description: Option<String>,
    /// Optional login username associated with imported credentials.
    pub username: Option<String>,
    /// Optional origin URL or domain associated with the secret.
    pub origin: Option<String>,
    /// Source label such as `manual` or `chrome`.
    pub source: String,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at_ms: u64,
    /// Last update time in milliseconds since the Unix epoch.
    pub updated_at_ms: u64,
}

/// Input for creating or updating one encrypted secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretUpsert {
    /// Optional existing secret id to update.
    pub id: Option<String>,
    /// User-facing label for the secret.
    pub label: String,
    /// Optional non-secret description for display and search.
    pub description: Option<String>,
    /// Raw secret value to encrypt.
    pub value: String,
    /// Optional login username associated with the secret.
    pub username: Option<String>,
    /// Optional origin URL or domain associated with the secret.
    pub origin: Option<String>,
    /// Source label such as `manual` or `chrome`.
    pub source: String,
}

/// Decrypted secret material resolved for runtime use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSecret {
    /// Stable secret id from the encrypted store.
    pub id: String,
    /// User-facing label for the secret.
    pub label: String,
    /// Optional non-secret description for display and search.
    pub description: Option<String>,
    /// Optional login username associated with the secret.
    pub username: Option<String>,
    /// Optional origin URL or domain associated with the secret.
    pub origin: Option<String>,
    /// Decrypted secret value.
    pub value: String,
}

/// Summary of a browser credential import pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Number of credentials written to the encrypted Puffer store.
    pub imported: usize,
    /// Number of source rows skipped because they were unusable or duplicates.
    pub skipped: usize,
    /// Non-secret import diagnostics.
    pub errors: Vec<String>,
}

/// Back-compat alias for the original Chrome-only report type.
pub type ChromeImportReport = ImportReport;

/// One credential decrypted from an external browser store, ready for import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportedCredential {
    pub(crate) origin_url: String,
    pub(crate) username: String,
    pub(crate) password: String,
}

/// A browser credential store Puffer can import saved logins from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserSource {
    Chrome,
    Firefox,
}

impl BrowserSource {
    /// Stable source id stored on each secret and used by the RPC/UI.
    pub fn id(self) -> &'static str {
        match self {
            BrowserSource::Chrome => "chrome",
            BrowserSource::Firefox => "firefox",
        }
    }

    /// User-facing source name.
    pub fn label(self) -> &'static str {
        match self {
            BrowserSource::Chrome => "Chrome",
            BrowserSource::Firefox => "Firefox",
        }
    }

    /// All known sources, in display order.
    pub fn all() -> [BrowserSource; 2] {
        [BrowserSource::Chrome, BrowserSource::Firefox]
    }

    /// Resolves a source from its stable id.
    pub fn from_id(id: &str) -> Option<BrowserSource> {
        BrowserSource::all()
            .into_iter()
            .find(|source| source.id() == id)
    }

    fn load_credentials(self) -> Result<Vec<ImportedCredential>> {
        match self {
            BrowserSource::Chrome => chromium::load_saved_credentials(chromium::Chromium::Chrome),
            BrowserSource::Firefox => firefox::load_saved_credentials(),
        }
    }

    fn is_available(self) -> bool {
        match self {
            BrowserSource::Chrome => chromium::is_available(chromium::Chromium::Chrome),
            BrowserSource::Firefox => firefox::is_available(),
        }
    }
}

/// Availability of one browser source for display in settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAvailability {
    /// Stable source id (e.g. `chrome`, `firefox`).
    pub id: String,
    /// User-facing source name.
    pub label: String,
    /// Whether at least one importable profile currently exists.
    pub available: bool,
}

/// Lists every import source (browsers + 1Password) and its availability.
pub fn available_browser_sources() -> Vec<SourceAvailability> {
    let mut sources: Vec<SourceAvailability> = BrowserSource::all()
        .into_iter()
        .map(|source| SourceAvailability {
            id: source.id().to_string(),
            label: source.label().to_string(),
            available: source.is_available(),
        })
        .collect();
    // 1Password is not a browser source. Offer it whenever the `op` CLI is
    // installed OR the 1Password desktop app is present — in the app-but-no-CLI
    // case clicking "Sync from 1Password" auto-installs the CLI, so the button
    // must NOT be disabled there.
    sources.push(SourceAvailability {
        id: "1password".to_string(),
        label: "1Password".to_string(),
        available: onepassword::op_cli_available() || onepassword::onepassword_app_installed(),
    });
    sources
}

/// Encrypted JSON-backed secret vault.
#[derive(Debug, Clone)]
pub struct SecretVault {
    path: PathBuf,
    key: [u8; KEY_BYTES],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecretStoreFile {
    version: u32,
    secrets: Vec<SecretRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecretRecord {
    id: String,
    label: String,
    #[serde(default)]
    description: Option<String>,
    username: Option<String>,
    origin: Option<String>,
    source: String,
    created_at_ms: u64,
    updated_at_ms: u64,
    encrypted: EncryptedValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EncryptedValue {
    algorithm: String,
    nonce: String,
    ciphertext: String,
}

impl SecretVault {
    /// Opens a production vault using the platform key source for `path`.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let key = keychain::load_or_create_key(&path)?;
        Ok(Self { path, key })
    }

    /// Opens a vault with an explicit key, primarily for tests.
    pub fn open_with_key(path: impl Into<PathBuf>, key: [u8; KEY_BYTES]) -> Self {
        Self {
            path: path.into(),
            key,
        }
    }

    /// Returns the default encrypted secret-store path under a Puffer config directory.
    pub fn default_path(user_config_dir: &Path) -> PathBuf {
        user_config_dir.join("secrets.json")
    }

    /// Lists stored secret metadata from a store file without loading an encryption key.
    pub fn list_metadata(path: impl AsRef<Path>) -> Result<Vec<SecretSummary>> {
        Ok(load_store_file(path.as_ref())?
            .secrets
            .iter()
            .map(secret_summary)
            .collect())
    }

    /// Lists all stored secrets without decrypting secret material.
    pub fn list(&self) -> Result<Vec<SecretSummary>> {
        Ok(self
            .load_store()?
            .secrets
            .iter()
            .map(secret_summary)
            .collect())
    }

    /// Searches stored secret metadata without decrypting secret material.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SecretSummary>> {
        let terms = secret_search_terms(query);
        let limit = limit.max(1);
        let mut matches = self
            .load_store()?
            .secrets
            .into_iter()
            .filter_map(|record| {
                let score = secret_search_score(&record, &terms);
                (terms.is_empty() || score > 0).then(|| (score, secret_summary(&record)))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.updated_at_ms.cmp(&left.1.updated_at_ms))
                .then_with(|| left.1.label.cmp(&right.1.label))
        });
        let mut matches = matches
            .into_iter()
            .map(|(_, summary)| summary)
            .collect::<Vec<_>>();
        matches.truncate(limit);
        Ok(matches)
    }

    /// Creates or updates one encrypted secret and returns its public summary.
    pub fn put(&self, input: SecretUpsert) -> Result<SecretSummary> {
        let label = normalized_required("secret label", &input.label)?;
        let value = input.value;
        if value.is_empty() {
            bail!("secret value cannot be empty");
        }
        let source = normalized_source(&input.source);
        let description = non_empty_option(input.description);
        let username = non_empty_option(input.username);
        let origin = non_empty_option(input.origin);
        let now = now_ms();
        let encrypted = self.encrypt(&value)?;
        let mut store = self.load_store()?;
        let existing_index = input
            .id
            .as_deref()
            .and_then(|id| store.secrets.iter().position(|record| record.id == id))
            .or_else(|| {
                store.secrets.iter().position(|record| {
                    record.label == label
                        && record.username == username
                        && record.origin == origin
                        && record.source == source
                })
            });
        let index = if let Some(index) = existing_index {
            store.secrets[index].label = label;
            store.secrets[index].description = description;
            store.secrets[index].username = username;
            store.secrets[index].origin = origin;
            store.secrets[index].source = source;
            store.secrets[index].updated_at_ms = now;
            store.secrets[index].encrypted = encrypted;
            index
        } else {
            store.secrets.push(SecretRecord {
                id: format!("sec_{}", Uuid::new_v4().simple()),
                label,
                description,
                username,
                origin,
                source,
                created_at_ms: now,
                updated_at_ms: now,
                encrypted,
            });
            store.secrets.len() - 1
        };
        let summary = secret_summary(&store.secrets[index]);
        self.save_store(&store)?;
        Ok(summary)
    }
    /// Deletes one secret by id, returning whether a record was removed.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let id = id.trim();
        if id.is_empty() {
            bail!("secret id cannot be empty");
        }
        let mut store = self.load_store()?;
        let before = store.secrets.len();
        store.secrets.retain(|record| record.id != id);
        let removed = store.secrets.len() != before;
        if removed {
            self.save_store(&store)?;
        }
        Ok(removed)
    }

    /// Reveals one secret by exact id or unique label.
    pub fn reveal(&self, id_or_label: &str) -> Result<ResolvedSecret> {
        let selector = normalized_required("secret selector", id_or_label)?;
        let store = self.load_store()?;
        let exact = store
            .secrets
            .iter()
            .position(|record| record.id == selector);
        let record = if let Some(index) = exact {
            &store.secrets[index]
        } else {
            let matches = store
                .secrets
                .iter()
                .enumerate()
                .filter_map(|(index, record)| (record.label == selector).then_some(index))
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [index] => &store.secrets[*index],
                [] => bail!("secret `{selector}` was not found"),
                _ => bail!("secret label `{selector}` is ambiguous; use the secret id"),
            }
        };
        Ok(ResolvedSecret {
            id: record.id.clone(),
            label: record.label.clone(),
            description: record.description.clone(),
            username: record.username.clone(),
            origin: record.origin.clone(),
            value: self.decrypt(&record.encrypted)?,
        })
    }

    /// Imports decryptable Chrome saved credentials into the encrypted vault.
    pub fn import_chrome_saved_credentials(&self) -> Result<ImportReport> {
        self.sync_browser_source(BrowserSource::Chrome)
    }

    /// Back-compat alias for [`Self::import_chrome_saved_credentials`].
    pub fn sync_chrome_saved_credentials(&self) -> Result<ImportReport> {
        self.sync_browser_source(BrowserSource::Chrome)
    }

    /// Imports decryptable saved credentials from one browser source.
    pub fn sync_browser_source(&self, source: BrowserSource) -> Result<ImportReport> {
        let credentials = source.load_credentials()?;
        let mut imported = 0usize;
        let mut skipped = 0usize;
        let mut errors = Vec::new();
        for credential in credentials {
            if credential.password.is_empty() {
                skipped += 1;
                continue;
            }
            let label = browser_secret_label(source, &credential);
            match self.put(SecretUpsert {
                id: None,
                label,
                description: Some(site_description(&credential.origin_url)),
                value: credential.password,
                username: non_empty_option(Some(credential.username)),
                origin: Some(credential.origin_url),
                source: source.id().to_string(),
            }) {
                Ok(_) => imported += 1,
                Err(error) => errors.push(error.to_string()),
            }
        }
        Ok(ImportReport {
            imported,
            skipped,
            errors,
        })
    }

    /// Imports every accessible 1Password login via the `op` CLI, resolving each
    /// to its plaintext value and storing it in the encrypted vault.
    pub fn sync_onepassword_references(&self) -> Result<ImportReport> {
        // Detect-first: if `op` isn't authorized yet, open the app best-effort and
        // return actionable guidance — so the user never has to configure it by
        // hand (falls back to the `.1pux` path the UI also offers).
        onepassword::connect_onepassword()?;
        // Batch-fetch every login WITH its values (one structured `op item get`
        // per item; the whole batch is one biometric authorization). A single
        // item that fails to fetch is skipped + reported, not fatal.
        let (logins, fetch_errors) = onepassword::import_resolved_logins()?;
        let mut report = self.store_resolved_logins(logins);
        report.errors.extend(fetch_errors);
        Ok(report)
    }

    /// Imports 1Password logins from a `.1pux` export file (no `op` CLI / no app
    /// integration — just the file the desktop app produces). Imports every vault
    /// in the file.
    pub fn sync_onepassword_export(&self, path: &std::path::Path) -> Result<ImportReport> {
        let logins = onepassword_1pux::import_logins(path)?;
        Ok(self.store_resolved_logins(logins))
    }

    /// Stores a batch of resolved 1Password logins into the vault (encrypted at
    /// rest, exactly like browser-imported credentials). Items without a password
    /// are skipped; per-item `put` failures are reported, not fatal. Shared by the
    /// `op` CLI path and the `.1pux` export-file path.
    fn store_resolved_logins(&self, logins: Vec<onepassword::ResolvedLogin>) -> ImportReport {
        let mut imported = 0usize;
        let mut skipped = 0usize;
        let mut errors = Vec::new();
        for login in logins {
            if login.password.is_empty() {
                skipped += 1;
                continue;
            }
            match self.put(SecretUpsert {
                id: None,
                label: login.label,
                description: Some("1Password login".to_string()),
                value: login.password,
                username: non_empty_option(login.username),
                origin: login.origin,
                source: "1password".to_string(),
            }) {
                Ok(_) => imported += 1,
                Err(error) => errors.push(error.to_string()),
            }
        }
        ImportReport {
            imported,
            skipped,
            errors,
        }
    }

    fn load_store(&self) -> Result<SecretStoreFile> {
        load_store_file(&self.path)
    }

    fn save_store(&self, store: &SecretStoreFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create secret store dir {}", parent.display()))?;
        }
        let serialized = serde_json::to_string_pretty(store)?;
        let temp_path = self.path.with_extension("json.tmp");
        fs::write(&temp_path, serialized)
            .with_context(|| format!("write secret store {}", temp_path.display()))?;
        set_private_permissions(&temp_path);
        fs::rename(&temp_path, &self.path)
            .with_context(|| format!("replace secret store {}", self.path.display()))?;
        set_private_permissions(&self.path);
        Ok(())
    }

    fn encrypt(&self, value: &str) -> Result<EncryptedValue> {
        let cipher = Aes256Gcm::new_from_slice(&self.key).context("initialize secret cipher")?;
        let mut nonce_bytes = [0u8; NONCE_BYTES];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), value.as_bytes())
            .map_err(|_| anyhow::anyhow!("encrypt secret value"))?;
        Ok(EncryptedValue {
            algorithm: "AES-256-GCM".to_string(),
            nonce: BASE64.encode(nonce_bytes),
            ciphertext: BASE64.encode(ciphertext),
        })
    }

    fn decrypt(&self, value: &EncryptedValue) -> Result<String> {
        if value.algorithm != "AES-256-GCM" {
            bail!(
                "unsupported secret encryption algorithm `{}`",
                value.algorithm
            );
        }
        let nonce = BASE64.decode(&value.nonce).context("decode secret nonce")?;
        if nonce.len() != NONCE_BYTES {
            bail!("secret nonce has invalid length");
        }
        let ciphertext = BASE64
            .decode(&value.ciphertext)
            .context("decode secret ciphertext")?;
        let cipher = Aes256Gcm::new_from_slice(&self.key).context("initialize secret cipher")?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
            .map_err(|_| anyhow::anyhow!("decrypt secret value"))?;
        String::from_utf8(plaintext).context("secret value is not UTF-8")
    }
}

fn load_store_file(path: &Path) -> Result<SecretStoreFile> {
    if !path.exists() {
        return Ok(SecretStoreFile {
            version: STORE_VERSION,
            secrets: Vec::new(),
        });
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read secret store {}", path.display()))?;
    let store: SecretStoreFile = serde_json::from_str(&raw)
        .with_context(|| format!("parse secret store {}", path.display()))?;
    if store.version != STORE_VERSION {
        bail!(
            "unsupported secret store version {} in {}",
            store.version,
            path.display()
        );
    }
    Ok(store)
}

fn secret_summary(record: &SecretRecord) -> SecretSummary {
    SecretSummary {
        id: record.id.clone(),
        label: record.label.clone(),
        description: record.description.clone(),
        username: record.username.clone(),
        origin: record.origin.clone(),
        source: record.source.clone(),
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
    }
}

fn secret_search_text(record: &SecretRecord) -> String {
    [
        record.id.as_str(),
        record.label.as_str(),
        record.description.as_deref().unwrap_or_default(),
        record.username.as_deref().unwrap_or_default(),
        record.origin.as_deref().unwrap_or_default(),
        record.source.as_str(),
    ]
    .join(" ")
}

fn secret_search_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for term in query
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
    {
        if !terms.contains(&term) {
            terms.push(term);
        }
    }
    terms
}

fn secret_search_score(record: &SecretRecord, terms: &[String]) -> usize {
    if terms.is_empty() {
        return 1;
    }
    let text = secret_search_text(record).to_ascii_lowercase();
    let has_specific_terms = terms
        .iter()
        .any(|term| !is_generic_secret_search_term(term));
    let mut specific_score = 0usize;
    let mut generic_score = 0usize;
    for term in terms {
        if !text.contains(term) {
            continue;
        }
        if is_generic_secret_search_term(term) {
            generic_score += 1;
        } else {
            specific_score += 10;
        }
    }
    if has_specific_terms && specific_score == 0 {
        0
    } else {
        specific_score + generic_score
    }
}

fn is_generic_secret_search_term(term: &str) -> bool {
    matches!(
        term,
        "account"
            | "accounts"
            | "api"
            | "auth"
            | "authentication"
            | "com"
            | "credential"
            | "credentials"
            | "email"
            | "http"
            | "https"
            | "key"
            | "keys"
            | "login"
            | "logins"
            | "password"
            | "passwords"
            | "secret"
            | "secrets"
            | "signin"
            | "site"
            | "token"
            | "tokens"
            | "user"
            | "username"
            | "website"
            | "www"
    )
}

fn browser_secret_label(source: BrowserSource, credential: &ImportedCredential) -> String {
    let host = site_description(&credential.origin_url);
    match credential.username.trim() {
        "" => format!("{} {host}", source.label()),
        username => format!("{} {username} @ {host}", source.label()),
    }
}

fn site_description(origin_url: &str) -> String {
    origin_url
        .split("://")
        .nth(1)
        .unwrap_or(origin_url)
        .split('/')
        .next()
        .unwrap_or(origin_url)
        .trim()
        .trim_end_matches(':')
        .to_string()
}

fn normalized_required(field: &str, value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("{field} cannot be empty");
    }
    Ok(trimmed.to_string())
}

fn normalized_source(source: &str) -> String {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        "manual".to_string()
    } else {
        trimmed.to_string()
    }
}

fn non_empty_option(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn set_private_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; KEY_BYTES] {
        [7u8; KEY_BYTES]
    }

    #[test]
    fn stores_secret_encrypted_and_reveals_by_label() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("secrets.json");
        let vault = SecretVault::open_with_key(&store_path, test_key());
        let summary = vault
            .put(SecretUpsert {
                id: None,
                label: "GitHub token".to_string(),
                description: Some("GitHub personal access token".to_string()),
                value: "ghp_secret".to_string(),
                username: Some("octo".to_string()),
                origin: Some("https://github.com".to_string()),
                source: "manual".to_string(),
            })
            .unwrap();
        let raw = fs::read_to_string(dir.path().join("secrets.json")).unwrap();
        assert!(!raw.contains("ghp_secret"));
        assert_eq!(vault.list().unwrap(), vec![summary.clone()]);
        assert_eq!(
            SecretVault::list_metadata(&store_path).unwrap(),
            vec![summary.clone()]
        );
        let revealed = vault.reveal("GitHub token").unwrap();
        assert_eq!(revealed.id, summary.id);
        assert_eq!(
            revealed.description.as_deref(),
            Some("GitHub personal access token")
        );
        assert_eq!(revealed.username.as_deref(), Some("octo"));
        assert_eq!(revealed.origin.as_deref(), Some("https://github.com"));
        assert_eq!(revealed.value, "ghp_secret");
    }

    #[test]
    fn searches_secret_metadata_without_decrypting_values() {
        let dir = tempfile::tempdir().unwrap();
        let vault = SecretVault::open_with_key(dir.path().join("secrets.json"), test_key());
        vault
            .put(SecretUpsert {
                id: None,
                label: "Deploy token".to_string(),
                description: Some("production deploys".to_string()),
                value: "secret-value".to_string(),
                username: Some("ci".to_string()),
                origin: Some("https://deploy.example".to_string()),
                source: "manual".to_string(),
            })
            .unwrap();

        let results = vault.search("deploy production", 10).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].label, "Deploy token");
        assert_eq!(
            results[0].description.as_deref(),
            Some("production deploys")
        );
    }

    #[test]
    fn searches_keyword_heavy_login_query_by_specific_terms() {
        let dir = tempfile::tempdir().unwrap();
        let vault = SecretVault::open_with_key(dir.path().join("secrets.json"), test_key());
        vault
            .put(SecretUpsert {
                id: None,
                label: "Generic account password".to_string(),
                description: Some("saved login".to_string()),
                value: "unrelated-secret".to_string(),
                username: None,
                origin: Some("https://example.com".to_string()),
                source: "manual".to_string(),
            })
            .unwrap();
        vault
            .put(SecretUpsert {
                id: None,
                label: "Chrome user@example.com @ www.doordash.com".to_string(),
                description: Some("saved Chrome credential".to_string()),
                value: "doordash-secret".to_string(),
                username: Some("user@example.com".to_string()),
                origin: Some("https://www.doordash.com".to_string()),
                source: "chrome".to_string(),
            })
            .unwrap();

        let results = vault
            .search("doordash DoorDash login account password", 10)
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].origin.as_deref(),
            Some("https://www.doordash.com")
        );
    }

    #[test]
    fn updates_existing_secret_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let vault = SecretVault::open_with_key(dir.path().join("secrets.json"), test_key());
        let first = vault
            .put(SecretUpsert {
                id: None,
                label: "API".to_string(),
                description: None,
                value: "old".to_string(),
                username: None,
                origin: None,
                source: "manual".to_string(),
            })
            .unwrap();
        let second = vault
            .put(SecretUpsert {
                id: Some(first.id.clone()),
                label: "API".to_string(),
                description: Some("updated API key".to_string()),
                value: "new".to_string(),
                username: None,
                origin: None,
                source: "manual".to_string(),
            })
            .unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(second.description.as_deref(), Some("updated API key"));
        assert_eq!(vault.list().unwrap().len(), 1);
        assert_eq!(vault.reveal(&first.id).unwrap().value, "new");
    }
}
