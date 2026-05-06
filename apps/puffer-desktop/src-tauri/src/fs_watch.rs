//! Filesystem watch registry — powers the Files pane's auto-refresh.
//!
//! The desktop's `FilesPane` caches directory listings client-side; without a
//! push signal from the daemon, agent-driven mutations (edits, moves, new
//! files) leave the tree stale until the user manually collapses/expands a
//! directory. This module fills that gap by wrapping `notify::RecommendedWatcher`
//! into a per-client subscription:
//!
//!   watch(paths, recursive) → debounced `workspace:fs:changed` events
//!   unwatch(watchId)        → drops the watcher + stops broadcasting
//!
//! Events are fanned onto the daemon's shared broadcast bus via
//! `DaemonState::publish_event(...)` so they also land in the replay ring
//! buffer — clients that reconnect after a drop see the historical changes
//! flagged with `"replay": true` and can ignore them (a freshly-mounted
//! FilesPane hasn't cached anything yet, so the replay is uninteresting).
//!
//! Debouncing: the native backends (FSEvents / inotify / ReadDirectoryChangesW)
//! can deliver bursts of events for a single logical write (temp-file rename,
//! metadata-then-content). To keep the UI from thrashing we coalesce changed
//! paths inside a 100 ms window, emitting at most one event per watch per
//! window with the union of unique paths seen during that interval.

use anyhow::{bail, Context, Result};
use notify::{Config as NotifyConfig, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

use crate::events::EventEmitter;
use crate::files::path_is_allowed;

/// How long to collect bursts before firing a single change event. Chosen
/// conservatively: below ~50 ms the desktop gets noticeably thrashy on file
/// saves that touch a temp file and rename; above ~200 ms the agent can feel
/// the delay when it's rewriting a file and the user is watching the tree.
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(100);

/// Owns one `RecommendedWatcher` + the debounce state for that subscription.
/// Dropping this struct drops the watcher, which unregisters the kernel-level
/// subscriptions (notify handles the teardown).
struct FsWatchHandle {
    _watcher: RecommendedWatcher,
    /// Kept alive so the debounce thread knows when to exit: on drop of the
    /// handle, this flag flips to `true` and the thread notices on its next
    /// wake-up and exits gracefully. Mutex<Option<…>> is just so the pending
    /// set can be taken/replaced from the producer side.
    pending: Arc<DebounceState>,
}

/// Shared between the notify callback and the debounce thread.
struct DebounceState {
    /// Unique paths changed since the last flush. `Mutex` because the notify
    /// callback (sync) and the timer thread both mutate it.
    paths: Mutex<HashSet<PathBuf>>,
    /// Signals "the handle was dropped, bail out". Set by `Drop for FsWatchHandle`.
    stopped: Mutex<bool>,
}

impl Drop for FsWatchHandle {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.pending.stopped.lock() {
            *guard = true;
        }
    }
}

/// Registry of active watches, keyed by watch-id (daemon-minted uuid unless
/// the client supplied one). Wrapped in an Arc on DaemonState so RPC handlers
/// and the debounce threads share a single registry across the lifetime of
/// the daemon.
pub struct FsWatchRegistry {
    inner: Mutex<HashMap<String, FsWatchHandle>>,
}

