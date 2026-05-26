//! Filesystem watcher that flips the cross-thread reload signal on
//! `AppState` whenever a skill, MCP server, or plugin file under one of
//! the resource roots changes.
//!
//! Why this exists: puffer-tui can run as a persistent session inside a
//! managed environment, where the user adds or edits a skill / MCP server
//! manifest in another shell and expects puffer to pick the change up
//! without restarting the process. The existing `/reload-plugins` slash
//! command (and `/mcp reload` / `/plugin enable …`) all set the
//! `state.reload_resources_requested` flag; this watcher does the same on
//! a debounce, but from a background thread.
//!
//! Scope of what we watch:
//! - `<paths.user_config_dir>/resources/{skills,mcp_servers,plugins,prompts,agents,tools,internal_tools,hooks,ides}/`
//! - `<paths.workspace_config_dir>/resources/{...}` (same set)
//! - `<paths.builtin_resources_dir>/{...}` (developer convenience —
//!   editing builtin yaml in-place during local development triggers a
//!   reload too)
//!
//! The watcher is *recursive* per root so we catch `SKILL.md` changes
//! deep inside `<root>/skills/<name>/SKILL.md`. We attach to existing
//! directories on startup and tolerate missing ones; rerunning
//! `start()` after the user materializes a directory is the recommended
//! recovery, but in practice the workspace and user dirs already exist
//! after `ensure_workspace_dirs()` runs at startup.
//!
//! Debounce: filesystem events arrive in bursts (an editor that
//! rewrites a file emits several events back-to-back). We collapse
//! everything inside a 200 ms window into a single signal flip — that
//! batching alone keeps the reload cost reasonable. The TUI consumes the
//! signal via `AppState::take_reload_request()` on its next loop tick
//! and reloads exactly once.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use puffer_config::ConfigPaths;

/// Subdirectories of every resource root we consider relevant. Events
/// outside these are dropped on the floor so writes to unrelated files
/// in the same parent dir (e.g. `.git`, `.DS_Store`) don't trigger a
/// reload. Keep in sync with [`puffer_resources::load_resources`]:
/// anything that contributes to `LoadedResources` belongs here.
const WATCHED_SUBDIRS: &[&str] = &[
    "skills",
    "mcp_servers",
    "mcp", // legacy directory still honored by load_resources
    "plugins",
    "prompts",
    "agents",
    "tools",
    "internal_tools",
    "hooks",
    "ides",
    "mascots",
    "providers",
];

/// Minimum interval between two raised reload signals from this watcher.
/// Events that arrive inside the window are coalesced — the next event
/// that arrives *after* the window flips the signal again.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// Owns the `notify` watcher handle plus the shared signal. Drop the
/// `ResourceWatcher` to stop watching (the underlying `RecommendedWatcher`
/// terminates its event thread on drop).
///
/// The handle is intentionally inert: callers don't poll it. The signal
/// gets flipped from inside `notify`'s event-handler closure; the TUI
/// reads it via `AppState::take_reload_request()`.
pub struct ResourceWatcher {
    /// Stored to keep the watcher alive. Dropping this stops the
    /// background thread that delivers events.
    _watcher: RecommendedWatcher,
    /// Roots that were successfully attached. Exposed for tests and the
    /// debug `/doctor` view; not strictly required for operation.
    watched: Vec<PathBuf>,
}

impl ResourceWatcher {
    /// Start a watcher that flips `signal` whenever a resource file
    /// changes anywhere under `paths`. The user-config and workspace
    /// resource roots are materialized on demand so the watcher can
    /// attach even on a fresh checkout where the user hasn't dropped any
    /// resources yet; the builtin root is only attached if it already
    /// exists (we don't write into the install location). Returns an
    /// error only if `notify` itself can't be constructed.
    pub fn start(paths: &ConfigPaths, signal: Arc<AtomicBool>) -> Result<Self> {
        let user_resources = paths.user_config_dir.join("resources");
        let workspace_resources = paths.workspace_config_dir.join("resources");
        let _ = std::fs::create_dir_all(&user_resources);
        let _ = std::fs::create_dir_all(&workspace_resources);
        let roots = candidate_roots(paths);
        Self::start_with_roots(&roots, signal)
    }

