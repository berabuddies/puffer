//! Host-scoped daemon singleton guard.

use anyhow::{bail, Context, Result};
use fslock::LockFile;
use puffer_config::ConfigPaths;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Holds the host-scoped daemon lock for the lifetime of the daemon process.
pub(crate) struct DaemonHostGuard {
    _lock: LockFile,
}

/// Acquires the one-daemon-per-host guard for the active Puffer home.
pub(crate) fn acquire(paths: &ConfigPaths) -> Result<DaemonHostGuard> {
    let path = lock_path(paths);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create daemon lock directory {}",
                parent.display()
            )
        })?;
    }
    let mut lock = LockFile::open(&path)
        .map_err(anyhow::Error::new)
        .with_context(|| format!("failed to open daemon lock {}", path.display()))?;
    let acquired = lock
        .try_lock_with_pid()
        .map_err(anyhow::Error::new)
        .with_context(|| format!("failed to acquire daemon lock {}", path.display()))?;
    if !acquired {
        bail!(
            "another Puffer daemon is already running on this host{}",
            active_daemon_hint(paths, &path)
        );
    }
    Ok(DaemonHostGuard { _lock: lock })
}

fn lock_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("daemon.lock")
}

fn active_daemon_hint(paths: &ConfigPaths, path: &Path) -> String {
    let mut parts = Vec::new();
    if let Some(pid) = lock_owner_pid(path) {
        parts.push(format!("pid {pid}"));
    }
    if let Some(handshake) = load_handshake_hint(paths) {
        parts.push(handshake);
    }
    if parts.is_empty() {
        format!("; lock: {}", path.display())
    } else {
        format!(" ({})", parts.join(", "))
    }
}

fn lock_owner_pid(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|pid| !pid.is_empty())
}

fn load_handshake_hint(paths: &ConfigPaths) -> Option<String> {
    let path = paths.user_config_dir.join("daemon.handshake");
    let raw = std::fs::read_to_string(&path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let url = value.get("url").and_then(Value::as_str)?;
    let workspace = value.get("workspaceRoot").and_then(Value::as_str);
    Some(match workspace {
        Some(workspace) if !workspace.is_empty() => {
            format!("url {url}, workspace {workspace}")
        }
        _ => format!("url {url}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn paths() -> (tempfile::TempDir, ConfigPaths) {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let user = temp.path().join("home/.puffer");
        let resources = workspace.join("resources");
        (
            temp,
            ConfigPaths {
                workspace_root: workspace.clone(),
                workspace_config_dir: workspace.join(".puffer"),
                user_config_dir: user,
                builtin_resources_dir: resources,
            },
        )
    }

    #[test]
    fn rejects_second_daemon_for_same_host_config() {
        let (_temp, paths) = paths();
        let _first = acquire(&paths).expect("first daemon lock");
        let error = match acquire(&paths) {
            Ok(_) => panic!("second daemon should fail"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("another Puffer daemon is already running on this host"));
    }

    #[test]
    fn releases_lock_when_guard_drops() {
        let (_temp, paths) = paths();
        {
            let _first = acquire(&paths).expect("first daemon lock");
        }
        let _second = acquire(&paths).expect("daemon lock after drop");
    }
}
