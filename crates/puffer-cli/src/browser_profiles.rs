//! Managed Chrome profile ownership for Puffer browser sessions.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

const MANAGED_PROFILE_MARKER: &str = ".puffer-managed-profile-version";
const MANAGED_PROFILE_VERSION: &str = "2";

/// Profile launch settings for a Chrome user-data directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ChromeProfileLaunch {
    pub(crate) user_data_dir: PathBuf,
    pub(crate) profile_directory: Option<String>,
    pub(crate) owns_user_data_dir: bool,
}

/// Resolves Chrome profile launch settings for browser connector sessions.
pub(crate) fn prepare_managed_profile(managed_user_data_dir: &Path) -> Result<ChromeProfileLaunch> {
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

    Ok(ChromeProfileLaunch {
        user_data_dir: managed_user_data_dir.to_path_buf(),
        profile_directory: None,
        owns_user_data_dir: true,
    })
}

fn managed_profile_marker_matches(managed_user_data_dir: &Path) -> bool {
    fs::read_to_string(managed_user_data_dir.join(MANAGED_PROFILE_MARKER))
        .map(|value| value.trim() == MANAGED_PROFILE_VERSION)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn managed_profile_ignores_source_chrome_profile() {
        let dir = tempdir().unwrap();
        let chrome_root = dir.path().join("chrome");
        let source_profile = chrome_root.join("Profile 1");
        let managed = dir.path().join("managed");
        fs::create_dir_all(source_profile.join("Network")).unwrap();
        fs::write(chrome_root.join("Local State"), "{}").unwrap();
        fs::write(source_profile.join("Preferences"), "{}").unwrap();
        fs::write(source_profile.join("Network/Cookies"), "source cookies").unwrap();

        let launch = prepare_managed_profile(&managed).unwrap();

        assert_eq!(launch.user_data_dir, managed);
        assert_eq!(launch.profile_directory.as_deref(), None);
        assert!(launch.owns_user_data_dir);
        assert!(managed_profile_marker_matches(&managed));
        assert!(!managed.join("Local State").exists());
        assert!(!managed.join("Profile 1/Preferences").exists());
        assert!(!managed.join("Profile 1/Network/Cookies").exists());
    }

    #[test]
    fn managed_profile_is_used_when_no_profile_is_selected() {
        let dir = tempdir().unwrap();
        let managed = dir.path().join("managed");

        let launch = prepare_managed_profile(&managed).unwrap();

        assert_eq!(launch.user_data_dir, managed);
        assert_eq!(launch.profile_directory.as_deref(), None);
        assert!(launch.owns_user_data_dir);
        assert!(managed_profile_marker_matches(&managed));
        assert!(!managed.join("Local State").exists());
    }
}
