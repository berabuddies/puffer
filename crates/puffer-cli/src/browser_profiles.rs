//! Chrome profile discovery and managed profile ownership.

use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const MANAGED_PROFILE_MARKER: &str = ".puffer-managed-profile-version";
const MANAGED_PROFILE_VERSION: &str = "2";

/// User-visible Chrome profile metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ChromeProfileChoice {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) email: Option<String>,
    pub(crate) google_accounts: Vec<ChromeGoogleAccount>,
    pub(crate) path: PathBuf,
    pub(crate) is_last_used: bool,
    pub(crate) is_selected: bool,
}

/// Google account metadata discovered from a Chrome profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ChromeGoogleAccount {
    pub(crate) email: String,
    pub(crate) name: Option<String>,
    pub(crate) gaia_id: Option<String>,
}

/// Profile launch settings for a managed Chrome user-data directory.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ChromeProfileLaunch {
    pub(crate) profile_directory: Option<String>,
}

/// Lists local Chrome profiles and marks the effective profile selection.
pub(crate) fn discover_chrome_profiles(selected_profile: Option<&str>) -> Vec<ChromeProfileChoice> {
    let Some(user_data_dir) = chrome_user_data_dir() else {
        return Vec::new();
    };
    let local_state = read_local_state(&user_data_dir).unwrap_or(Value::Null);
    let last_used = last_used_profile_id(&local_state);
    let mut profiles = profile_choices_from_local_state(&user_data_dir, &local_state, &last_used);
    if profiles.is_empty() {
        let default_path = user_data_dir.join("Default");
        if default_path.is_dir() {
            profiles.push(ChromeProfileChoice {
                id: "Default".to_string(),
                name: "Default".to_string(),
                email: None,
                google_accounts: google_accounts_from_profile(&default_path),
                path: default_path,
                is_last_used: true,
                is_selected: false,
            });
        }
    }
    let selected_id = selected_profile
        .and_then(|id| profiles.iter().find(|profile| profile.id == id))
        .map(|profile| profile.id.clone())
        .or_else(|| {
            profiles
                .iter()
                .find(|profile| profile.is_last_used)
                .map(|profile| profile.id.clone())
        })
        .or_else(|| profiles.first().map(|profile| profile.id.clone()));
    for profile in &mut profiles {
        profile.is_selected = selected_id.as_deref() == Some(profile.id.as_str());
    }
    profiles.sort_by(|left, right| {
        right
            .is_selected
            .cmp(&left.is_selected)
            .then_with(|| right.is_last_used.cmp(&left.is_last_used))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.id.cmp(&right.id))
    });
    profiles
}

/// Prepares a Puffer-owned Chrome user-data directory without copying user auth state.
pub(crate) fn prepare_managed_profile(
    managed_user_data_dir: &Path,
    selected_profile: Option<&str>,
) -> Result<ChromeProfileLaunch> {
    if managed_user_data_dir.exists() && !managed_profile_marker_matches(managed_user_data_dir) {
        fs::remove_dir_all(managed_user_data_dir).with_context(|| {
            format!(
                "reset legacy managed browser profile {}",
                managed_user_data_dir.display()
            )
        })?;
    }
    fs::create_dir_all(managed_user_data_dir).with_context(|| {
        format!(
            "create browser profile directory {}",
            managed_user_data_dir.display()
        )
    })?;
    fs::write(
        managed_user_data_dir.join(MANAGED_PROFILE_MARKER),
        MANAGED_PROFILE_VERSION,
    )
    .with_context(|| {
        format!(
            "write managed browser profile marker {}",
            managed_user_data_dir.join(MANAGED_PROFILE_MARKER).display()
        )
    })?;

    let Some(profile) = effective_chrome_profile(selected_profile) else {
        return Ok(ChromeProfileLaunch::default());
    };
    fs::create_dir_all(managed_user_data_dir.join(&profile.id)).with_context(|| {
        format!(
            "create managed Chrome profile {}",
            managed_user_data_dir.join(&profile.id).display()
        )
    })?;
    Ok(ChromeProfileLaunch {
        profile_directory: Some(profile.id.clone()),
    })
}

