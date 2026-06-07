//! Native Chromium Embedded Framework host commands for Puffer Desktop.

use anyhow::{anyhow, bail, Context, Result};
use crate::browser_debug::{cef_log, cef_result, cef_state_summary};
#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
use puffer_config::{stage_builtin_captcha_extension, CaptchaExtensionSeed, ConfigPaths};
#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
use puffer_secrets::SecretVault;
#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::ffi::{c_char, c_void, CStr};
#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
use std::ffi::CString;
#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
use std::fs::{self, File};
#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
use std::io::copy;
use std::path::{Path, PathBuf};
#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
use std::process::Command;
use std::sync::OnceLock;
#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
use std::time::Duration;
use tauri::Window;

const DEFAULT_REMOTE_DEBUGGING_PORT: u16 = 9333;
#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
const CT_REPO: &str = "berabuddies/ct";
#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
const CT_TAG: &str = "ct";

/// Native CEF runtime status returned to the Svelte shell.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CefNativeStatus {
    available: bool,
    active: bool,
    root: Option<String>,
    helper: Option<String>,
    remote_debugging_port: u16,
    build_enabled: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CefBrowserRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone)]
struct CefRuntime {
    root: PathBuf,
    helper: PathBuf,
}

static CEF_INITIALIZATION: OnceLock<Result<CefRuntime, String>> = OnceLock::new();

/// Returns native CEF availability for the current desktop process.
#[tauri::command]
pub(crate) fn browser_cef_native_status() -> Value {
    let discovered_runtime = CefRuntime::discover(false);
    let initialization = CEF_INITIALIZATION.get();
    let initialized_runtime = initialization.and_then(|value| value.as_ref().ok());
    let discovery_error = discovered_runtime
        .as_ref()
        .err()
        .map(|error| error.to_string());
    let diagnostic_runtime = initialized_runtime.or_else(|| discovered_runtime.as_ref().ok());
    let initialization_error = initialization.and_then(|value| value.as_ref().err().cloned());
    let available = initialization_error.is_none() && diagnostic_runtime.is_some();
    let status = json!(CefNativeStatus {
        available,
        active: initialized_runtime.is_some(),
        root: diagnostic_runtime.map(|value| display_path(&value.root)),
        helper: diagnostic_runtime.map(|value| display_path(&value.helper)),
        remote_debugging_port: remote_debugging_port(),
        build_enabled: native_enabled(),
        error: initialization_error.or(discovery_error),
    });
    cef_log("status", cef_state_summary(&status));
    status
}

/// Opens or focuses a native CEF browser for a Browser tab.
#[tauri::command]
pub(crate) fn browser_cef_native_open(
    window: Window,
    session_id: String,
    url: Option<String>,
    rect: CefBrowserRect,
) -> Result<Value, String> {
    let requested_url = url.unwrap_or_else(|| "about:blank".to_string());
    cef_log(
        "open begin",
        format!(
            "session_id={} url={} rect={}",
            session_id,
            requested_url,
            rect_summary(rect)
        ),
    );
    let state = with_native_browser(&session_id, |session_id| {
        native_open(session_id, window_handle(&window)?, &requested_url, rect)
    });
    cef_result("open", &session_id, &state);
    state
}

/// Resizes the native CEF browser for a Browser tab.
#[tauri::command]
pub(crate) fn browser_cef_native_resize(
    window: Window,
    session_id: String,
    rect: CefBrowserRect,
) -> Result<Value, String> {
    cef_log(
        "resize begin",
        format!("session_id={} rect={}", session_id, rect_summary(rect)),
    );
    let state = with_native_browser(&session_id, |session_id| {
        native_resize(session_id, window_handle(&window)?, rect)
    });
    cef_result("resize", &session_id, &state);
    state
}

/// Navigates a native CEF browser.
#[tauri::command]
pub(crate) fn browser_cef_native_navigate(
    session_id: String,
    url: String,
) -> Result<Value, String> {
    cef_log(
        "navigate begin",
        format!("session_id={} url={}", session_id, url),
    );
    let state = ensure_native_initialized()
        .and_then(|()| native_navigate(&session_id, &url))
        .and_then(|()| native_state(&session_id))
        .map_err(|error| error.to_string());
    cef_result("navigate", &session_id, &state);
    state
}

