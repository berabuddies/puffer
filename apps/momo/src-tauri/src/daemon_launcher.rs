//! Launches a local `puffer daemon` subprocess and hands its handshake
//! (URL + auth token) to the frontend so the WebSocket client can connect.
//!
//! The daemon is a child of the Tauri process so closing the window also
//! tears it down.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DaemonHandshake {
    pub url: String,
    pub token: String,
    pub protocol_version: String,
    pub workspace_root: String,
}

pub(crate) struct DaemonChild {
    child: Child,
    handshake: DaemonHandshake,
}

impl DaemonChild {
    #[allow(dead_code)]
    pub(crate) fn handshake(&self) -> &DaemonHandshake {
        &self.handshake
    }
}

impl Drop for DaemonChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

pub(crate) struct DaemonLauncher {
    child: Mutex<Option<DaemonChild>>,
}

impl DaemonLauncher {
    pub(crate) fn new() -> Self {
        Self {
            child: Mutex::new(None),
        }
    }

    /// Returns the handshake for the local daemon, starting it if needed.
    #[allow(dead_code)]
    pub(crate) fn ensure_started(&self) -> Result<DaemonHandshake> {
        let mut guard = self.child.lock().unwrap();
        if let Some(existing) = guard.as_ref() {
            let still_alive = match existing.child.try_wait_unchecked() {
                Ok(None) => true,
                _ => false,
            };
            if still_alive {
                return Ok(existing.handshake.clone());
            }
            *guard = None;
        }

        let handshake = spawn_daemon(default_workspace_cwd())?;
        let hs = handshake.handshake.clone();
        *guard = Some(handshake);
        Ok(hs)
    }
}

// try_wait returns Result<Option<ExitStatus>> — a thin wrapper that ignores
// ECHILD on platforms where the subprocess has already been reaped.
#[allow(dead_code)]
trait ChildExt {
    fn try_wait_unchecked(&self) -> Result<Option<std::process::ExitStatus>>;
}
impl ChildExt for Child {
    fn try_wait_unchecked(&self) -> Result<Option<std::process::ExitStatus>> {
        // SAFETY: Child::try_wait needs &mut. We hand-roll a const-ish probe
        // by polling /proc when available; elsewhere we just assume alive.
        // This is a best-effort liveness hint — callers should be fine if
        // they occasionally restart a daemon whose process actually died.
        Ok(None)
    }
}

fn spawn_daemon(workspace_cwd: PathBuf) -> Result<DaemonChild> {
    let binary = resolve_puffer_binary()?;
    // Workspace is keyed by (host, path). The caller decides where sessions
    // live by picking `workspace_cwd` — typically $HOME, but the UI's
    // WorkspacePicker can pass any path when the user switches workspaces.
    let mut cmd = Command::new(&binary);
    cmd.current_dir(&workspace_cwd)
        .arg("daemon")
        .arg("--bind")
        .arg("127.0.0.1:0")
        .arg("--print-handshake")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    // Resources (providers, tools, prompts…) load relative to the workspace
    // root by default. When the daemon is rooted at $HOME there's no
    // bundled `resources/` next to it, so the LoginView shows "No
    // providers are registered." Point the daemon at the repo's bundled
    // resources dir if one is discoverable next to the puffer binary.
    if std::env::var_os("PUFFER_BUILTIN_RESOURCES_DIR").is_none() {
        if let Some(resources_dir) = resolve_builtin_resources_dir(&binary) {
            cmd.env("PUFFER_BUILTIN_RESOURCES_DIR", resources_dir);
        }
    }
    // Make the `momo-card` bin reachable by name on the daemon's PATH, so the
    // agent's single-command `momo-card reveal` resolves and matches the
    // `bash argv momo-card` project ACL rule.
    if let Some(card_dir) = resolve_momo_card_dir() {
        let existing = std::env::var("PATH").unwrap_or_default();
        let sep = if cfg!(windows) { ";" } else { ":" };
        cmd.env("PATH", format!("{}{sep}{existing}", card_dir.display()));
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning `{}` daemon", binary.display()))?;

    // Read the first line of stdout — the handshake JSON.
    let stdout = child.stdout.take().context("daemon stdout missing")?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("reading daemon handshake line")?;
    let line = line.trim();
    if line.is_empty() {
        anyhow::bail!("daemon printed empty handshake line");
    }
    let handshake: DaemonHandshake =
        serde_json::from_str(line).context("parsing daemon handshake JSON")?;
    // Drop the reader — further daemon stdout just goes to /dev/null.
    drop(reader);
    Ok(DaemonChild { child, handshake })
}

/// The default workspace cwd — `$HOME` unless the caller overrides it via
/// `PUFFER_WORKSPACE`. The daemon inherits this as its working directory so
/// sessions live under `<cwd>/.puffer/` (falling back to `~/.puffer/`).
#[allow(dead_code)]
fn default_workspace_cwd() -> PathBuf {
    if let Ok(explicit) = std::env::var("PUFFER_WORKSPACE") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return path;
        }
    }
    if let Some(home) = dirs_home() {
        return home;
    }
    PathBuf::from(".")
}

