use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::io::copy;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use url::Url;

use super::CHROME_START_TIMEOUT;

const CT_REPO: &str = "berabuddies/ct";
const CT_TAG: &str = "ct";

/// Waits for Chrome to publish its browser-level DevTools WebSocket URL.
pub(super) fn read_devtools_ws_url(child: &mut Child, profile_dir: &Path) -> Result<String> {
    let stderr = child.stderr.take().context("Chrome stderr missing")?;
    spawn_stderr_drain(BufReader::new(stderr));
    let active_port_path = profile_dir.join("DevToolsActivePort");
    let start = Instant::now();
    while start.elapsed() < CHROME_START_TIMEOUT {
        if let Some(status) = child.try_wait().context("check Chrome launch status")? {
            bail!("Chrome exited before publishing a DevTools endpoint: {status}");
        }
        if let Ok(contents) = std::fs::read_to_string(&active_port_path) {
            if let Some(url) = devtools_url_from_active_port(&contents) {
                return Ok(url);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("Chrome did not publish a DevTools endpoint");
}

fn devtools_url_from_active_port(contents: &str) -> Option<String> {
    let mut lines = contents.lines();
    let port = lines.next()?.trim();
    let path = lines.next()?.trim();
    if port.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("ws://127.0.0.1:{port}{path}"))
}

fn spawn_stderr_drain<R: std::io::Read + Send + 'static>(mut reader: BufReader<R>) {
    std::thread::spawn(move || {
        let mut sink = String::new();
        while reader.read_line(&mut sink).unwrap_or(0) > 0 {
            sink.clear();
        }
    });
}

/// Returns the HTTP origin for a browser-level DevTools WebSocket URL.
pub(super) fn cdp_http_endpoint(browser_ws: &str) -> Result<String> {
    let parsed = Url::parse(browser_ws).context("parse Chrome DevTools URL")?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("Chrome DevTools URL missing host"))?;
    let port = parsed
        .port()
        .ok_or_else(|| anyhow!("Chrome DevTools URL missing port"))?;
    Ok(format!("http://{host}:{port}"))
}

/// Returns the first page target Chrome opened at startup.
pub(super) fn first_page_target(browser_ws: &str) -> Result<String> {
    let endpoint = format!("{}/json/list", cdp_http_endpoint(browser_ws)?);
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build Chrome HTTP client")?;
    let values: Vec<Value> = client
        .get(endpoint)
        .send()
        .context("list Chrome targets")?
        .error_for_status()
        .context("Chrome target listing failed")?
        .json()
        .context("parse Chrome target list response")?;
    values
        .into_iter()
        .find(|value| value.get("type").and_then(Value::as_str) == Some("page"))
        .and_then(|value| {
            value
                .get("webSocketDebuggerUrl")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .ok_or_else(|| anyhow!("Chrome target list missing page webSocketDebuggerUrl"))
}

/// Creates a new Chrome page target and returns its DevTools WebSocket URL.
pub(super) fn create_page_target(browser_ws: &str, url: &str) -> Result<String> {
    let endpoint = format!(
        "{}/json/new?{}",
        cdp_http_endpoint(browser_ws)?,
        urlencoding(url)
    );
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build Chrome HTTP client")?;
    let value: Value = client
        .put(endpoint)
        .send()
        .context("create Chrome target")?
        .error_for_status()
        .context("Chrome target creation failed")?
        .json()
        .context("parse Chrome target response")?;
    value
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("Chrome target response missing webSocketDebuggerUrl"))
}

/// Finds or downloads the custom Chromium executable Puffer should manage.
pub(super) fn ensure_chrome_executable() -> Result<PathBuf> {
    if let Some(path) = discover_chrome_executable() {
        return Ok(path);
    }
    download_ct_chrome_release()?;
    discover_chrome_executable().ok_or_else(|| {
        anyhow!("Puffer CT Chromium runtime was downloaded but no executable was found")
    })
}

/// Terminates orphaned Chrome processes that still own `profile_dir`.
pub(super) fn terminate_profile_processes(profile_dir: &Path) {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    terminate_profile_processes_unix(profile_dir);

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = profile_dir;
}

fn discover_chrome_executable() -> Option<PathBuf> {
    explicit_chrome_override()
        .or_else(cached_chrome_executable)
        .or_else(packaged_chrome_executable)
        .or_else(local_tintin_chrome_executable)
}

fn explicit_chrome_override() -> Option<PathBuf> {
    for key in ["PUFFER_CHROME", "PUFFER_CT_CHROME"] {
        let Some(value) = std::env::var_os(key) else {
            continue;
        };
        let path = PathBuf::from(value);
        if is_executable_candidate(&path) {
            return Some(path);
        }
    }
    None
}

fn cached_chrome_executable() -> Option<PathBuf> {
    executable_in_release_dir(&runtime_extract_dir()?)
}

fn packaged_chrome_executable() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let mut roots = Vec::new();
    roots.push(exe_dir.join("browser-runtimes"));
    if let Some(contents_dir) = exe_dir.parent() {
        roots.push(contents_dir.join("Resources").join("browser-runtimes"));
    }
    for root in roots {
        if let Some(path) = executable_in_release_dir(&root.join(asset_stem()?)) {
            return Some(path);
        }
    }
    None
}

