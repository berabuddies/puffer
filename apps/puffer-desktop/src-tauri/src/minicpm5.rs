//! MiniCPM5 local-model onboarding: detect whether to recommend the on-device
//! model, and run its installer with streamed progress.
//!
//! The detection + install logic lives in `scripts/minicpm5-{recommend,install}.sh`
//! (single source of truth, also usable from a terminal). These commands just
//! locate and run them, surfacing the result to the desktop onboarding card.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

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

// ---- behavior analysis: the "does something" half of the feature ----------
// The installed local model runs continuous, on-device user-behavior analysis.
// We tail the active session transcript through scripts/minicpm5-behavior.py
// (which calls the local mlx server) and stream each rolling read as a
// `minicpm5://behavior` event. One watcher at a time.

/// The running behavior watcher child, if any. Replaced on start, killed on stop.
static BEHAVIOR: Mutex<Option<Child>> = Mutex::new(None);
/// The mlx server only needs starting once per app run, not on every session
/// switch (serve.sh doesn't single-instance and .status() can block).
static SERVE_ENSURED: AtomicBool = AtomicBool::new(false);
/// Monotonic watcher generation. Bumped on every start/stop so a superseded
/// watcher's reader thread stops emitting (no stale analysis under a new
/// session) and a start that lost the race aborts instead of clobbering.
static BEHAVIOR_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn puffer_home() -> PathBuf {
    std::env::var_os("PUFFER_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".puffer")))
        .unwrap_or_else(|| PathBuf::from(".puffer"))
}

/// Kill any running watcher. Best-effort.
fn stop_behavior_locked(slot: &mut Option<Child>) {
    if let Some(mut child) = slot.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Start (or restart) behavior analysis for `session_id`. Ensures the mlx
/// server is up (best-effort, non-blocking), then tails the session transcript
/// and streams `minicpm5://behavior` events. Replaces any existing watcher.
#[tauri::command]
pub fn minicpm5_behavior_start(app: AppHandle, session_id: String) -> Result<(), String> {
    use std::sync::atomic::Ordering::SeqCst;
    // Session ids are daemon-issued UUIDs; reject anything with path separators
    // so an unexpected value can't escape the sessions dir.
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains("..")
    {
        return Err("invalid session id".to_string());
    }
    // Claim a generation; if a newer start arrives before we install, we abort.
    let gen = BEHAVIOR_GEN.fetch_add(1, SeqCst) + 1;
    let behavior = script_path("minicpm5-behavior.py").ok_or("behavior script not found")?;

    // Hold the lock across the whole start so concurrent starts fully serialize.
    let mut slot = BEHAVIOR.lock().unwrap_or_else(|e| e.into_inner());
    // A newer start was issued while we waited for the lock — yield to it.
    if BEHAVIOR_GEN.load(SeqCst) != gen {
        return Ok(());
    }
    // Stop the previous watcher BEFORE validating the new session, so switching
    // to a session with no local transcript still tears down the stale watcher.
    stop_behavior_locked(&mut slot);

    // Resolve + canonicalize, and require the target stay under the sessions
    // dir (defends against symlink/`..` escape even past the id check).
    let sessions_root = puffer_home().join("sessions");
    let session_file = sessions_root.join(format!("{session_id}.session.jsonl"));
    let canonical = session_file
        .canonicalize()
        .map_err(|_| format!("session transcript not found: {}", session_file.display()))?;
    let root_canonical = sessions_root
        .canonicalize()
        .map_err(|_| "sessions directory missing".to_string())?;
    if !canonical.starts_with(&root_canonical) || !canonical.is_file() {
        return Err("session transcript outside sessions directory".to_string());
    }

    // Ensure the local server is running — once per app run (serve.sh does not
    // single-instance and .status() can block). Fire-and-forget; the watcher
    // tolerates a not-yet-ready server.
    if !SERVE_ENSURED.swap(true, SeqCst) {
        if let Some(serve) = script_path("minicpm5-serve.sh") {
            std::thread::spawn(move || {
                let _ = Command::new("/bin/bash").arg(&serve).arg("--bg").status();
            });
        }
    }

    // `-u` = unbuffered, so each analysis line reaches us immediately.
    let mut child = Command::new("python3")
        .arg("-u")
        .arg(&behavior)
        .arg("--watch")
        .arg(&canonical)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("spawn behavior watcher: {err}"))?;

    if let Some(out) = child.stdout.take() {
        let app = app.clone();
        let sid = session_id.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                // Superseded by a newer start/stop — stop emitting stale reads.
                if BEHAVIOR_GEN.load(SeqCst) != gen {
                    break;
                }
                let line = line.trim();
                if line.is_empty() || !line.starts_with('{') {
                    continue; // skip the "[watch] …" banner
                }
                if let Ok(value) = serde_json::from_str::<Value>(line) {
                    // Tag with the session so the UI ignores events from a
                    // watcher that was for a different (now inactive) session.
                    let _ = app.emit(
                        "minicpm5://behavior",
                        json!({ "sessionId": sid, "behavior": value }),
                    );
                }
            }
        });
    }
    *slot = Some(child);
    Ok(())
}

/// Stop the behavior watcher.
#[tauri::command]
pub fn minicpm5_behavior_stop() -> Result<(), String> {
    // Bump the generation so the current reader thread stops emitting any
    // still-buffered lines before its pipe closes.
    BEHAVIOR_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut slot = BEHAVIOR.lock().unwrap_or_else(|e| e.into_inner());
    stop_behavior_locked(&mut slot);
    Ok(())
}
