//! 1Password `.1pux` export-file import (no `op` CLI / no app integration).
//!
//! A `.1pux` file is a ZIP archive whose `export.data` entry is a JSON document
//! of the user's account(s), organised by vault. This module reads that file
//! directly so credentials can be imported from just the 1Password desktop app:
//! the user does File -> Export -> 1PUX in the app, and Puffer parses the result
//! — no `op` binary, no service-account token, no desktop-app integration.
//!
//! The export contains PLAINTEXT secrets. This module never deletes it — the
//! user chose the export location and is responsible for removing it; callers
//! should treat the path as sensitive but leave the file in place.

use crate::onepassword::ResolvedLogin;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::io::Read;
use std::path::Path;

/// 1Password category ids we import as logins.
const CATEGORY_LOGIN: &str = "001";
const CATEGORY_PASSWORD: &str = "005";

/// Top-level `export.data` document. Deserialization is intentionally lenient
/// (`#[serde(default)]`, unknown fields ignored) to tolerate schema drift across
/// 1Password versions.
#[derive(Debug, Default, Deserialize)]
struct Export {
    #[serde(default)]
    accounts: Vec<Account>,
}

#[derive(Debug, Default, Deserialize)]
struct Account {
    #[serde(default)]
    vaults: Vec<Vault>,
}

#[derive(Debug, Default, Deserialize)]
struct Vault {
    #[serde(default)]
    items: Vec<Item>,
}

#[derive(Debug, Default, Deserialize)]
struct Item {
    #[serde(default)]
    state: String,
    #[serde(default, rename = "categoryUuid")]
    category_uuid: String,
    #[serde(default)]
    details: Details,
    #[serde(default)]
    overview: Overview,
}

