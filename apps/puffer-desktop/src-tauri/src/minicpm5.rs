//! MiniCPM5 local-model onboarding: detect whether to recommend the on-device
//! model, and run its installer with streamed progress.
//!
//! The detection + install logic lives in `scripts/minicpm5-{recommend,install}.sh`
//! (single source of truth, also usable from a terminal). These commands just
//! locate and run them, surfacing the result to the desktop onboarding card.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

/// Guards against concurrent installs (e.g. a double-clicked button) clobbering
/// the same `~/.puffer` paths.
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// Locate a repo `scripts/<name>` from a dev/bundled layout. Mirrors
/// daemon_launcher's resources-dir walk: climb from the exe (and cwd) looking
/// for a `scripts/` sibling of the bundled `resources/`.
fn script_path(name: &str) -> Option<PathBuf> {
    // Only the binary-relative walk is trusted in ALL builds: it resolves next
    // to the actually-installed executable. cwd-walking and the PUFFER_REPO
    // override are dev conveniences and are attacker-influenceable (the app may
    // be launched from an untrusted directory whose ./scripts/ we'd otherwise
    // run on mount), so they are debug-only. Every candidate is canonicalized
    // and must be a regular file.
    fn resolved(candidate: PathBuf) -> Option<PathBuf> {
        let canonical = candidate.canonicalize().ok()?;
        canonical.is_file().then_some(canonical)
    }

    #[cfg(debug_assertions)]
    if let Ok(repo) = std::env::var("PUFFER_REPO") {
        if let Some(p) = resolved(PathBuf::from(repo).join("scripts").join(name)) {
            return Some(p);
        }
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
    {
        roots.push(dir);
    }
    #[cfg(debug_assertions)]
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }

    for base in roots {
        let mut dir = base;
        for _ in 0..8 {
            if let Some(p) = resolved(dir.join("scripts").join(name)) {
                return Some(p);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    None
}

/// Should puffer recommend installing the local model on this machine? Returns
/// the recommend.sh JSON decision (recommend/false + reason/metadata).
#[tauri::command]
pub fn minicpm5_recommend() -> Value {
    let Some(script) = script_path("minicpm5-recommend.sh") else {
        return json!({ "recommend": false, "reason": "installer scripts not found" });
    };
    match Command::new("/bin/bash").arg(&script).output() {
        Ok(out) => {
            // Scan backward for the decision line: the last line that parses as
            // a JSON object with a boolean `recommend`. Tolerates trailing
            // warnings / noisy wrappers after the JSON, and empty output.
            let txt = String::from_utf8_lossy(&out.stdout);
            for line in txt.lines().rev() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(value) = serde_json::from_str::<Value>(line) {
                    if value.get("recommend").and_then(Value::as_bool).is_some() {
                        return value;
                    }
                }
            }
            let stderr = String::from_utf8_lossy(&out.stderr);
            let hint = stderr.lines().last().unwrap_or("no decision emitted");
            json!({ "recommend": false, "reason": format!("no recommendation: {}", hint.trim()) })
        }
        Err(err) => json!({ "recommend": false, "reason": format!("run failed: {err}") }),
    }
}

/// Run the installer in the background, streaming stdout/stderr lines as
/// `minicpm5://install-log` events and a final `minicpm5://install-done`
/// ({ success: bool }). Non-blocking so the UI stays responsive during the
/// multi-minute weight download.
#[tauri::command]
pub fn minicpm5_install(app: AppHandle) -> Result<(), String> {
    let script = script_path("minicpm5-install.sh").ok_or("installer script not found")?;
    // Reject a second concurrent install instead of racing two writers into
    // ~/.puffer. The UI also hides the button while installing; this is the
    // backend backstop.
    if INSTALLING.swap(true, Ordering::SeqCst) {
        let _ = app.emit(
            "minicpm5://install-log",
            "Install already in progress.".to_string(),
        );
        return Ok(());
    }
    std::thread::spawn(move || {
        let spawned = Command::new("/bin/bash")
            .arg(&script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(err) => {
                INSTALLING.store(false, Ordering::SeqCst);
                let _ = app.emit(
                    "minicpm5://install-done",
                    json!({ "success": false, "error": err.to_string() }),
                );
                return;
            }
        };

        // Merge stderr into the same log stream so progress + warnings show.
        // Keep the handle so we can drain it fully before reporting done —
        // otherwise install-done can fire ahead of the final stderr lines.
        let stderr_handle = child.stderr.take().map(|err| {
            let app = app.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let _ = app.emit("minicpm5://install-log", line);
                }
            })
        });
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = app.emit("minicpm5://install-log", line);
            }
        }

        let success = child.wait().map(|s| s.success()).unwrap_or(false);
        if let Some(handle) = stderr_handle {
            let _ = handle.join();
        }
        INSTALLING.store(false, Ordering::SeqCst);
        let _ = app.emit("minicpm5://install-done", json!({ "success": success }));
    });
    Ok(())
}
