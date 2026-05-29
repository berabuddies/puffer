//! Best-effort local Slack desktop session import.

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_SCAN_FILES: usize = 5000;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Options for importing a browser session from local Slack app storage.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlackLocalImportOptions {
    /// Optional root path to scan instead of standard Slack app locations.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Optional workspace URL used to disambiguate multi-workspace profiles.
    #[serde(default)]
    pub workspace_url: Option<String>,
}

/// Browser-session tokens extracted from local Slack storage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlackLocalImport {
    /// Workspace URL used for browser-token Web API calls.
    pub workspace_url: String,
    /// Browser `d` cookie value.
    pub xoxd_token: String,
    /// Browser API token.
    pub xoxc_token: String,
    /// File that supplied the best matching tokens.
    pub source_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
struct ScanState {
    xoxd_tokens: BTreeSet<String>,
    xoxc_tokens: BTreeSet<String>,
    workspace_urls: BTreeSet<String>,
    best_source: Option<PathBuf>,
}

/// Imports a Slack browser session from local Slack app data.
pub fn import_local_slack_session(options: SlackLocalImportOptions) -> Result<SlackLocalImport> {
    let roots = scan_roots(options.path.as_deref())?;
    let workspace_hint = options
        .workspace_url
        .as_deref()
        .map(normalize_workspace_url)
        .transpose()?;
    let mut scanned_files = 0usize;
    let mut best = ScanState::default();
    let mut fallback = ScanState::default();
    for root in roots {
        scan_path(
            &root,
            workspace_hint.as_deref(),
            &mut scanned_files,
            &mut best,
            &mut fallback,
        )?;
        if has_tokens(&best) && best.workspace_urls.iter().next().is_some() {
            break;
        }
    }
    let state = select_import_state(&best, &fallback, workspace_hint.as_deref())?;
    if workspace_hint.is_none() && state.workspace_urls.len() > 1 {
        bail!(
            "found multiple Slack workspaces in local app data; pass --workspace-url to choose one"
        );
    }
    let xoxd_token = select_single(
        &state.xoxd_tokens,
        "Slack xoxd browser cookie",
        "pass --path for a narrower profile or use login-browser with explicit tokens",
    )?;
    let xoxc_token = select_single(
        &state.xoxc_tokens,
        "Slack xoxc browser token",
        "pass --path for a narrower profile or use login-browser with explicit tokens",
    )?;
    let workspace_url = match workspace_hint {
        Some(url) => url,
        None => state.workspace_urls.iter().next().cloned().ok_or_else(|| {
            anyhow!("found Slack browser tokens but no workspace URL; pass --workspace-url")
        })?,
    };
    let source_path = state
        .best_source
        .clone()
        .ok_or_else(|| anyhow!("Slack token source path was not recorded"))?;
    Ok(SlackLocalImport {
        workspace_url,
        xoxd_token,
        xoxc_token,
        source_path,
    })
}

fn select_import_state(
    best: &ScanState,
    fallback: &ScanState,
    workspace_hint: Option<&str>,
) -> Result<ScanState> {
    if workspace_hint.is_none() {
        return Ok(if has_tokens(best) {
            best.clone()
        } else {
            fallback.clone()
        });
    }

    let mut selected = best.clone();
    if selected.xoxd_tokens.is_empty() && fallback.xoxd_tokens.len() == 1 {
        selected.xoxd_tokens = fallback.xoxd_tokens.clone();
    }
    if selected.xoxc_tokens.is_empty() && fallback.xoxc_tokens.len() == 1 {
        selected.xoxc_tokens = fallback.xoxc_tokens.clone();
    }
    if selected.best_source.is_none() {
        selected.best_source = fallback.best_source.clone();
    }
    if has_tokens(&selected) {
        return Ok(selected);
    }
    bail!(
        "no complete Slack browser token pair was found for the requested workspace; pass --path for that profile or use login-browser"
    )
}

fn select_single(tokens: &BTreeSet<String>, label: &str, hint: &str) -> Result<String> {
    match tokens.len() {
        0 => Err(anyhow!("no {label} found in local app data")),
        1 => Ok(tokens.iter().next().expect("one token").clone()),
        _ => Err(anyhow!("found multiple {label} candidates; {hint}")),
    }
}