/// Returns the last known state for a native CEF browser.
#[tauri::command]
pub(crate) fn browser_cef_native_state(session_id: String) -> Result<Value, String> {
    let state = match CEF_INITIALIZATION.get() {
        Some(Ok(_)) => native_state(&session_id).map_err(|error| error.to_string()),
        Some(Err(error)) => Err(error.clone()),
        None => Ok(native_disconnected_state()),
    };
    if state.as_ref().is_err() {
        cef_result("state", &session_id, &state);
    }
    state
}

/// Reloads a native CEF browser.
#[tauri::command]
pub(crate) fn browser_cef_native_reload(session_id: String) -> Result<Value, String> {
    cef_log("reload begin", format!("session_id={}", session_id));
    let state = ensure_native_initialized()
        .and_then(|()| native_reload(&session_id))
        .and_then(|()| native_state(&session_id))
        .map_err(|error| error.to_string());
    cef_result("reload", &session_id, &state);
    state
}

/// Moves a native CEF browser backward or forward in history.
#[tauri::command]
pub(crate) fn browser_cef_native_history(
    session_id: String,
    direction: String,
) -> Result<Value, String> {
    cef_log(
        "history begin",
        format!("session_id={} direction={}", session_id, direction),
    );
    let direction = if direction.eq_ignore_ascii_case("back") {
        -1
    } else {
        1
    };
    let state = ensure_native_initialized()
        .and_then(|()| native_history(&session_id, direction))
        .and_then(|()| native_state(&session_id))
        .map_err(|error| error.to_string());
    cef_result("history", &session_id, &state);
    state
}

/// Closes a native CEF browser.
#[tauri::command]
pub(crate) fn browser_cef_native_close(session_id: String) -> Result<Value, String> {
    cef_log("close begin", format!("session_id={}", session_id));
    let state = match CEF_INITIALIZATION.get() {
        Some(Ok(_)) => native_close(&session_id)
            .map(|()| json!({ "ok": true }))
            .map_err(|error| error.to_string()),
        Some(Err(error)) => Err(error.clone()),
        None => Ok(json!({ "ok": true })),
    };
    cef_result("close", &session_id, &state);
    state
}

/// Hides a native CEF browser without closing its renderer process.
#[tauri::command]
pub(crate) fn browser_cef_native_hide(session_id: String) -> Result<Value, String> {
    cef_log("hide begin", format!("session_id={}", session_id));
    let state = match CEF_INITIALIZATION.get() {
        Some(Ok(_)) => native_hide(&session_id)
            .map(|()| json!({ "ok": true }))
            .map_err(|error| error.to_string()),
        Some(Err(error)) => Err(error.clone()),
        None => Ok(json!({ "ok": true })),
    };
    cef_result("hide", &session_id, &state);
    state
}

fn rect_summary(rect: CefBrowserRect) -> String {
    format!(
        "x={:.1},y={:.1},w={:.1},h={:.1}",
        rect.x, rect.y, rect.width, rect.height
    )
}

fn with_native_browser<F>(session_id: &str, action: F) -> Result<Value, String>
where
    F: FnOnce(&str) -> Result<()>,
{
    ensure_native_initialized()
        .and_then(|()| action(session_id))
        .map_err(|error| error.to_string())?;
    native_state(session_id).map_err(|error| error.to_string())
}

fn ensure_native_initialized() -> Result<()> {
    initialize_native_once().map(|_| ())
}

fn native_disconnected_state() -> Value {
    json!({
        "connected": false,
        "url": "about:blank",
        "title": "",
        "loading": false,
        "error": null,
    })
}