fn managed_profile_marker_matches(managed_user_data_dir: &Path) -> bool {
    fs::read_to_string(managed_user_data_dir.join(MANAGED_PROFILE_MARKER))
        .map(|value| value.trim() == MANAGED_PROFILE_VERSION)
        .unwrap_or(false)
}

fn effective_chrome_profile(selected_profile: Option<&str>) -> Option<ChromeProfileChoice> {
    discover_chrome_profiles(selected_profile)
        .into_iter()
        .find(|profile| profile.is_selected)
}

fn profile_choices_from_local_state(
    user_data_dir: &Path,
    local_state: &Value,
    last_used: &Option<String>,
) -> Vec<ChromeProfileChoice> {
    let Some(info_cache) = local_state
        .get("profile")
        .and_then(|profile| profile.get("info_cache"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let mut profiles = Vec::new();
    for (id, value) in info_cache {
        if !valid_profile_id(id) {
            continue;
        }
        let path = user_data_dir.join(id);
        if !path.is_dir() {
            continue;
        }
        let google_accounts = google_accounts_from_profile(&path);
        profiles.push(ChromeProfileChoice {
            id: id.clone(),
            name: value
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(id)
                .to_string(),
            email: value
                .get("user_name")
                .and_then(Value::as_str)
                .filter(|email| !email.trim().is_empty())
                .map(ToString::to_string),
            google_accounts,
            path,
            is_last_used: last_used.as_deref() == Some(id.as_str()),
            is_selected: false,
        });
    }
    profiles
}

fn read_local_state(user_data_dir: &Path) -> Result<Value> {
    let raw = fs::read_to_string(user_data_dir.join("Local State")).with_context(|| {
        format!(
            "read Chrome profile metadata {}",
            user_data_dir.join("Local State").display()
        )
    })?;
    serde_json::from_str(&raw).context("parse Chrome profile metadata")
}

fn google_accounts_from_profile(profile_path: &Path) -> Vec<ChromeGoogleAccount> {
    let Ok(raw) = fs::read_to_string(profile_path.join("Preferences")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let mut accounts = Vec::new();
    collect_google_account_array(value.get("account_info"), &mut accounts);
    collect_google_account_array(value.pointer("/signin/account_info"), &mut accounts);
    accounts.sort_by(|left, right| left.email.cmp(&right.email));
    accounts.dedup_by(|left, right| left.email == right.email);
    accounts
}

fn collect_google_account_array(value: Option<&Value>, accounts: &mut Vec<ChromeGoogleAccount>) {
    let Some(items) = value.and_then(Value::as_array) else {
        return;
    };
    for item in items {
        let Some(email) = item
            .get("email")
            .or_else(|| item.get("user_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|email| looks_like_email(email))
        else {
            continue;
        };
        accounts.push(ChromeGoogleAccount {
            email: email.to_ascii_lowercase(),
            name: item
                .get("full_name")
                .or_else(|| item.get("given_name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToString::to_string),
            gaia_id: item
                .get("gaia")
                .or_else(|| item.get("gaia_id"))
                .or_else(|| item.get("account_id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(ToString::to_string),
        });
    }
}

fn looks_like_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.ends_with('.')
}

fn last_used_profile_id(local_state: &Value) -> Option<String> {
    local_state
        .get("profile")
        .and_then(|profile| profile.get("last_used"))
        .and_then(Value::as_str)
        .filter(|id| valid_profile_id(id))
        .map(ToString::to_string)
}

fn chrome_user_data_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PUFFER_CHROME_USER_DATA_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
    {
        return Some(path);
    }
    default_chrome_user_data_dirs()
        .into_iter()
        .find(|path| path.is_dir())
}

fn default_chrome_user_data_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let Some(home) = home_dir() else {
            return Vec::new();
        };
        return vec![home.join("Library/Application Support/Google/Chrome")];
    }
    #[cfg(target_os = "windows")]
    {
        let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
            return Vec::new();
        };
        return vec![local_app_data.join("Google/Chrome/User Data")];
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let Some(home) = home_dir() else {
            return Vec::new();
        };
        vec![
            home.join(".config/google-chrome"),
            home.join(".config/chromium"),
        ]
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn valid_profile_id(id: &str) -> bool {
    !id.is_empty() && !id.contains('/') && !id.contains('\\') && id != "." && id != ".."
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;
    use tempfile::tempdir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn local_state_selection_prefers_config_then_last_used() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Default")).unwrap();
        fs::create_dir_all(root.join("Profile 1")).unwrap();
        let state = json!({
            "profile": {
                "last_used": "Profile 1",
                "info_cache": {
                    "Default": {"name": "Personal", "user_name": "me@example.com"},
                    "Profile 1": {"name": "Work", "user_name": "work@example.com"}
                }
            }
        });

        let profiles =
            profile_choices_from_local_state(root, &state, &last_used_profile_id(&state));
        assert_eq!(profiles.len(), 2);
        assert!(profiles.iter().any(|profile| {
            profile.id == "Profile 1" && profile.is_last_used && profile.name == "Work"
        }));
    }

    #[test]
    fn profile_preferences_expose_google_accounts() {
        let dir = tempdir().unwrap();
        let profile = dir.path().join("Default");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            profile.join("Preferences"),
            serde_json::to_string(&json!({
                "account_info": [
                    {
                        "email": "Me@Example.COM",
                        "full_name": "Me Example",
                        "gaia": "gaia-1"
                    },
                    {
                        "email": "me@example.com",
                        "full_name": "Duplicate"
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let accounts = google_accounts_from_profile(&profile);

        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].email, "me@example.com");
        assert_eq!(accounts[0].name.as_deref(), Some("Me Example"));
        assert_eq!(accounts[0].gaia_id.as_deref(), Some("gaia-1"));
    }

    #[test]
    fn prepare_managed_profile_resets_legacy_clone_without_copying_auth() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("PUFFER_CHROME_USER_DATA_DIR");
        let dir = tempdir().unwrap();
        let chrome_root = dir.path().join("chrome");
        let source_profile = chrome_root.join("Profile 1");
        let managed = dir.path().join("managed");
        fs::create_dir_all(source_profile.join("Network")).unwrap();
        fs::create_dir_all(managed.join("Profile 1/Network")).unwrap();
        fs::write(
            chrome_root.join("Local State"),
            serde_json::to_string(&json!({
                "profile": {
                    "last_used": "Profile 1",
                    "info_cache": {
                        "Profile 1": {"name": "Work", "user_name": "work@example.com"}
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(source_profile.join("Preferences"), "{}").unwrap();
        fs::write(source_profile.join("Network/Cookies"), "source cookies").unwrap();
        fs::write(
            managed.join("Profile 1/Network/Cookies"),
            "legacy copied cookies",
        )
        .unwrap();
        std::env::set_var("PUFFER_CHROME_USER_DATA_DIR", &chrome_root);

        let launch = prepare_managed_profile(&managed, Some("Profile 1")).unwrap();

        assert_eq!(launch.profile_directory.as_deref(), Some("Profile 1"));
        assert!(managed.join("Profile 1").is_dir());
        assert!(managed_profile_marker_matches(&managed));
        assert!(!managed.join("Local State").exists());
        assert!(!managed.join("Profile 1/Preferences").exists());
        assert!(!managed.join("Profile 1/Network/Cookies").exists());

        if let Some(previous) = previous {
            std::env::set_var("PUFFER_CHROME_USER_DATA_DIR", previous);
        } else {
            std::env::remove_var("PUFFER_CHROME_USER_DATA_DIR");
        }
    }
}
