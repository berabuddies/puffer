//! Native Chromium Embedded Framework host commands for Puffer Desktop.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::Window;

const DEFAULT_REMOTE_DEBUGGING_PORT: u16 = 9333;

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

/// Initializes native CEF before Tauri enters the AppKit event loop.
pub(crate) fn warm_up_native() -> Result<()> {
    initialize_native_once().map(|_| ())
}

/// Returns native CEF availability for the current desktop process.
#[tauri::command]
pub(crate) fn browser_cef_native_status() -> Value {
    let discovered_runtime = CefRuntime::discover();
    let initialization = CEF_INITIALIZATION.get();
    let initialized_runtime = initialization.and_then(|value| value.as_ref().ok());
    let discovery_error = discovered_runtime
        .as_ref()
        .err()
        .map(|error| error.to_string());
    let diagnostic_runtime = initialized_runtime.or_else(|| discovered_runtime.as_ref().ok());
    let initialization_error = initialization.and_then(|value| value.as_ref().err().cloned());
    let not_initialized_error = (initialization.is_none()
        && discovery_error.is_none()
        && native_enabled())
    .then_some("native CEF was not initialized before the desktop event loop started".to_string());
    json!(CefNativeStatus {
        available: initialized_runtime.is_some(),
        active: initialized_runtime.is_some(),
        root: diagnostic_runtime.map(|value| display_path(&value.root)),
        helper: diagnostic_runtime.map(|value| display_path(&value.helper)),
        remote_debugging_port: remote_debugging_port(),
        build_enabled: native_enabled(),
        error: initialization_error
            .or(discovery_error)
            .or(not_initialized_error),
    })
}

/// Opens or focuses a native CEF browser for a Browser tab.
#[tauri::command]
pub(crate) fn browser_cef_native_open(
    window: Window,
    session_id: String,
    url: Option<String>,
    rect: CefBrowserRect,
) -> Result<Value, String> {
    let state = with_native_browser(&session_id, |session_id| {
        native_open(
            session_id,
            window_handle(&window)?,
            &url.unwrap_or_else(|| "about:blank".to_string()),
            rect,
        )
    })?;
    Ok(state)
}

/// Resizes the native CEF browser for a Browser tab.
#[tauri::command]
pub(crate) fn browser_cef_native_resize(
    window: Window,
    session_id: String,
    rect: CefBrowserRect,
) -> Result<Value, String> {
    let state = with_native_browser(&session_id, |session_id| {
        native_resize(session_id, window_handle(&window)?, rect)
    })?;
    Ok(state)
}

/// Navigates a native CEF browser.
#[tauri::command]
pub(crate) fn browser_cef_native_navigate(
    session_id: String,
    url: String,
) -> Result<Value, String> {
    ensure_native_initialized()
        .and_then(|()| native_navigate(&session_id, &url))
        .and_then(|()| native_state(&session_id))
        .map_err(|error| error.to_string())
}

/// Reloads a native CEF browser.
#[tauri::command]
pub(crate) fn browser_cef_native_reload(session_id: String) -> Result<Value, String> {
    ensure_native_initialized()
        .and_then(|()| native_reload(&session_id))
        .and_then(|()| native_state(&session_id))
        .map_err(|error| error.to_string())
}

/// Moves a native CEF browser backward or forward in history.
#[tauri::command]
pub(crate) fn browser_cef_native_history(
    session_id: String,
    direction: String,
) -> Result<Value, String> {
    let direction = if direction.eq_ignore_ascii_case("back") {
        -1
    } else {
        1
    };
    ensure_native_initialized()
        .and_then(|()| native_history(&session_id, direction))
        .and_then(|()| native_state(&session_id))
        .map_err(|error| error.to_string())
}

/// Closes a native CEF browser.
#[tauri::command]
pub(crate) fn browser_cef_native_close(session_id: String) -> Result<Value, String> {
    ensure_native_initialized()
        .and_then(|()| native_close(&session_id))
        .map(|()| json!({ "ok": true }))
        .map_err(|error| error.to_string())
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
    match CEF_INITIALIZATION.get() {
        Some(Ok(_)) => Ok(()),
        Some(Err(error)) => bail!("{error}"),
        None => bail!("native CEF was not initialized before the desktop event loop started"),
    }
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
    fn discover() -> Result<Self> {
        #[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
        {
            let root = runtime_root_from_env()
                .or_else(runtime_root_from_bundle)
                .or_else(runtime_root_from_compiled_env)
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
            bail!("native CEF bridge was not compiled for this desktop build")
        }
    }
}

fn initialize_native_once() -> Result<&'static CefRuntime> {
    let initialization = CEF_INITIALIZATION.get_or_init(|| {
        let runtime = CefRuntime::discover().map_err(|error| error.to_string())?;
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
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set"))?;
    let root = home
        .join("Library/Application Support/Puffer")
        .join("cef-profile");
    std::fs::create_dir_all(root.join("Default")).context("create CEF cache directory")?;
    Ok(root)
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
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
        pub(super) fn puffer_cef_state_json(session_id: *const c_char) -> *mut c_char;
        pub(super) fn puffer_cef_free_string(value: *mut c_char);
    }
}

#[cfg(all(target_os = "macos", puffer_desktop_cef_native))]
fn native_initialize(runtime: &CefRuntime) -> Result<()> {
    let root = cstring_path(&runtime.root)?;
    let helper = cstring_path(&runtime.helper)?;
    let cache = cstring_path(&cache_root()?)?;
    let mut error = ErrorBuffer::new();
    let ok = unsafe {
        ffi::puffer_cef_initialize(
            root.as_ptr(),
            helper.as_ptr(),
            cache.as_ptr(),
            i32::from(remote_debugging_port()),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    error.result(ok, "initialize native CEF")
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
    fn native_status_is_unavailable_before_setup_initialization() {
        if CEF_INITIALIZATION.get().is_some() {
            return;
        }
        let status = browser_cef_native_status();
        assert_eq!(status["available"], serde_json::json!(false));
        assert_eq!(status["active"], serde_json::json!(false));
    }
}