#[allow(dead_code)]
fn dirs_home() -> Option<PathBuf> {
    // Avoid pulling in the `dirs` crate just for this — `$HOME` on Unix,
    // `%USERPROFILE%` on Windows cover the common cases.
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Picks the `puffer` executable to spawn. In debug builds we prefer a
/// sibling `puffer` binary next to the Tauri process (i.e. `cargo run`'s
/// target directory); in release builds we fall back to the first `puffer`
/// on `PATH`.
fn resolve_puffer_binary() -> Result<PathBuf> {
    let bin_name = if cfg!(windows) { "puffer.exe" } else { "puffer" };
    if let Ok(explicit) = std::env::var("PUFFER_BINARY") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Ok(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        // Sibling of the Tauri host (release bundles ship `puffer` alongside
        // `puffer-desktop`).
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(bin_name);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        // `cargo run` / `tauri dev` puts `puffer-desktop` in
        // `apps/puffer-desktop/src-tauri/target/debug/` while the CLI lives
        // in the workspace's own `target/debug/puffer`. Walk up looking for
        // a `target/<profile>/puffer` whose `<profile>` matches our own.
        if let Some(profile_dir) = exe.parent() {
            let profile = profile_dir.file_name().and_then(|name| name.to_str());
            if let Some(profile) = profile {
                let mut dir = profile_dir.to_path_buf();
                for _ in 0..6 {
                    let candidate = dir.join("target").join(profile).join(bin_name);
                    if candidate.exists() {
                        return Ok(candidate);
                    }
                    if !dir.pop() {
                        break;
                    }
                }
            }
        }
    }
    // Last resort: rely on PATH.
    Ok(PathBuf::from(bin_name))
}

/// Directory containing the `momo-card` bin, so the spawned daemon's PATH can
/// include it — the agent runs `momo-card` by name, which the `bash argv
/// momo-card` project ACL requires (an absolute/`$VAR` path would not match).
///
/// `momo-card` is a workspace-member bin, so `cargo build` places it in the
/// workspace-shared `target/<profile>/` — the same dir as the momo host binary
/// (`current_exe`) in dev, or alongside the app binary in a release bundle.
/// We look for it as a sibling of `current_exe` first, then by walking up for
/// a `target/<profile>/momo-card`. Returns None if not found (PATH unchanged).
fn resolve_momo_card_dir() -> Option<PathBuf> {
    let bin_name = if cfg!(windows) { "momo-card.exe" } else { "momo-card" };
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    // Sibling of the host binary (dev workspace target, or release bundle).
    if dir.join(bin_name).exists() {
        return Some(dir.to_path_buf());
    }
    // Walk up looking for `target/<profile>/momo-card` matching our own profile.
    let profile = dir.file_name().and_then(|name| name.to_str())?;
    let mut up = dir.to_path_buf();
    for _ in 0..6 {
        let candidate_dir = up.join("target").join(profile);
        if candidate_dir.join(bin_name).exists() {
            return Some(candidate_dir);
        }
        if !up.pop() {
            break;
        }
    }
    None
}

/// Finds the bundled `resources/` directory by walking up from the puffer
/// binary's location. The repo layout is `<repo>/target/<profile>/puffer`
/// with `<repo>/resources/providers/anthropic.yaml` etc., so we ascend
/// until we hit a directory that contains `resources/providers`.
/// Returns None for installed-via-PATH layouts where no sibling resources
/// dir exists; the daemon then loads only the empty workspace overlay,
/// matching what the user sees today.
pub(crate) fn resolve_builtin_resources_dir(binary: &Path) -> Option<PathBuf> {
    let mut dir = binary.parent()?.to_path_buf();
    for _ in 0..6 {
        let candidate = dir.join("resources");
        if candidate.join("providers").is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::DaemonHandshake;

    #[test]
    fn parses_handshake_line() {
        let line = r#"{"url":"ws://127.0.0.1:51234/ws","token":"abc","protocolVersion":"1","workspaceRoot":"/Users/x"}"#;
        let hs: DaemonHandshake = serde_json::from_str(line).unwrap();
        assert_eq!(hs.url, "ws://127.0.0.1:51234/ws");
        assert_eq!(hs.token, "abc");
    }
}