impl FsWatchRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Start watching `paths`. Each path is validated against the daemon's
    /// allowlist (session cwd / workspace root / $HOME) just like the read-only
    /// file RPCs — the WebSocket token is already the auth boundary, but we
    /// still refuse to let a client subscribe outside its sandbox.
    ///
    /// Returns the opaque `watchId` the caller uses to `unwatch` later. If
    /// the caller supplied `watch_id`, it's reused (idempotent-ish — calling
    /// with the same id replaces the existing watch).
    pub fn watch(
        self: &Arc<Self>,
        events: EventEmitter,
        watch_id: Option<String>,
        paths: Vec<PathBuf>,
        recursive: bool,
        allowed_roots: &[PathBuf],
    ) -> Result<String> {
        if paths.is_empty() {
            bail!("no paths provided");
        }

        // Canonicalize + validate every path before we start binding to
        // kernel watches. Fail fast so partial state doesn't leak through on
        // a typo in the middle of the list.
        let mut canonical_paths: Vec<PathBuf> = Vec::with_capacity(paths.len());
        for p in &paths {
            if !p.is_absolute() {
                bail!("path must be absolute: {}", p.display());
            }
            let canonical = std::fs::canonicalize(p)
                .with_context(|| format!("path does not exist: {}", p.display()))?;
            if !path_is_allowed(allowed_roots, &canonical) {
                bail!("path escapes allowed roots: {}", canonical.display());
            }
            canonical_paths.push(canonical);
        }

        let watch_id = watch_id.unwrap_or_else(|| Uuid::new_v4().to_string());

        // If this id was already in use, drop the old handle first. Dropping
        // the RecommendedWatcher unregisters its native subscription, so we
        // won't leak kernel resources.
        {
            let mut guard = self.inner.lock().unwrap();
            guard.remove(&watch_id);
        }

        let pending = Arc::new(DebounceState {
            paths: Mutex::new(HashSet::new()),
            stopped: Mutex::new(false),
        });

        // Build the watcher. Notify's v6 recommended-watcher uses the best
        // native backend available (FSEvents on macOS, inotify on Linux,
        // ReadDirectoryChangesW on Windows). We route every interesting event
        // into the shared pending set — the debounce thread does the fan-out.
        let pending_for_cb = pending.clone();
        let watcher_res = RecommendedWatcher::new(
            move |res: notify::Result<notify::Event>| {
                let Ok(event) = res else { return };
                // Ignore pure access events (reads) — the UI only cares about
                // mutations. Anything that could change what the tree renders
                // (create / modify / remove / rename) passes through.
                if matches!(event.kind, EventKind::Access(_)) {
                    return;
                }
                let mut guard = match pending_for_cb.paths.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                for p in event.paths {
                    guard.insert(p);
                }
            },
            NotifyConfig::default(),
        );
        let mut watcher = watcher_res.context("build filesystem watcher")?;

        // Bind each canonical path to the watcher.
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        for p in &canonical_paths {
            watcher
                .watch(p, mode)
                .with_context(|| format!("watch path {}", p.display()))?;
        }

        // Spin up the debounce / flush thread. It wakes every DEBOUNCE_WINDOW,
        // drains the pending set, and fires a `workspace:fs:changed` event if
        // anything accumulated. Exits when `stopped` flips to true (i.e. the
        // watch was unregistered — dropping the handle sets that flag).
        let events_for_thread = events.clone();
        let watch_id_for_thread = watch_id.clone();
        let pending_for_thread = pending.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(DEBOUNCE_WINDOW);
                // Bail if the handle was dropped.
                if pending_for_thread
                    .stopped
                    .lock()
                    .map(|g| *g)
                    .unwrap_or(true)
                {
                    break;
                }
                let drained: Vec<PathBuf> = {
                    let mut guard = match pending_for_thread.paths.lock() {
                        Ok(g) => g,
                        Err(_) => break,
                    };
                    if guard.is_empty() {
                        continue;
                    }
                    guard.drain().collect()
                };
                let payload = build_event_payload(&watch_id_for_thread, &drained);
                events_for_thread.emit("workspace:fs:changed", payload);
            }
        });

        let handle = FsWatchHandle {
            _watcher: watcher,
            pending,
        };
        self.inner.lock().unwrap().insert(watch_id.clone(), handle);
        Ok(watch_id)
    }

    /// Stop watching. Idempotent — calling on an already-removed id is a
    /// no-op. Dropping the handle unregisters the native watch and signals
    /// the debounce thread to exit on its next tick.
    pub fn unwatch(&self, watch_id: &str) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        guard.remove(watch_id);
        Ok(())
    }
}

fn build_event_payload(watch_id: &str, paths: &[PathBuf]) -> Value {
    let mut out: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    // Sort for a stable event payload — helps UI diffing + log readability.
    out.sort();
    json!({
        "watchId": watch_id,
        "paths": out,
        "changedAtMs": now_ms(),
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// RPC handlers — wired into the daemon's method dispatcher.
// ---------------------------------------------------------------------------

pub(crate) fn handle_fs_watch(
    registry: &Arc<FsWatchRegistry>,
    events: EventEmitter,
    params: &Value,
    allowed_roots: &[PathBuf],
) -> Result<Value> {
    let paths_raw = params
        .get("paths")
        .and_then(|v| v.as_array())
        .context("missing paths array")?;
    if paths_raw.is_empty() {
        bail!("paths array is empty");
    }
    let mut paths: Vec<PathBuf> = Vec::with_capacity(paths_raw.len());
    for v in paths_raw {
        let s = v.as_str().context("paths entry must be a string")?;
        paths.push(PathBuf::from(s));
    }
    let recursive = params
        .get("recursive")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let watch_id = params
        .get("watchId")
        .or_else(|| params.get("watch_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let id = registry
        .clone()
        .watch(events, watch_id, paths, recursive, allowed_roots)?;
    Ok(json!({ "watchId": id }))
}

pub(crate) fn handle_fs_unwatch(registry: &FsWatchRegistry, params: &Value) -> Result<Value> {
    let watch_id = params
        .get("watchId")
        .or_else(|| params.get("watch_id"))
        .and_then(|v| v.as_str())
        .context("missing watchId")?;
    registry.unwatch(watch_id)?;
    Ok(json!({ "ok": true }))
}

// Crate-level helper: lets unit tests / smoke scripts validate a path the
// same way a real `fs_watch` request would. Kept alongside the RPC helpers
// so the allowlist semantics live in one place.
#[allow(dead_code)]
pub(crate) fn validate_watch_path(allowed_roots: &[PathBuf], raw: &Path) -> Result<PathBuf> {
    if !raw.is_absolute() {
        bail!("path must be absolute: {}", raw.display());
    }
    let canonical = std::fs::canonicalize(raw)
        .with_context(|| format!("path does not exist: {}", raw.display()))?;
    if !path_is_allowed(allowed_roots, &canonical) {
        bail!("path escapes allowed roots: {}", canonical.display());
    }
    Ok(canonical)
}