fn scan_roots(explicit: Option<&Path>) -> Result<Vec<PathBuf>> {
    if let Some(path) = explicit {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Library/Application Support/Slack"));
        roots.push(home.join(
            "Library/Containers/com.tinyspeck.slackmacgap/Data/Library/Application Support/Slack",
        ));
        roots.push(home.join(".config/Slack"));
        roots.push(home.join("AppData/Roaming/Slack"));
    }
    roots.retain(|path| path.exists());
    if roots.is_empty() {
        bail!("no local Slack app data directories found; pass --path to scan a profile");
    }
    Ok(roots)
}

fn scan_path(
    path: &Path,
    workspace_hint: Option<&str>,
    scanned_files: &mut usize,
    best: &mut ScanState,
    fallback: &mut ScanState,
) -> Result<()> {
    if *scanned_files >= MAX_SCAN_FILES {
        return Ok(());
    }
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(()),
    };
    if metadata.is_dir() {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            scan_path(&entry.path(), workspace_hint, scanned_files, best, fallback)?;
            if *scanned_files >= MAX_SCAN_FILES {
                break;
            }
        }
        return Ok(());
    }
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Ok(());
    }
    *scanned_files += 1;
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(()),
    };
    let text = String::from_utf8_lossy(&bytes);
    let found = extract_from_text(&text).with_context(|| format!("scan {}", path.display()))?;
    if found.xoxc_tokens.is_empty()
        && found.xoxd_tokens.is_empty()
        && found.workspace_urls.is_empty()
    {
        return Ok(());
    }
    merge_state(fallback, &found, path);
    if workspace_hint
        .map(|hint| found.workspace_urls.iter().any(|url| url == hint))
        .unwrap_or_else(|| has_tokens(&found) && !found.workspace_urls.is_empty())
    {
        merge_state(best, &found, path);
    }
    Ok(())
}

fn extract_from_text(text: &str) -> Result<ScanState> {
    let xoxd = Regex::new(r"xoxd-[A-Za-z0-9%_\-]+")?;
    let xoxc = Regex::new(r"xoxc-[A-Za-z0-9%_\-]+")?;
    let workspace_url = Regex::new(r"https://[A-Za-z0-9][A-Za-z0-9.\-]*\.slack\.com")?;
    let mut state = ScanState::default();
    state.xoxd_tokens.extend(
        xoxd.find_iter(text)
            .map(|m| percent_decode_token(m.as_str())),
    );
    state
        .xoxc_tokens
        .extend(xoxc.find_iter(text).map(|m| m.as_str().to_string()));
    for found in workspace_url.find_iter(text) {
        let normalized = normalize_workspace_url(found.as_str())?;
        if normalized != "https://app.slack.com" {
            state.workspace_urls.insert(normalized);
        }
    }
    Ok(state)
}

fn merge_state(target: &mut ScanState, found: &ScanState, path: &Path) {
    if target.best_source.is_none() && has_tokens(found) {
        target.best_source = Some(path.to_path_buf());
    }
    target.xoxd_tokens.extend(found.xoxd_tokens.iter().cloned());
    target.xoxc_tokens.extend(found.xoxc_tokens.iter().cloned());
    target
        .workspace_urls
        .extend(found.workspace_urls.iter().cloned());
}

fn has_tokens(state: &ScanState) -> bool {
    !state.xoxd_tokens.is_empty() && !state.xoxc_tokens.is_empty()
}

/// Normalizes and validates a Slack workspace URL.
pub fn normalize_workspace_url(value: &str) -> Result<String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        bail!("workspace URL is empty");
    }
    let url = if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    let parsed = url::Url::parse(&url).context("parse Slack workspace URL")?;
    let Some(host) = parsed.host_str() else {
        bail!("Slack workspace URL has no host");
    };
    if !host.ends_with(".slack.com") || host == "app.slack.com" {
        bail!("Slack workspace URL must be a workspace subdomain");
    }
    Ok(format!("https://{host}"))
}