fn local_tintin_chrome_executable() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    executable_in_release_dir(&home.join("chromium_tintin/src/out/Release"))
}

fn download_ct_chrome_release() -> Result<()> {
    let Some(asset) = chrome_asset_name() else {
        bail!(
            "Puffer CT Chromium release does not provide an asset for {}-{}",
            runtime_platform(),
            runtime_arch()
        );
    };
    let Some(extract_dir) = runtime_extract_dir() else {
        bail!("HOME or PUFFER_HOME is required to cache the Puffer CT Chromium runtime");
    };
    let complete_marker = extract_dir.join(".puffer-runtime-complete");
    if complete_marker.is_file() && executable_in_release_dir(&extract_dir).is_some() {
        return Ok(());
    }

    let archive_path = runtime_cache_root()
        .context("resolve Puffer CT runtime cache")?
        .join("downloads")
        .join(&asset);
    download_release_asset(&asset, &archive_path)?;
    reset_extract_dir(&extract_dir)?;
    extract_release_asset(&archive_path, &extract_dir)?;
    if executable_in_release_dir(&extract_dir).is_none() {
        bail!("Puffer CT Chromium asset `{asset}` did not contain a usable Chromium executable");
    }
    fs::write(complete_marker, asset).context("mark Puffer CT Chromium runtime complete")
}

fn download_release_asset(asset: &str, archive_path: &Path) -> Result<()> {
    if archive_path.is_file() {
        return Ok(());
    }
    let parent = archive_path
        .parent()
        .ok_or_else(|| anyhow!("release archive path has no parent"))?;
    fs::create_dir_all(parent).context("create CT runtime download directory")?;
    let repo = std::env::var("PUFFER_CT_REPO").unwrap_or_else(|_| CT_REPO.to_string());
    let tag = std::env::var("PUFFER_CT_RELEASE_TAG").unwrap_or_else(|_| CT_TAG.to_string());
    let url = format!("https://github.com/{repo}/releases/download/{tag}/{asset}");
    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .context("build CT runtime HTTP client")?;
    let mut response = client
        .get(&url)
        .send()
        .with_context(|| format!("download Puffer CT Chromium runtime from {url}"))?
        .error_for_status()
        .with_context(|| format!("download Puffer CT Chromium runtime from {url}"))?;
    let tmp_path = archive_path.with_extension("download");
    let mut out = File::create(&tmp_path).context("create temporary CT runtime archive")?;
    copy(&mut response, &mut out).context("write CT runtime archive")?;
    fs::rename(&tmp_path, archive_path).context("move CT runtime archive into cache")
}

fn extract_release_asset(archive_path: &Path, extract_dir: &Path) -> Result<()> {
    let status = if archive_path
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.ends_with(".zip"))
    {
        Command::new("unzip")
            .arg("-q")
            .arg(archive_path)
            .arg("-d")
            .arg(extract_dir)
            .status()
            .context("run unzip for CT Chromium runtime")?
    } else {
        Command::new("tar")
            .arg("-xzf")
            .arg(archive_path)
            .arg("-C")
            .arg(extract_dir)
            .status()
            .context("run tar for CT Chromium runtime")?
    };
    if !status.success() {
        bail!("extract Puffer CT Chromium runtime failed: {status}");
    }
    Ok(())
}

