//! CEF runtime discovery for the desktop Browser backend.

use serde::Serialize;
use std::path::{Path, PathBuf};

use super::chrome::resolve_chrome_executable;
use super::ct_runtime;

const CEF_RENDERER: &str = "cef";
const SCREENCAST_RENDERER: &str = "screencast";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserBackendStatus {
    preferred_renderer: String,
    active_renderer: String,
    fallback_reason: Option<String>,
    cef: CefRuntimeStatus,
    screencast: ScreencastRuntimeStatus,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CefRuntimeStatus {
    available: bool,
    root: Option<String>,
    framework_path: Option<String>,
    missing: Vec<String>,
    tintin_chromium: TintinChromiumStatus,
    build_hint: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TintinChromiumStatus {
    executable: Option<String>,
    app_bundle: Option<String>,
    is_cef_runtime: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScreencastRuntimeStatus {
    chromium_executable: Option<String>,
}

/// Returns the current browser backend capability and fallback status.
pub(crate) fn backend_status(preferred_renderer: &str) -> serde_json::Value {
    let preferred_renderer = normalize_renderer(preferred_renderer);
    let cef = cef_runtime_status();
    let screencast = ScreencastRuntimeStatus {
        chromium_executable: resolve_chrome_executable().map(display_path),
    };
    let fallback_reason = if preferred_renderer == CEF_RENDERER {
        Some(cef_fallback_reason(&cef))
    } else {
        None
    };
    let active_renderer = SCREENCAST_RENDERER;
    serde_json::to_value(BrowserBackendStatus {
        preferred_renderer: preferred_renderer.to_string(),
        active_renderer: active_renderer.to_string(),
        fallback_reason,
        cef,
        screencast,
    })
    .unwrap_or_else(|_| serde_json::json!({}))
}

fn normalize_renderer(value: &str) -> &str {
    if value.eq_ignore_ascii_case(CEF_RENDERER) {
        CEF_RENDERER
    } else {
        SCREENCAST_RENDERER
    }
}

fn cef_runtime_status() -> CefRuntimeStatus {
    let tintin_chromium = tintin_chromium_status();
    let roots = cef_candidate_roots();
    let mut missing = Vec::new();
    for root in &roots {
        if let Some(framework_path) = cef_framework_path(root) {
            return CefRuntimeStatus {
                available: true,
                root: Some(display_path(root.clone())),
                framework_path: Some(display_path(framework_path)),
                missing: Vec::new(),
                tintin_chromium,
                build_hint: cef_build_hint(),
            };
        }
        missing.push(display_path(expected_cef_artifact(root)));
    }
    CefRuntimeStatus {
        available: false,
        root: roots.first().cloned().map(display_path),
        framework_path: None,
        missing,
        tintin_chromium,
        build_hint: cef_build_hint(),
    }
}

fn cef_candidate_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = ct_runtime::discover_cef_root() {
        add_cef_root_candidates(&mut roots, root);
    }
    for key in ["PUFFER_CEF_PATH", "PUFFER_CEF_ROOT", "CEF_PATH"] {
        if let Some(path) = std::env::var_os(key) {
            add_cef_root_candidates(&mut roots, PathBuf::from(path));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let tintin_root = PathBuf::from(home).join("chromium_tintin");
        add_cef_root_candidates(&mut roots, tintin_root.join("src/out/Release_GN_arm64"));
        add_cef_root_candidates(&mut roots, tintin_root.join("src/out/Release"));
        roots.extend(local_cef_distribution_roots(&tintin_root.join("output")));
    }
    roots.sort();
    roots.dedup();
    roots
}

fn add_cef_root_candidates(roots: &mut Vec<PathBuf>, root: PathBuf) {
    roots.push(root.clone());
    roots.push(root.join("Release"));
    roots.push(root.join("Release_GN_arm64"));
}

fn local_cef_distribution_roots(output_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(output_dir) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("cef_binary_")
        {
            roots.push(path.join("Release"));
        }
    }
    roots
}

fn cef_framework_path(root: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let framework = root
            .join("Chromium Embedded Framework.framework")
            .join("Chromium Embedded Framework");
        framework.is_file().then_some(framework)
    }
    #[cfg(target_os = "windows")]
    {
        let dll = root.join("libcef.dll");
        dll.is_file().then_some(dll)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let so = root.join("libcef.so");
        so.is_file().then_some(so)
    }
}

fn expected_cef_artifact(root: &Path) -> PathBuf {
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

fn tintin_chromium_status() -> TintinChromiumStatus {
    let Some(home) = std::env::var_os("HOME") else {
        return TintinChromiumStatus {
            executable: None,
            app_bundle: None,
            is_cef_runtime: false,
        };
    };
    let root = PathBuf::from(home).join("chromium_tintin/src/out/Release");
    let executable = root.join("Chromium.app/Contents/MacOS/Chromium");
    let app_bundle = root.join("Chromium.app");
    TintinChromiumStatus {
        executable: executable.is_file().then_some(display_path(executable)),
        app_bundle: app_bundle.is_dir().then_some(display_path(app_bundle)),
        is_cef_runtime: cef_framework_path(&root).is_some(),
    }
}

fn cef_fallback_reason(status: &CefRuntimeStatus) -> String {
    if status.available {
        return "CEF runtime was found. The daemon Browser backend uses screencast unless the desktop native CEF bridge is active.".to_string();
    }
    if status.tintin_chromium.executable.is_some() {
        "Tintin Chromium is available as Chromium.app, but no CEF framework was found. Using screencast until a CEF build is present.".to_string()
    } else {
        "No CEF framework was found. Using screencast until a CEF build is present.".to_string()
    }
}

fn cef_build_hint() -> String {
    "Use the puffer-cef asset from the berabuddies/ct release, point PUFFER_CEF_PATH at that extracted runtime, or build CEF from ~/chromium_tintin. Chromium.app alone is not loadable as CEF.".to_string()
}

fn display_path(path: PathBuf) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::{backend_status, normalize_renderer, CEF_RENDERER, SCREENCAST_RENDERER};

    #[test]
    fn renderer_preference_is_clamped() {
        assert_eq!(normalize_renderer("cef"), CEF_RENDERER);
        assert_eq!(normalize_renderer("CEF"), CEF_RENDERER);
        assert_eq!(normalize_renderer("other"), SCREENCAST_RENDERER);
    }

    #[test]
    fn cef_preference_reports_daemon_screencast_fallback() {
        let value = backend_status("cef");
        assert_eq!(value["preferredRenderer"], CEF_RENDERER);
        assert_eq!(value["activeRenderer"], SCREENCAST_RENDERER);
        assert!(value["fallbackReason"].as_str().is_some());
    }
}