fn window_handle(window: &Window) -> Result<*mut c_void> {
    #[cfg(target_os = "macos")]
    {
        window
            .ns_window()
            .map_err(|error| anyhow!("read Tauri NSWindow handle: {error}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        bail!("native CEF embedding is only implemented on macOS")
    }
}

impl CefRuntime {
    fn discover(allow_download: bool) -> Result<Self> {
        #[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
        {
            let root = runtime_root_from_env()
                .or_else(runtime_root_from_bundle)
                .or_else(runtime_root_from_compiled_env)
                .or_else(runtime_root_from_ct_cache)
                .or_else(|| allow_download.then(ensure_ct_cef_root).and_then(Result::ok))
                .ok_or_else(|| anyhow!("CEF runtime root was not found"))?;
            let helper = runtime_helper_from_env()
                .or_else(|| helper_for_root(&root))
                .or_else(|| {
                    option_env!("PUFFER_DESKTOP_CEF_HELPER")
                        .map(PathBuf::from)
                        .filter(|path| path.is_file())
                })
                .ok_or_else(|| anyhow!("CEF helper executable was not found"))?;
            Ok(Self { root, helper })
        }
        #[cfg(not(all(target_os = "macos", puffer_desktop_cef_native)))]
        {
            let _ = allow_download;
            bail!("native CEF bridge was not compiled for this desktop build")
        }
    }
}

fn initialize_native_once() -> Result<&'static CefRuntime> {
    let initialization = CEF_INITIALIZATION.get_or_init(|| {
        let runtime = CefRuntime::discover(true).map_err(|error| error.to_string())?;
        native_initialize(&runtime).map_err(|error| error.to_string())?;
        Ok(runtime)
    });
    match initialization {
        Ok(runtime) => Ok(runtime),
        Err(error) => bail!("{error}"),
    }
}

fn runtime_root_from_env() -> Option<PathBuf> {
    for key in ["PUFFER_CEF_PATH", "PUFFER_CEF_ROOT", "CEF_PATH"] {
        let Some(value) = std::env::var_os(key) else {
            continue;
        };
        for candidate in root_candidates(PathBuf::from(value)) {
            if cef_framework_binary(&candidate).is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn runtime_root_from_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let mut roots = vec![exe_dir.to_path_buf()];
    if let Some(contents_dir) = exe_dir.parent() {
        roots.push(contents_dir.join("Frameworks"));
        roots.push(contents_dir.join("Resources").join("cef"));
    }
    for root in roots {
        for candidate in root_candidates(root) {
            if cef_framework_binary(&candidate).is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn runtime_root_from_compiled_env() -> Option<PathBuf> {
    let root = option_env!("PUFFER_DESKTOP_CEF_ROOT").map(PathBuf::from)?;
    root_candidates(root)
        .into_iter()
        .find(|candidate| cef_framework_binary(candidate).is_file())
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn runtime_root_from_ct_cache() -> Option<PathBuf> {
    let root = cef_runtime_extract_dir()?;
    cef_root_in_release_dir(&root)
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn ensure_ct_cef_root() -> Result<PathBuf> {
    if let Some(root) = runtime_root_from_ct_cache() {
        return Ok(root);
    }
    download_ct_cef_release()?;
    runtime_root_from_ct_cache()
        .ok_or_else(|| anyhow!("Puffer CT CEF runtime was downloaded but no root was found"))
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn download_ct_cef_release() -> Result<()> {
    let Some(asset) = cef_asset_name() else {
        bail!(
            "Puffer CT CEF release does not provide an asset for {}-{}",
            runtime_platform(),
            runtime_arch()
        );
    };
    let Some(extract_dir) = cef_runtime_extract_dir() else {
        bail!("HOME or PUFFER_HOME is required to cache the Puffer CT CEF runtime");
    };
    let complete_marker = extract_dir.join(".puffer-runtime-complete");
    if complete_marker.is_file() && cef_root_in_release_dir(&extract_dir).is_some() {
        return Ok(());
    }
    let archive_path = runtime_cache_root()
        .context("resolve Puffer CT runtime cache")?
        .join("downloads")
        .join(&asset);
    download_release_asset(&asset, &archive_path)?;
    reset_extract_dir(&extract_dir)?;
    extract_release_asset(&archive_path, &extract_dir)?;
    if cef_root_in_release_dir(&extract_dir).is_none() {
        bail!("Puffer CT CEF asset `{asset}` did not contain a usable CEF runtime");
    }
    fs::write(complete_marker, asset).context("mark Puffer CT CEF runtime complete")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
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
        .with_context(|| format!("download Puffer CT CEF runtime from {url}"))?
        .error_for_status()
        .with_context(|| format!("download Puffer CT CEF runtime from {url}"))?;
    let tmp_path = archive_path.with_extension("download");
    let mut out = File::create(&tmp_path).context("create temporary CT runtime archive")?;
    copy(&mut response, &mut out).context("write CT runtime archive")?;
    fs::rename(&tmp_path, archive_path).context("move CT runtime archive into cache")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn extract_release_asset(archive_path: &Path, extract_dir: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive_path)
        .arg("-C")
        .arg(extract_dir)
        .status()
        .context("run tar for CT CEF runtime")?;
    if !status.success() {
        bail!("extract Puffer CT CEF runtime failed: {status}");
    }
    Ok(())
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn reset_extract_dir(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).context("reset CT runtime extract directory")?;
    }
    fs::create_dir_all(path).context("create CT runtime extract directory")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn cef_runtime_extract_dir() -> Option<PathBuf> {
    Some(runtime_cache_root()?.join(cef_asset_stem()?))
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
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

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn cef_root_in_release_dir(root: &Path) -> Option<PathBuf> {
    for candidate in root_candidates(root.to_path_buf()) {
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
                root_candidates(path)
                    .into_iter()
                    .find(|candidate| cef_framework_binary(candidate).is_file())
            })
            .flatten()
    })
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn cef_asset_name() -> Option<String> {
    let platform = runtime_platform();
    let arch = runtime_arch();
    match platform.as_str() {
        "macos" | "linux" => Some(format!("puffer-cef-{platform}-{arch}.tar.gz")),
        _ => None,
    }
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn cef_asset_stem() -> Option<String> {
    let asset = cef_asset_name()?;
    Some(asset.trim_end_matches(".tar.gz").to_string())
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn runtime_platform() -> String {
    if cfg!(target_os = "macos") {
        "macos".to_string()
    } else if cfg!(target_os = "linux") {
        "linux".to_string()
    } else {
        std::env::consts::OS.to_string()
    }
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn runtime_arch() -> String {
    match std::env::consts::ARCH {
        "aarch64" => "arm64".to_string(),
        "x86_64" => "x64".to_string(),
        other => other.to_string(),
    }
}

fn runtime_helper_from_env() -> Option<PathBuf> {
    std::env::var_os("PUFFER_CEF_HELPER")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

fn root_candidates(root: PathBuf) -> Vec<PathBuf> {
    vec![
        root.clone(),
        root.join("Release_GN_arm64"),
        root.join("Release"),
    ]
}

fn cef_framework_binary(root: &Path) -> PathBuf {
    root.join("Chromium Embedded Framework.framework")
        .join("Chromium Embedded Framework")
}

fn helper_for_root(root: &Path) -> Option<PathBuf> {
    let helper = root.join("cefsimple Helper.app/Contents/MacOS/cefsimple Helper");
    helper.is_file().then_some(helper)
}

fn remote_debugging_port() -> u16 {
    std::env::var("PUFFER_CEF_REMOTE_DEBUGGING_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port >= 1024)
        .unwrap_or(DEFAULT_REMOTE_DEBUGGING_PORT)
}

fn cache_root() -> Result<PathBuf> {
    let root = if let Some(root) = std::env::var_os("PUFFER_CEF_PROFILE_DIR") {
        PathBuf::from(root)
    } else {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("HOME is not set"))?;
        home.join("Library/Application Support/Puffer")
            .join("cef-profile")
    };
    std::fs::create_dir_all(root.join("Default")).context("create CEF cache directory")?;
    Ok(root)
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

fn push_extension_dir(extension_dirs: &mut Vec<PathBuf>, path: PathBuf) {
    if path.join("manifest.json").is_file() {
        extension_dirs.push(path);
    }
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn reveal_secret_value(paths: &ConfigPaths, secret_id: &str) -> Option<String> {
    let store_path = SecretVault::default_path(&paths.user_config_dir);
    let vault = SecretVault::open(store_path).ok()?;
    match vault.reveal(secret_id) {
        Ok(secret) => Some(secret.value),
        Err(error) => {
            eprintln!(
                "puffer browser: captcha API key `{secret_id}` could not be revealed: {error}"
            );
            None
        }
    }
}

fn dedupe_extension_dirs(extension_dirs: &mut Vec<PathBuf>) {
    let mut seen = BTreeSet::new();
    extension_dirs.retain(|path| seen.insert(path.clone()));
}

fn native_enabled() -> bool {
    cfg!(all(target_os = "macos", puffer_desktop_cef_native))
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
mod ffi {
    use super::{c_char, c_void};

    extern "C" {
        pub(super) fn puffer_cef_initialize(
            runtime_root: *const c_char,
            helper_path: *const c_char,
            cache_path: *const c_char,
            extension_dirs: *const c_char,
            remote_debugging_port: i32,
            error: *mut c_char,
            error_len: usize,
        ) -> i32;
        pub(super) fn puffer_cef_open(
            session_id: *const c_char,
            ns_window: *mut c_void,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
            url: *const c_char,
            error: *mut c_char,
            error_len: usize,
        ) -> i32;
        pub(super) fn puffer_cef_resize(
            session_id: *const c_char,
            ns_window: *mut c_void,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
            error: *mut c_char,
            error_len: usize,
        ) -> i32;
        pub(super) fn puffer_cef_navigate(
            session_id: *const c_char,
            url: *const c_char,
            error: *mut c_char,
            error_len: usize,
        ) -> i32;
        pub(super) fn puffer_cef_reload(
            session_id: *const c_char,
            error: *mut c_char,
            error_len: usize,
        ) -> i32;
        pub(super) fn puffer_cef_history(
            session_id: *const c_char,
            direction: i32,
            error: *mut c_char,
            error_len: usize,
        ) -> i32;
        pub(super) fn puffer_cef_close(
            session_id: *const c_char,
            error: *mut c_char,
            error_len: usize,
        ) -> i32;
        pub(super) fn puffer_cef_hide(
            session_id: *const c_char,
            error: *mut c_char,
            error_len: usize,
        ) -> i32;
        pub(super) fn puffer_cef_state_json(session_id: *const c_char) -> *mut c_char;
        pub(super) fn puffer_cef_free_string(value: *mut c_char);
    }
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn native_initialize(runtime: &CefRuntime) -> Result<()> {
    let root = cstring_path(&runtime.root)?;
    let helper = cstring_path(&runtime.helper)?;
    let cache = cstring_path(&cache_root()?)?;
    let extension_dirs = CString::new(native_cef_extension_dirs()?.as_bytes())
        .context("encode native CEF extension directories")?;
    let mut error = ErrorBuffer::new();
    let ok = unsafe {
        ffi::puffer_cef_initialize(
            root.as_ptr(),
            helper.as_ptr(),
            cache.as_ptr(),
            extension_dirs.as_ptr(),
            i32::from(remote_debugging_port()),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    error.result(ok, "initialize native CEF")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn native_cef_extension_dirs() -> Result<String> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let paths = ConfigPaths::discover(&cwd);
    let config = puffer_config::load_config(&paths).context("load browser extension config")?;
    let browser = config.browser;
    if !browser.extensions_enabled {
        return Ok(String::new());
    }
    let mut dirs = Vec::new();
    for extension in browser.extensions.iter().filter(|extension| extension.enabled) {
        push_extension_dir(&mut dirs, PathBuf::from(&extension.path));
    }
    if browser.captcha.enabled {
        if let Some(solver) = puffer_config::builtin_captcha_solvers()
            .iter()
            .find(|solver| solver.id == browser.captcha.selected_solver)
        {
            let configured = browser.captcha.solvers.get(solver.id);
            let enabled = configured.map(|item| item.enabled).unwrap_or(true);
            if enabled {
                let source_dir = paths.builtin_resources_dir.join(solver.extension_path);
                let mut extension_dir = source_dir.clone();
                if let Some(secret_id) = configured.and_then(|item| item.api_key_secret_id.as_ref())
                {
                    if let Some(api_key) = reveal_secret_value(&paths, secret_id) {
                        let base_url = configured
                            .and_then(|item| item.base_url.clone())
                            .unwrap_or_else(|| solver.default_base_url.to_string());
                        let seed = CaptchaExtensionSeed::new(solver.id, api_key, base_url);
                        extension_dir = stage_builtin_captcha_extension(
                            &source_dir,
                            &paths.user_config_dir.join("browser-extension-stage"),
                            &seed,
                        )?;
                    }
                }
                push_extension_dir(&mut dirs, extension_dir);
            }
        }
    }
    dedupe_extension_dirs(&mut dirs);
    Ok(dirs
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(","))
}

#[cfg(not(all(target_os = "macos", puffer_desktop_cef_native)))]
fn native_initialize(_runtime: &CefRuntime) -> Result<()> {
    bail!("native CEF bridge was not compiled for this desktop build")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn native_open(
    session_id: &str,
    ns_window: *mut c_void,
    url: &str,
    rect: CefBrowserRect,
) -> Result<()> {
    let session_id = CString::new(session_id).context("encode CEF session id")?;
    let url = CString::new(url).context("encode CEF URL")?;
    let mut error = ErrorBuffer::new();
    let ok = unsafe {
        ffi::puffer_cef_open(
            session_id.as_ptr(),
            ns_window,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            url.as_ptr(),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    error.result(ok, "open native CEF browser")
}

#[cfg(not(all(target_os = "macos", puffer_desktop_cef_native)))]
fn native_open(
    _session_id: &str,
    _ns_window: *mut c_void,
    _url: &str,
    _rect: CefBrowserRect,
) -> Result<()> {
    bail!("native CEF bridge was not compiled for this desktop build")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn native_resize(session_id: &str, ns_window: *mut c_void, rect: CefBrowserRect) -> Result<()> {
    let session_id = CString::new(session_id).context("encode CEF session id")?;
    let mut error = ErrorBuffer::new();
    let ok = unsafe {
        ffi::puffer_cef_resize(
            session_id.as_ptr(),
            ns_window,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    error.result(ok, "resize native CEF browser")
}

#[cfg(not(all(target_os = "macos", puffer_desktop_cef_native)))]
fn native_resize(_session_id: &str, _ns_window: *mut c_void, _rect: CefBrowserRect) -> Result<()> {
    bail!("native CEF bridge was not compiled for this desktop build")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn native_navigate(session_id: &str, url: &str) -> Result<()> {
    let session_id = CString::new(session_id).context("encode CEF session id")?;
    let url = CString::new(url).context("encode CEF URL")?;
    let mut error = ErrorBuffer::new();
    let ok = unsafe {
        ffi::puffer_cef_navigate(
            session_id.as_ptr(),
            url.as_ptr(),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    error.result(ok, "navigate native CEF browser")
}

#[cfg(not(all(target_os = "macos", puffer_desktop_cef_native)))]
fn native_navigate(_session_id: &str, _url: &str) -> Result<()> {
    bail!("native CEF bridge was not compiled for this desktop build")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn native_reload(session_id: &str) -> Result<()> {
    let session_id = CString::new(session_id).context("encode CEF session id")?;
    let mut error = ErrorBuffer::new();
    let ok =
        unsafe { ffi::puffer_cef_reload(session_id.as_ptr(), error.as_mut_ptr(), error.len()) };
    error.result(ok, "reload native CEF browser")
}

#[cfg(not(all(target_os = "macos", puffer_desktop_cef_native)))]
fn native_reload(_session_id: &str) -> Result<()> {
    bail!("native CEF bridge was not compiled for this desktop build")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn native_history(session_id: &str, direction: i32) -> Result<()> {
    let session_id = CString::new(session_id).context("encode CEF session id")?;
    let mut error = ErrorBuffer::new();
    let ok = unsafe {
        ffi::puffer_cef_history(
            session_id.as_ptr(),
            direction,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    error.result(ok, "move native CEF history")
}

#[cfg(not(all(target_os = "macos", puffer_desktop_cef_native)))]
fn native_history(_session_id: &str, _direction: i32) -> Result<()> {
    bail!("native CEF bridge was not compiled for this desktop build")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn native_close(session_id: &str) -> Result<()> {
    let session_id = CString::new(session_id).context("encode CEF session id")?;
    let mut error = ErrorBuffer::new();
    let ok = unsafe { ffi::puffer_cef_close(session_id.as_ptr(), error.as_mut_ptr(), error.len()) };
    error.result(ok, "close native CEF browser")
}

#[cfg(not(all(target_os = "macos", puffer_desktop_cef_native)))]
fn native_close(_session_id: &str) -> Result<()> {
    bail!("native CEF bridge was not compiled for this desktop build")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn native_hide(session_id: &str) -> Result<()> {
    let session_id = CString::new(session_id).context("encode CEF session id")?;
    let mut error = ErrorBuffer::new();
    let ok = unsafe { ffi::puffer_cef_hide(session_id.as_ptr(), error.as_mut_ptr(), error.len()) };
    error.result(ok, "hide native CEF browser")
}

#[cfg(not(all(target_os = "macos", puffer_desktop_cef_native)))]
fn native_hide(_session_id: &str) -> Result<()> {
    bail!("native CEF bridge was not compiled for this desktop build")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn native_state(session_id: &str) -> Result<Value> {
    let session_id = CString::new(session_id).context("encode CEF session id")?;
    let raw = unsafe { ffi::puffer_cef_state_json(session_id.as_ptr()) };
    if raw.is_null() {
        bail!("CEF state allocation failed");
    }
    let text = unsafe { CStr::from_ptr(raw).to_string_lossy().to_string() };
    unsafe { ffi::puffer_cef_free_string(raw) };
    serde_json::from_str(&text).context("parse native CEF state")
}

#[cfg(not(all(target_os = "macos", puffer_desktop_cef_native)))]
fn native_state(_session_id: &str) -> Result<Value> {
    bail!("native CEF bridge was not compiled for this desktop build")
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn cstring_path(path: &Path) -> Result<CString> {
    CString::new(path.to_string_lossy().as_bytes()).context("encode CEF path")
}

struct ErrorBuffer {
    bytes: Vec<c_char>,
}

impl ErrorBuffer {
    fn new() -> Self {
        Self {
            bytes: vec![0; 2048],
        }
    }

    fn as_mut_ptr(&mut self) -> *mut c_char {
        self.bytes.as_mut_ptr()
    }

    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn message(&self) -> String {
        unsafe { CStr::from_ptr(self.bytes.as_ptr()) }
            .to_string_lossy()
            .trim()
            .to_string()
    }

    fn result(self, ok: i32, context: &str) -> Result<()> {
        if ok != 0 {
            return Ok(());
        }
        let message = self.message();
        if message.is_empty() {
            bail!("{context} failed");
        }
        bail!("{context}: {message}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_status_does_not_treat_lazy_initialization_as_error() {
        if CEF_INITIALIZATION.get().is_some() {
            return;
        }
        let status = browser_cef_native_status();
        assert_eq!(status["active"], serde_json::json!(false));
        assert_ne!(
            status["error"],
            serde_json::json!(
                "native CEF was not initialized before the desktop event loop started"
            )
        );
    }

    #[test]
    fn native_state_before_initialization_is_disconnected() {
        if CEF_INITIALIZATION.get().is_some() {
            return;
        }
        let state = browser_cef_native_state("tab-1".to_string()).unwrap();
        assert_eq!(state["connected"], serde_json::json!(false));
        assert_eq!(state["url"], serde_json::json!("about:blank"));
    }
}