fn reset_extract_dir(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).context("reset CT runtime extract directory")?;
    }
    fs::create_dir_all(path).context("create CT runtime extract directory")
}

fn runtime_extract_dir() -> Option<PathBuf> {
    Some(runtime_cache_root()?.join(asset_stem()?))
}

fn runtime_cache_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("PUFFER_BROWSER_RUNTIME_DIR") {
        return Some(PathBuf::from(root));
    }
    if let Some(home) = std::env::var_os("PUFFER_HOME") {
        return Some(PathBuf::from(home).join("browser-runtimes").join("ct"));
    }
    Some(
        PathBuf::from(std::env::var_os("HOME")?)
            .join(".puffer")
            .join("browser-runtimes")
            .join("ct"),
    )
}

fn executable_in_release_dir(root: &Path) -> Option<PathBuf> {
    let candidates = if runtime_platform() == "macos" {
        vec![root.join("Chromium.app/Contents/MacOS/Chromium")]
    } else {
        vec![
            root.join("chrome"),
            root.join("chromium"),
            root.join("chrome-linux/chrome"),
            root.join("chrome-linux64/chrome"),
            root.join("Chromium/chrome"),
        ]
    };
    candidates
        .into_iter()
        .find(|path| is_executable_candidate(path))
}

fn is_executable_candidate(path: &Path) -> bool {
    path.is_file()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn terminate_profile_processes_unix(profile_dir: &Path) {
    let profile = profile_dir.to_string_lossy();
    let Ok(output) = Command::new("ps").args(["-axo", "pid=,command="]).output() else {
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pids = stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, command) = line.split_once(char::is_whitespace)?;
            let is_chrome = command.contains("Chrome")
                || command.contains("chrome")
                || command.contains("Chromium")
                || command.contains("chromium");
            let owns_profile = is_chrome
                && command.contains("--user-data-dir=")
                && command.contains(profile.as_ref());
            owns_profile.then(|| pid.to_string())
        })
        .collect::<Vec<_>>();
    if pids.is_empty() {
        return;
    }
    for pid in &pids {
        let _ = Command::new("kill").arg("-TERM").arg(pid).status();
    }
    std::thread::sleep(Duration::from_millis(250));
    for pid in pids {
        if process_is_alive(&pid) {
            let _ = Command::new("kill").arg("-KILL").arg(pid).status();
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn process_is_alive(pid: &str) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn chrome_asset_name() -> Option<String> {
    let platform = runtime_platform();
    let arch = runtime_arch();
    match platform.as_str() {
        "macos" => Some(format!("chromium-tintin-chrome-{platform}-{arch}.zip")),
        "linux" if arch == "x64" => {
            Some(format!("chromium-tintin-chrome-{platform}-{arch}.tar.gz"))
        }
        _ => None,
    }
}

fn asset_stem() -> Option<String> {
    let asset = chrome_asset_name()?;
    Some(
        asset
            .trim_end_matches(".tar.gz")
            .trim_end_matches(".zip")
            .to_string(),
    )
}

fn runtime_platform() -> String {
    if cfg!(target_os = "macos") {
        "macos".to_string()
    } else if cfg!(target_os = "linux") {
        "linux".to_string()
    } else {
        std::env::consts::OS.to_string()
    }
}

fn runtime_arch() -> String {
    match std::env::consts::ARCH {
        "aarch64" => "arm64".to_string(),
        "x86_64" => "x64".to_string(),
        other => other.to_string(),
    }
}

/// Converts a session id into a filesystem-safe browser profile directory name.
pub(super) fn safe_profile_name(session_id: &str) -> String {
    session_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::{chrome_asset_name, runtime_arch, runtime_platform};

    #[test]
    fn current_platform_maps_to_release_asset() {
        let platform = runtime_platform();
        let arch = runtime_arch();
        if platform == "macos" || (platform == "linux" && arch == "x64") {
            assert!(chrome_asset_name().is_some());
        }
    }
}
