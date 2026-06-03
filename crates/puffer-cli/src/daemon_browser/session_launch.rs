//! Browser root launch helpers shared by Chrome and CEF-backed sessions.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::browser_profiles::ChromeProfileLaunch;

use super::launch_settings::BrowserLaunchSettings;
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
    launch_settings: &BrowserLaunchSettings,
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
        .arg("--enable-extensions")
        .arg("--allow-file-access")
        .arg("--allow-file-access-from-files")
        .arg("--force-color-profile=srgb")
        .arg(format!("--window-size={width},{height}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(profile_directory) = &launch.profile_directory {
        command.arg(format!("--profile-directory={profile_directory}"));
    }
    let extension_dirs = launch_settings.extension_dirs();
    if !extension_dirs.is_empty() {
        let joined = extension_dirs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(",");
        command
            .arg("--enable-unsafe-extension-debugging")
            .arg(format!("--disable-extensions-except={joined}"))
            .arg(format!("--load-extension={joined}"));
    }
    command.arg(DEFAULT_URL);
}

/// Returns the CEF remote debugging port advertised by the desktop launcher.
pub(super) fn cef_remote_debugging_port() -> Option<u16> {
    std::env::var("PUFFER_CEF_REMOTE_DEBUGGING_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port >= 1024)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_profiles::ChromeProfileLaunch;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    #[test]
    fn chrome_extension_flags_are_emitted_before_default_url() {
        let launch = ChromeProfileLaunch {
            user_data_dir: PathBuf::from("/tmp/puffer-profile"),
            profile_directory: None,
            owns_user_data_dir: true,
        };
        let settings = BrowserLaunchSettings::with_extension_dirs(vec![
            PathBuf::from("/tmp/nopecha"),
            PathBuf::from("/tmp/2captcha"),
        ]);
        let mut command = Command::new("chrome");

        configure_chrome_command(&mut command, &launch, 800, 600, &settings);

        let args = command
            .get_args()
            .map(OsStr::to_string_lossy)
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        assert!(args.contains(&"--enable-extensions".to_string()));
        assert!(args.contains(&"--enable-unsafe-extension-debugging".to_string()));
        assert!(
            args.contains(&"--disable-extensions-except=/tmp/nopecha,/tmp/2captcha".to_string())
        );
        assert!(args.contains(&"--load-extension=/tmp/nopecha,/tmp/2captcha".to_string()));
        assert_eq!(args.last().map(String::as_str), Some(DEFAULT_URL));
    }
}
