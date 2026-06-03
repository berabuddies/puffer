//! Custom CT browser runtime discovery for managed browser sessions.

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use std::fs::{self, File};
use std::io::copy;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const CT_REPO: &str = "berabuddies/ct";
const CT_TAG: &str = "ct";

/// Returns a custom Chromium executable already present on disk.
pub(super) fn discover_chrome_executable() -> Option<PathBuf> {
    explicit_chrome_override()
        .or_else(cached_chrome_executable)
        .or_else(packaged_chrome_executable)
        .or_else(local_tintin_chrome_executable)
}

/// Returns a custom CEF runtime root already present on disk.
pub(super) fn discover_cef_root() -> Option<PathBuf> {
    explicit_cef_override()
        .or_else(cached_cef_root)
        .or_else(packaged_cef_root)
        .or_else(local_tintin_cef_root)
}

/// Returns a custom Chromium executable, downloading the CT release if needed.
pub(super) fn ensure_chrome_executable() -> Result<PathBuf> {
    if let Some(path) = discover_chrome_executable() {
        return Ok(path);
    }
    download_ct_chrome_release()?;
    discover_chrome_executable().ok_or_else(|| {
        anyhow::anyhow!("Puffer CT Chromium runtime was downloaded but no executable was found")
    })
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

fn explicit_cef_override() -> Option<PathBuf> {
    for key in ["PUFFER_CEF_PATH", "PUFFER_CEF_ROOT", "CEF_PATH"] {
        let Some(value) = std::env::var_os(key) else {
            continue;
        };
        if let Some(root) = cef_root_in_release_dir(&PathBuf::from(value)) {
            return Some(root);
        }
    }
    None
}

fn cached_chrome_executable() -> Option<PathBuf> {
    executable_in_release_dir(&chrome_runtime_extract_dir()?)
}

fn cached_cef_root() -> Option<PathBuf> {
    cef_root_in_release_dir(&cef_runtime_extract_dir()?)
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
        if let Some(path) = executable_in_release_dir(&root.join(chrome_asset_stem()?)) {
            return Some(path);
        }
    }
    None
}

fn packaged_cef_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let mut roots = Vec::new();
    roots.push(exe_dir.join("cef"));
    roots.push(exe_dir.join("browser-runtimes").join(cef_asset_stem()?));
    if let Some(contents_dir) = exe_dir.parent() {
        roots.push(contents_dir.join("Resources").join("cef"));
        roots.push(
            contents_dir
                .join("Resources")
                .join("browser-runtimes")
                .join(cef_asset_stem()?),
        );
    }
    roots
        .into_iter()
        .find_map(|root| cef_root_in_release_dir(&root))
}

fn local_tintin_chrome_executable() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let root = home.join("chromium_tintin/src/out/Release");
    executable_in_release_dir(&root)
}

fn local_tintin_cef_root() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let tintin_root = home.join("chromium_tintin");
    [
        tintin_root.join("src/out/Release_GN_arm64"),
        tintin_root.join("src/out/Release"),
    ]
    .into_iter()
    .find_map(|root| cef_root_in_release_dir(&root))
    .or_else(|| local_cef_distribution_root(&tintin_root.join("output")))
}

fn local_cef_distribution_root(output_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(output_dir).ok()?;
    entries.flatten().find_map(|entry| {
        let path = entry.path();
        if !path.is_dir()
            || !entry
                .file_name()
                .to_string_lossy()
                .starts_with("cef_binary_")
        {
            return None;
        }
        cef_root_in_release_dir(&path.join("Release"))
    })
}

fn download_ct_chrome_release() -> Result<()> {
    let Some(asset) = chrome_asset_name() else {
        bail!(
            "Puffer CT Chromium release does not provide an asset for {}-{}",
            runtime_platform(),
            runtime_arch()
        );
    };
    let Some(extract_dir) = chrome_runtime_extract_dir() else {
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
        .ok_or_else(|| anyhow::anyhow!("release archive path has no parent"))?;
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
        .with_context(|| format!("download Puffer CT browser runtime from {url}"))?
        .error_for_status()
        .with_context(|| format!("download Puffer CT browser runtime from {url}"))?;
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
            .context("run unzip for CT browser runtime")?
    } else {
        Command::new("tar")
            .arg("-xzf")
            .arg(archive_path)
            .arg("-C")
            .arg(extract_dir)
            .status()
            .context("run tar for CT browser runtime")?
    };
    if !status.success() {
        bail!("extract Puffer CT browser runtime failed: {status}");
    }
    Ok(())
}

fn reset_extract_dir(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).context("reset CT runtime extract directory")?;
    }
    fs::create_dir_all(path).context("create CT runtime extract directory")
}

fn chrome_runtime_extract_dir() -> Option<PathBuf> {
    Some(runtime_cache_root()?.join(chrome_asset_stem()?))
}

fn cef_runtime_extract_dir() -> Option<PathBuf> {
    Some(runtime_cache_root()?.join(cef_asset_stem()?))
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
    let platform = runtime_platform();
    let candidates = if platform == "macos" {
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

fn cef_root_in_release_dir(root: &Path) -> Option<PathBuf> {
    for candidate in cef_root_candidates(root.to_path_buf()) {
        if cef_framework_binary(&candidate).is_file() {
            return Some(candidate);
        }
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return None;
    };
    entries.flatten().find_map(|entry| {
        let path = entry.path();
        path.is_dir()
            .then(|| {
                cef_root_candidates(path)
                    .into_iter()
                    .find(|candidate| cef_framework_binary(candidate).is_file())
            })
            .flatten()
    })
}

fn cef_root_candidates(root: PathBuf) -> Vec<PathBuf> {
    vec![
        root.clone(),
        root.join("Release"),
        root.join("Release_GN_arm64"),
    ]
}

fn cef_framework_binary(root: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        root.join("Chromium Embedded Framework.framework")
            .join("Chromium Embedded Framework")
    }
    #[cfg(target_os = "windows")]
    {
        root.join("libcef.dll")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        root.join("libcef.so")
    }
}

fn is_executable_candidate(path: &Path) -> bool {
    path.is_file()
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

fn cef_asset_name() -> Option<String> {
    let platform = runtime_platform();
    let arch = runtime_arch();
    match platform.as_str() {
        "macos" | "linux" => Some(format!("puffer-cef-{platform}-{arch}.tar.gz")),
        _ => None,
    }
}

fn chrome_asset_stem() -> Option<String> {
    let asset = chrome_asset_name()?;
    Some(asset_stem(&asset))
}

fn cef_asset_stem() -> Option<String> {
    let asset = cef_asset_name()?;
    Some(asset_stem(&asset))
}

fn asset_stem(asset: &str) -> String {
    asset
        .trim_end_matches(".tar.gz")
        .trim_end_matches(".zip")
        .to_string()
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

#[cfg(test)]
mod tests {
    use super::{cef_asset_name, chrome_asset_name, runtime_arch, runtime_platform};

    #[test]
    fn current_platform_maps_to_release_asset() {
        let platform = runtime_platform();
        let arch = runtime_arch();
        if platform == "macos" || (platform == "linux" && arch == "x64") {
            assert!(chrome_asset_name().is_some());
            assert!(cef_asset_name().is_some());
        }
    }
}