fn percent_decode_token(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Some(byte) = hex_pair(bytes[index + 1], bytes[index + 2]) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_pair(high: u8, low: u8) -> Option<u8> {
    Some(hex_value(high)? << 4 | hex_value(low)?)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_local_session_extracts_tokens_from_fixture() {
        let temp = tempfile::tempdir().unwrap();
        let fixture = temp.path().join("Local Storage").join("leveldb");
        fs::create_dir_all(&fixture).unwrap();
        fs::write(
            fixture.join("000003.log"),
            r#"{"url":"https://acme.slack.com","token":"xoxc-abc_123","cookie":"xoxd-def_456"}"#,
        )
        .unwrap();

        let imported = import_local_slack_session(SlackLocalImportOptions {
            path: Some(temp.path().to_path_buf()),
            workspace_url: None,
        })
        .unwrap();

        assert_eq!(imported.workspace_url, "https://acme.slack.com");
        assert_eq!(imported.xoxc_token, "xoxc-abc_123");
        assert_eq!(imported.xoxd_token, "xoxd-def_456");
    }

    #[test]
    fn workspace_hint_can_supply_missing_url() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("state"), "xoxc-abc xoxd-def").unwrap();

        let imported = import_local_slack_session(SlackLocalImportOptions {
            path: Some(temp.path().to_path_buf()),
            workspace_url: Some("acme.slack.com".into()),
        })
        .unwrap();

        assert_eq!(imported.workspace_url, "https://acme.slack.com");
    }

    #[test]
    fn import_decodes_url_encoded_xoxd_cookie() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("state"),
            "https://acme.slack.com xoxc-abc xoxd-encoded%2Btoken%2Fwith%3Dspecial",
        )
        .unwrap();

        let imported = import_local_slack_session(SlackLocalImportOptions {
            path: Some(temp.path().to_path_buf()),
            workspace_url: None,
        })
        .unwrap();

        assert_eq!(imported.xoxd_token, "xoxd-encoded+token/with=special");
    }

    #[test]
    fn import_rejects_multiple_workspaces_without_hint() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("acme-state"),
            "https://acme.slack.com xoxc-acme xoxd-acme",
        )
        .unwrap();
        fs::write(
            temp.path().join("side-state"),
            "https://side.slack.com xoxc-side xoxd-side",
        )
        .unwrap();

        let error = import_local_slack_session(SlackLocalImportOptions {
            path: Some(temp.path().to_path_buf()),
            workspace_url: None,
        })
        .unwrap_err()
        .to_string();

        assert!(error.contains("multiple Slack workspaces"));
    }

    #[test]
    fn workspace_hint_selects_matching_tokens() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("acme-state"),
            "https://acme.slack.com xoxc-acme xoxd-acme",
        )
        .unwrap();
        fs::write(
            temp.path().join("side-state"),
            "https://side.slack.com xoxc-side xoxd-side",
        )
        .unwrap();

        let imported = import_local_slack_session(SlackLocalImportOptions {
            path: Some(temp.path().to_path_buf()),
            workspace_url: Some("side.slack.com".into()),
        })
        .unwrap();

        assert_eq!(imported.workspace_url, "https://side.slack.com");
        assert_eq!(imported.xoxc_token, "xoxc-side");
        assert_eq!(imported.xoxd_token, "xoxd-side");
    }

    #[test]
    fn import_rejects_multiple_token_candidates() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("state"),
            "https://acme.slack.com xoxc-aaa xoxc-bbb xoxd-cookie",
        )
        .unwrap();

        let error = import_local_slack_session(SlackLocalImportOptions {
            path: Some(temp.path().to_path_buf()),
            workspace_url: None,
        })
        .unwrap_err()
        .to_string();

        assert!(error.contains("multiple Slack xoxc browser token"));
    }

    #[test]
    fn invalid_workspace_hint_is_rejected() {
        let error = import_local_slack_session(SlackLocalImportOptions {
            path: Some(PathBuf::from("/tmp/does-not-matter")),
            workspace_url: Some("https://app.slack.com".into()),
        })
        .unwrap_err()
        .to_string();

        assert!(error.contains("workspace subdomain"));
    }
}