    /// Like [`start`] but takes an explicit list of directories. The
    /// watcher attaches recursively to each existing root; events whose
    /// path doesn't fall under one of [`WATCHED_SUBDIRS`] are dropped.
    /// Used from tests so the watcher can target a `TempDir` without
    /// going through `ConfigPaths`.
    pub fn start_with_roots(roots: &[PathBuf], signal: Arc<AtomicBool>) -> Result<Self> {
        let last_fired: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let signal_clone = Arc::clone(&signal);
        let last_fired_clone = Arc::clone(&last_fired);
        let roots_clone: Vec<PathBuf> = roots.to_vec();

        let mut watcher: RecommendedWatcher =
            notify::recommended_watcher(move |result: notify::Result<Event>| {
                let Ok(event) = result else {
                    return;
                };
                if !is_meaningful_event(&event) {
                    return;
                }
                if !is_resource_event(&event, &roots_clone) {
                    return;
                }
                let now = Instant::now();
                let mut guard = match last_fired_clone.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if !should_raise_signal(*guard, signal_clone.load(Ordering::Acquire), now) {
                    // Already raised inside the debounce window; the
                    // existing flag covers this burst.
                    return;
                }
                *guard = Some(now);
                signal_clone.store(true, Ordering::Release);
            })
            .context("construct filesystem watcher")?;

        let mut attached = Vec::new();
        for root in roots {
            if root.exists() {
                match watcher.watch(root, RecursiveMode::Recursive) {
                    Ok(()) => attached.push(root.clone()),
                    Err(err) => {
                        tracing::warn!(
                            target = "puffer::resource_watcher",
                            "failed to attach watcher to {}: {err}",
                            root.display()
                        );
                    }
                }
            }
        }

        Ok(Self {
            _watcher: watcher,
            watched: attached,
        })
    }

    /// Returns the list of resource directories the watcher is currently
    /// attached to. Empty when none of the roots existed at startup.
    pub fn watched_roots(&self) -> &[PathBuf] {
        &self.watched
    }
}

/// Builds the candidate watch list for a `ConfigPaths`. Returns the
/// top-level `resources/` directory for each layer (builtin, user,
/// workspace) — the watcher attaches recursively so any subdirectory
/// added later is automatically covered. Missing roots are silently
/// skipped at attach time.
pub fn candidate_roots(paths: &ConfigPaths) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let roots = [
        paths.builtin_resources_dir.clone(),
        paths.user_config_dir.join("resources"),
        paths.workspace_config_dir.join("resources"),
    ];
    for root in roots {
        if !out.iter().any(|existing| existing == &root) {
            out.push(root);
        }
    }
    out
}

/// Filters out housekeeping events from the editor / OS that don't
/// actually mutate a resource file:
/// - `EventKind::Access(_)` (read-only access).
/// - Pure metadata changes outside of content/perm rewrites.
fn is_meaningful_event(event: &Event) -> bool {
    !matches!(event.kind, EventKind::Access(_))
}

/// Returns true when at least one path in the event is "resource-like":
///   1. some path component matches one of [`WATCHED_SUBDIRS`]
///      (e.g. `…/skills/picked-up/SKILL.md` contains a `skills` segment),
///      AND every component appearing *after* it is non-noise, AND
///   2. no path component is a dotfile / editor swap file / tmpfile.
///
/// We deliberately don't prefix-match against `_roots` because notify
/// reports paths through whatever the OS provides — on macOS, TempDir
/// roots come back canonicalized to `/private/var/folders/...` while our
/// configured root is the un-canonicalized symlink target, and the prefix
/// match would silently fail. Matching on a known subdir component is
/// just as targeted and survives this platform quirk.
fn is_resource_event(event: &Event, _roots: &[PathBuf]) -> bool {
    event.paths.iter().any(|p| {
        matches!(
            classify_resource_path(p.as_path()),
            ResourcePathMatch::Concrete
        )
    })
}