#[derive(Debug, Default, Deserialize)]
struct Details {
    #[serde(default, rename = "loginFields")]
    login_fields: Vec<LoginField>,
    /// Present on Password-category (005) items.
    #[serde(default)]
    password: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct LoginField {
    #[serde(default)]
    value: String,
    /// "username" or "password" for the canonical login fields.
    #[serde(default)]
    designation: String,
}

#[derive(Debug, Default, Deserialize)]
struct Overview {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    urls: Vec<UrlEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct UrlEntry {
    #[serde(default)]
    url: String,
}

/// Reads the `export.data` JSON entry out of a `.1pux` ZIP archive.
fn read_export_data(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("open 1Password export `{}`", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("`{}` is not a valid .1pux (zip) archive", path.display()))?;
    let mut entry = archive
        .by_name("export.data")
        .context("`.1pux` archive has no `export.data` entry")?;
    // Cap the decompressed read so a crafted/zip-bomb .1pux can't OOM the daemon.
    const MAX_EXPORT_BYTES: u64 = 256 * 1024 * 1024;
    let mut buf = Vec::new();
    entry
        .take(MAX_EXPORT_BYTES + 1)
        .read_to_end(&mut buf)
        .context("read `export.data` from .1pux")?;
    if buf.len() as u64 > MAX_EXPORT_BYTES {
        bail!(
            "1Password export `export.data` is unexpectedly large (> {} MB)",
            MAX_EXPORT_BYTES / (1024 * 1024)
        );
    }
    Ok(buf)
}

fn parse_export(bytes: &[u8]) -> Result<Export> {
    serde_json::from_slice(bytes).context("parse 1Password `export.data` JSON")
}

fn load_export(path: &Path) -> Result<Export> {
    parse_export(&read_export_data(path)?)
}

/// Imports login/password items from a `.1pux` export as resolved logins, ready
/// to store. Imports every vault in the file; archived/trashed items are skipped.
pub fn import_logins(path: &Path) -> Result<Vec<ResolvedLogin>> {
    let export = load_export(path)?;
    let mut out = Vec::new();
    for account in &export.accounts {
        for vault in &account.vaults {
            for item in &vault.items {
                if let Some(login) = item_to_login(item) {
                    out.push(login);
                }
            }
        }
    }
    Ok(out)
}

/// Maps one export item to a login, or None if it is not an active login/password.
fn item_to_login(item: &Item) -> Option<ResolvedLogin> {
    // Only active items (skip "archived"/"trashed"); empty state means active.
    if !item.state.is_empty() && item.state != "active" {
        return None;
    }
    if item.category_uuid != CATEGORY_LOGIN && item.category_uuid != CATEGORY_PASSWORD {
        return None;
    }
    let login_field = |designation: &str| -> Option<String> {
        item.details
            .login_fields
            .iter()
            .find(|field| field.designation.eq_ignore_ascii_case(designation))
            .map(|field| field.value.clone())
            .filter(|value| !value.is_empty())
    };
    // Password: login field for 001, the dedicated `password` for 005.
    let password = login_field("password")
        .or_else(|| item.details.password.clone())
        .unwrap_or_default();
    if password.is_empty() {
        return None;
    }
    let username = login_field("username");
    let label = if item.overview.title.is_empty() {
        "1Password item".to_string()
    } else {
        item.overview.title.clone()
    };
    let origin = if !item.overview.url.is_empty() {
        Some(item.overview.url.clone())
    } else {
        item.overview
            .urls
            .iter()
            .map(|entry| entry.url.clone())
            .find(|url| !url.is_empty())
    };
    Some(ResolvedLogin {
        label,
        username,
        password,
        origin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const EXPORT_JSON: &str = r#"{
        "accounts": [{
            "attrs": {"name": "Personal Account"},
            "vaults": [
                {
                    "attrs": {"uuid": "vault_personal", "name": "Personal"},
                    "items": [
                        {
                            "state": "active", "categoryUuid": "001",
                            "details": {"loginFields": [
                                {"value": "neo", "designation": "username"},
                                {"value": "trinity", "designation": "password"}
                            ]},
                            "overview": {"title": "Matrix", "url": "https://matrix.test"}
                        },
                        {
                            "state": "active", "categoryUuid": "005",
                            "details": {"password": "just-a-password"},
                            "overview": {"title": "Wifi", "urls": [{"url": "https://router.test"}]}
                        },
                        {
                            "state": "archived", "categoryUuid": "001",
                            "details": {"loginFields": [{"value": "old", "designation": "password"}]},
                            "overview": {"title": "Archived"}
                        },
                        {
                            "state": "active", "categoryUuid": "002",
                            "details": {}, "overview": {"title": "Credit Card"}
                        }
                    ]
                },
                {
                    "attrs": {"uuid": "vault_shared", "name": "Shared"},
                    "items": [{
                        "state": "active", "categoryUuid": "001",
                        "details": {"loginFields": [{"value": "shared-secret", "designation": "password"}]},
                        "overview": {"title": "SharedThing"}
                    }]
                }
            ]
        }]
    }"#;

    /// Builds a minimal `.1pux` (a zip with a stored `export.data`) in a tempfile.
    fn write_1pux(json: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.1pux");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("export.data", opts).unwrap();
        zip.write_all(json.as_bytes()).unwrap();
        zip.finish().unwrap();
        (dir, path)
    }

    #[test]
    fn imports_all_vaults_skipping_non_logins_and_archived() {
        let (_dir, path) = write_1pux(EXPORT_JSON);
        let logins = import_logins(&path).unwrap();
        // Login(001) + Password(005) in Personal + Login in Shared = 3.
        // Archived item and CreditCard(002) are skipped.
        assert_eq!(logins.len(), 3);
        let matrix = logins.iter().find(|l| l.label == "Matrix").unwrap();
        assert_eq!(matrix.username.as_deref(), Some("neo"));
        assert_eq!(matrix.password, "trinity");
        assert_eq!(matrix.origin.as_deref(), Some("https://matrix.test"));
        let wifi = logins.iter().find(|l| l.label == "Wifi").unwrap();
        assert_eq!(wifi.username, None);
        assert_eq!(wifi.password, "just-a-password");
        assert_eq!(wifi.origin.as_deref(), Some("https://router.test"));
    }

    #[test]
    fn rejects_non_zip_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.1pux");
        std::fs::write(&path, b"not a zip").unwrap();
        assert!(import_logins(&path).is_err());
    }
}
