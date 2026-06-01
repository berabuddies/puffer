//! Browser root launch helpers shared by Chrome and CEF-backed sessions.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::browser_profiles::ChromeProfileLaunch;

use super::DEFAULT_URL;

/// Removes a stale DevTools endpoint marker from a managed Chrome profile.
pub(super) fn remove_stale_devtools_port(profile_dir: &Path) -> Result<()> {
    match std::fs::remove_file(profile_dir.join("DevToolsActivePort")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("remove stale Chrome DevToolsActivePort"),
    }
}

/// Adds the managed Chrome launch flags used by the screencast fallback backend.
pub(super) fn configure_chrome_command(
    command: &mut Command,
    launch: &ChromeProfileLaunch,
    width: u32,
    height: u32,
) {
    command
        .arg("--headless=new")
        .arg("--remote-debugging-port=0")
        .arg(format!(
            "--user-data-dir={}",
            launch.user_data_dir.display()
        ))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking")
        .arg("--disable-features=Translate")
        .arg("--disable-gpu")
        .arg("--allow-file-access")
        .arg("--allow-file-access-from-files")
        .arg("--force-color-profile=srgb")
        .arg(format!("--window-size={width},{height}"))
        .arg(DEFAULT_URL)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(profile_directory) = &launch.profile_directory {
        command.arg(format!("--profile-directory={profile_directory}"));
    }
}

/// Returns the CEF remote debugging port advertised by the desktop launcher.
pub(super) fn cef_remote_debugging_port() -> Option<u16> {
    std::env::var("PUFFER_CEF_REMOTE_DEBUGGING_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port >= 1024)
}