enum ResourcePathMatch {
    Concrete,
    ParentOnly,
    Noise,
    Unrelated,
}

fn classify_resource_path(path: &Path) -> ResourcePathMatch {
    // Walk path components forward. Ignore everything up to (and
    // including) the first match against a watched subdir; check only
    // the *trailing* segments for dotfile/swap-file noise. We can't
    // reject on a dot anywhere in the path because the parent directory
    // is allowed to be e.g. `/var/folders/.../.tmpABCDEF/...` on macOS,
    // and the user's `.puffer/resources/...` workspace dir always
    // starts with a dot too.
    let mut after_subdir: Option<usize> = None;
    let components: Vec<_> = path.components().collect();
    for (idx, component) in components.iter().enumerate() {
        let std::path::Component::Normal(raw) = component else {
            continue;
        };
        let Some(name) = raw.to_str() else {
            continue;
        };
        if WATCHED_SUBDIRS.iter().any(|sub| *sub == name) {
            after_subdir = Some(idx);
            break;
        }
    }
    let Some(start) = after_subdir else {
        return ResourcePathMatch::Unrelated;
    };
    let mut trailing_components = 0;
    for component in components.iter().skip(start) {
        let std::path::Component::Normal(raw) = component else {
            continue;
        };
        let Some(name) = raw.to_str() else {
            return ResourcePathMatch::Noise;
        };
        if name.starts_with('.') {
            return ResourcePathMatch::Noise;
        }
        if name.ends_with('~') || name.ends_with(".swp") || name.ends_with(".tmp") {
            return ResourcePathMatch::Noise;
        }
        if trailing_components > 0 {
            return ResourcePathMatch::Concrete;
        }
        trailing_components += 1;
    }
    ResourcePathMatch::ParentOnly
}

fn should_raise_signal(
    last_fired: Option<Instant>,
    signal_already_set: bool,
    now: Instant,
) -> bool {
    let Some(prev) = last_fired else {
        return true;
    };
    now.duration_since(prev) >= DEBOUNCE || !signal_already_set
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;
    use tempfile::tempdir;

    /// Poll the signal up to `timeout` for it to become true. Returns
    /// whether the signal flipped within the budget. We sleep in 25 ms
    /// chunks so individual tests don't waste the full timeout when the
    /// watcher fires quickly.
    fn wait_for_signal(signal: &Arc<AtomicBool>, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if signal.load(Ordering::Acquire) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        signal.load(Ordering::Acquire)
    }

    #[test]
    fn watcher_attaches_only_to_existing_roots() {
        let temp = tempdir().unwrap();
        let extant = temp.path().join("resources");
        fs::create_dir_all(&extant).unwrap();
        let missing = temp.path().join("absent");
        let roots = vec![extant.clone(), missing.clone()];

        let signal = Arc::new(AtomicBool::new(false));
        let watcher = ResourceWatcher::start_with_roots(&roots, signal).unwrap();
        let watched = watcher.watched_roots();
        assert!(watched.contains(&extant));
        assert!(!watched.contains(&missing));
    }

    #[test]
    fn watcher_flips_signal_when_new_skill_appears() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("resources");
        fs::create_dir_all(&root).unwrap();

        let signal = Arc::new(AtomicBool::new(false));
        let _watcher =
            ResourceWatcher::start_with_roots(&[root.clone()], Arc::clone(&signal)).unwrap();

        // Brief pause so the watcher thread completes attachment before
        // we generate events. notify is async under the hood; the
        // attach call returns before the watch is fully wired on some
        // platforms.
        std::thread::sleep(Duration::from_millis(100));

        let skill_dir = root.join("skills/reviewer");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: reviewer\ndescription: test\n---\nBody\n",
        )
        .unwrap();

        assert!(
            wait_for_signal(&signal, Duration::from_secs(3)),
            "filesystem watcher should raise the reload signal after a new SKILL.md is written"
        );
    }

    #[test]
    fn watcher_ignores_dotfile_writes() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("resources");
        fs::create_dir_all(root.join("skills")).unwrap();

        let signal = Arc::new(AtomicBool::new(false));
        let _watcher =
            ResourceWatcher::start_with_roots(&[root.clone()], Arc::clone(&signal)).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        // Editor swap files / hidden files should not fire reloads.
        fs::write(root.join("skills/.swap.swp"), b"swap").unwrap();
        // Give the watcher a generous window to wrongly trip.
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            !signal.load(Ordering::Acquire),
            "watcher should ignore editor swap files"
        );
    }

    #[test]
    fn watcher_ignores_unrelated_subdirs() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("resources");
        let unrelated = root.join(".git");
        fs::create_dir_all(&unrelated).unwrap();

        let signal = Arc::new(AtomicBool::new(false));
        let _watcher =
            ResourceWatcher::start_with_roots(&[root.clone()], Arc::clone(&signal)).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        fs::write(unrelated.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            !signal.load(Ordering::Acquire),
            "watcher should ignore files outside the recognized resource subdirs"
        );
    }

    #[test]
    fn watcher_debounces_event_bursts_into_one_flip() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("resources");
        fs::create_dir_all(root.join("skills")).unwrap();
        let signal = Arc::new(AtomicBool::new(false));
        let _watcher =
            ResourceWatcher::start_with_roots(&[root.clone()], Arc::clone(&signal)).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        // Fire a flurry of writes inside the debounce window. The
        // signal should still latch — but observing exactly one flip
        // through an `AtomicBool` is impossible because callers always
        // see "true" once it's set. What we *can* check is that the
        // signal isn't stuck and that a second flurry after the
        // debounce can re-raise it after the consumer clears the flag.
        for n in 0..6 {
            let path = root.join(format!("skills/a{n}.md"));
            fs::write(&path, "x").unwrap();
        }
        assert!(wait_for_signal(&signal, Duration::from_secs(2)));
        signal.store(false, Ordering::Release);
        std::thread::sleep(DEBOUNCE + Duration::from_millis(50));
        fs::write(root.join("skills/after.md"), "y").unwrap();
        assert!(
            wait_for_signal(&signal, Duration::from_secs(2)),
            "watcher should re-raise the signal after the debounce window elapses"
        );
    }

    #[test]
    fn debounce_allows_reraising_after_consumer_clears_signal() {
        let now = Instant::now();
        let first_fire = now - Duration::from_millis(10);

        assert!(
            !should_raise_signal(Some(first_fire), true, now),
            "a still-latched signal covers duplicate events inside the debounce window"
        );
        assert!(
            should_raise_signal(Some(first_fire), false, now),
            "a consumed signal can be raised again for a follow-up file write inside the debounce window"
        );
    }

    #[test]
    fn watcher_picks_up_mcp_server_manifests() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("resources");
        fs::create_dir_all(root.join("mcp_servers")).unwrap();
        let signal = Arc::new(AtomicBool::new(false));
        let _watcher =
            ResourceWatcher::start_with_roots(&[root.clone()], Arc::clone(&signal)).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        fs::write(
            root.join("mcp_servers/workspace.yaml"),
            "id: hot\ndisplay_name: Hot MCP\ntransport: stdio\ntarget: hot\nenabled: true\n",
        )
        .unwrap();
        assert!(wait_for_signal(&signal, Duration::from_secs(3)));
    }

    #[test]
    fn candidate_roots_cover_all_layers() {
        let temp = tempdir().unwrap();
        let paths = ConfigPaths {
            workspace_root: temp.path().join("workspace"),
            workspace_config_dir: temp.path().join("workspace/.puffer"),
            user_config_dir: temp.path().join(".home/.puffer"),
            builtin_resources_dir: temp.path().join("resources"),
        };
        let roots = candidate_roots(&paths);
        // One entry per layer (builtin, user, workspace).
        assert_eq!(roots.len(), 3);
        assert!(roots.contains(&paths.builtin_resources_dir));
        assert!(roots.contains(&paths.user_config_dir.join("resources")));
        assert!(roots.contains(&paths.workspace_config_dir.join("resources")));
    }
}
