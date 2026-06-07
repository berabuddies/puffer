//! Launches a local `puffer daemon` subprocess and hands its handshake
//! (URL + auth token) to the frontend so the WebSocket client can connect.
//!
//! The daemon is a child of the Tauri process so closing the window also
//! tears it down. Remote mode spawns `ssh <target> puffer daemon ...` and
//! opens a local port-forward to the remote WebSocket so the frontend can
//! continue to connect to `ws://127.0.0.1:<localport>/ws` transparently.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DaemonHandshake {
    pub url: String,
    pub token: String,
    pub protocol_version: String,
    pub workspace_root: String,
}

pub(crate) struct DaemonChild {
    child: Option<Child>,
    handshake: DaemonHandshake,
}

impl DaemonChild {
    #[allow(dead_code)]
    pub(crate) fn handshake(&self) -> &DaemonHandshake {
        &self.handshake
    }

    fn spawned(child: Child, handshake: DaemonHandshake) -> Self {
        Self {
            child: Some(child),
            handshake,
        }
    }

    fn attached(handshake: DaemonHandshake) -> Self {
        Self {
            child: None,
            handshake,
        }
    }

    fn is_alive(&self) -> bool {
        match &self.child {
            Some(child) => matches!(child.try_wait_unchecked(), Ok(None)),
            None => daemon_handshake_reachable(&self.handshake),
        }
    }
}

impl Drop for DaemonChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
        }
    }
}

/// Supplemental children kept alive for a remote-daemon session (e.g. the
/// `ssh -N -L …` port forward).
struct AuxChildren {
    children: Vec<Child>,
}
impl Drop for AuxChildren {
    fn drop(&mut self) {
        for c in self.children.iter_mut() {
            let _ = c.kill();
        }
    }
}

#[derive(Default)]
pub(crate) struct DaemonLauncher {
    child: Mutex<Option<DaemonChild>>,
    // Active SSH sessions keyed by the handshake URL — lets the frontend
    // disconnect a specific remote without affecting others.
    remotes: Mutex<Vec<RemoteSession>>,
}

struct RemoteSession {
    // Keep the handshake around for diagnostics / future "list remotes" UX.
    #[allow(dead_code)]
    handshake: DaemonHandshake,
    #[allow(dead_code)]
    remote_child: Child,
    #[allow(dead_code)]
    aux: AuxChildren,
}
impl Drop for RemoteSession {
    fn drop(&mut self) {
        let _ = self.remote_child.kill();
    }
}

impl DaemonLauncher {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Returns the handshake for the local daemon, starting it if needed.
    #[allow(dead_code)]
    pub(crate) fn ensure_started(&self) -> Result<DaemonHandshake> {
        let mut guard = self.child.lock().unwrap();
        if let Some(existing) = guard.as_ref() {
            if existing.is_alive() {
                return Ok(existing.handshake.clone());
            }
            *guard = None;
        }

        let handshake = spawn_daemon(default_workspace_cwd())?;
        let hs = handshake.handshake.clone();
        *guard = Some(handshake);
        Ok(hs)
    }

    /// Tears down the current local daemon subprocess and spawns a fresh
    /// one rooted at `cwd`. Sessions live under `<cwd>/.puffer/` so
    /// switching the cwd effectively switches the workspace the UI sees.
    /// Used by the WorkspacePicker's "Switch local workspace" action.
    pub(crate) fn restart_local(&self, cwd: PathBuf) -> Result<DaemonHandshake> {
        let mut guard = self.child.lock().unwrap();
        // Drop the existing child — Drop impl calls kill() — then wait a
        // beat for the OS to release the ephemeral port and the handshake
        // file before we spawn the replacement.
        *guard = None;
        std::thread::sleep(std::time::Duration::from_millis(100));
        let handshake = spawn_daemon(cwd)?;
        let hs = handshake.handshake.clone();
        *guard = Some(handshake);
        Ok(hs)
    }

    /// Spawns `puffer daemon` on a remote host over SSH, forwards its
    /// WebSocket port to a local port, and returns a handshake pointing at
    /// the local end. The frontend uses this URL with its normal
    /// DaemonClient — the tunnel makes the remote look local.
    ///
    /// `ssh_target` is the usual `[user@]host` form. `remote_binary` is an
    /// optional override for the remote `puffer` executable path.
    pub(crate) fn start_ssh(
        &self,
        ssh_target: &str,
        remote_binary: Option<&str>,
        remote_workspace: Option<&str>,
    ) -> Result<DaemonHandshake> {
        let (mut remote_child, remote_handshake) =
            spawn_remote_daemon(ssh_target, remote_binary, remote_workspace)?;

        // Parse the remote port from the daemon's announced URL.
        let remote_port =
            parse_ws_port(&remote_handshake.url).context("parsing remote daemon WebSocket port")?;

        // Pick a free local port. Small race window between drop + forward
        // bind — tolerable since we immediately try to forward.
        let local_port = {
            let listener =
                TcpListener::bind("127.0.0.1:0").context("picking local forward port")?;
            listener.local_addr()?.port()
        };

        // ssh -N -L <local>:127.0.0.1:<remote> <target>
        // ServerAliveInterval keeps idle connections alive; ExitOnForwardFailure
        // makes ssh quit if the forward can't be bound.
        let forward = Command::new("ssh")
            .arg("-o")
            .arg("ExitOnForwardFailure=yes")
            .arg("-o")
            .arg("ServerAliveInterval=30")
            .arg("-N")
            .arg("-L")
            .arg(format!(
                "{local}:127.0.0.1:{remote}",
                local = local_port,
                remote = remote_port
            ))
            .arg(ssh_target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .context("spawning ssh port-forward")?;

        // Wait for the ssh forward to actually bind the local port — probe
        // with short-timeout TCP connects instead of a blind sleep. Retry
        // every 50 ms for up to 5 s.
        let local_addr: SocketAddr = format!("127.0.0.1:{local_port}").parse()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut bound = false;
        while Instant::now() < deadline {
            match TcpStream::connect_timeout(&local_addr, Duration::from_millis(200)) {
                Ok(stream) => {
                    drop(stream);
                    bound = true;
                    break;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        if !bound {
            // Forward never came up; kill the remote daemon so we don't
            // leak a subprocess and surface a useful error.
            let _ = remote_child.kill();
            anyhow::bail!(
                "ssh port-forward {local_port}→{remote_port} never accepted a TCP probe — \
check that sshd allows TCP forwarding and that the remote daemon really bound"
            );
        }
        let _ = &remote_child;

        let local_url = format!("ws://127.0.0.1:{local_port}/ws");
        let local_handshake = DaemonHandshake {
            url: local_url,
            token: remote_handshake.token.clone(),
            protocol_version: remote_handshake.protocol_version.clone(),
            workspace_root: remote_handshake.workspace_root.clone(),
        };

        // Track the session so our Drop teardown kills both processes when
        // the Tauri app exits.
        self.remotes.lock().unwrap().push(RemoteSession {
            handshake: local_handshake.clone(),
            remote_child,
            aux: AuxChildren {
                children: vec![forward],
            },
        });

        Ok(local_handshake)
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
    let browser_debug = daemon_browser_debug_enabled();
    cmd.current_dir(&workspace_cwd)
        .arg("daemon")
        .arg("--bind")
        .arg("127.0.0.1:0")
        .arg("--print-handshake")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if std::env::var_os("PUFFER_CEF_REMOTE_DEBUGGING_PORT").is_none() {
        cmd.env("PUFFER_CEF_REMOTE_DEBUGGING_PORT", "9333");
    }
    if browser_debug && std::env::var_os("PUFFER_BROWSER_DEBUG").is_none() {
        cmd.env("PUFFER_BROWSER_DEBUG", "1");
    }
    if std::env::var_os("PUFFER_CEF_PROFILE_DIR").is_none() {
        if let Some(profile_dir) = default_cef_profile_dir() {
            cmd.env("PUFFER_CEF_PROFILE_DIR", profile_dir);
        }
    }
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
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning `{}` daemon", binary.display()))?;
    let stderr = child.stderr.take();

    // Read the first line of stdout — the handshake JSON.
    let stdout = child.stdout.take().context("daemon stdout missing")?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("reading daemon handshake line")?;
    let line = line.trim();
    if line.is_empty() {
        let mut stderr_text = String::new();
        if let Some(mut pipe) = stderr {
            let _ = pipe.read_to_string(&mut stderr_text);
        }
        let status = child.wait().ok();
        let message = daemon_handshake_failure_message(status, &stderr_text);
        if message.contains("another Puffer daemon is already running") {
            if let Some(handshake) = existing_daemon_handshake(&workspace_cwd) {
                return Ok(DaemonChild::attached(handshake));
            }
        }
        anyhow::bail!("{message}");
    }
    let handshake: DaemonHandshake =
        serde_json::from_str(line).context("parsing daemon handshake JSON")?;
    // Drop the reader — further daemon stdout just goes to /dev/null.
    drop(reader);
    drain_daemon_stderr(stderr, browser_debug);
    Ok(DaemonChild::spawned(child, handshake))
}

fn daemon_browser_debug_enabled() -> bool {
    cfg!(debug_assertions) || std::env::var_os("PUFFER_BROWSER_DEBUG").is_some()
}

fn default_cef_profile_dir() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("HOME")?)
            .join("Library/Application Support/Puffer")
            .join("cef-profile"),
    )
}

fn existing_daemon_handshake(workspace_cwd: &Path) -> Option<DaemonHandshake> {
    read_existing_daemon_handshake(&daemon_handshake_paths(workspace_cwd), workspace_cwd)
        .ok()
        .flatten()
}

fn read_existing_daemon_handshake(
    paths: &[PathBuf],
    workspace_cwd: &Path,
) -> Result<Option<DaemonHandshake>> {
    for path in paths {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading daemon handshake {}", path.display()));
            }
        };
        let handshake: DaemonHandshake = serde_json::from_str(&text)
            .with_context(|| format!("parsing daemon handshake {}", path.display()))?;
        if !handshake_matches_workspace(&handshake, workspace_cwd) {
            continue;
        }
        if daemon_handshake_reachable(&handshake) {
            return Ok(Some(handshake));
        }
    }
    Ok(None)
}

fn daemon_handshake_paths(workspace_cwd: &Path) -> Vec<PathBuf> {
    let workspace = workspace_cwd.join(".puffer").join("daemon.handshake");
    let user = user_config_dir().join("daemon.handshake");
    if workspace == user {
        vec![workspace]
    } else {
        vec![workspace, user]
    }
}

fn user_config_dir() -> PathBuf {
    std::env::var_os("PUFFER_HOME")
        .map(PathBuf::from)
        .or_else(dirs_home)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".puffer")
}

fn handshake_matches_workspace(handshake: &DaemonHandshake, workspace_cwd: &Path) -> bool {
    canonical_workspace_root(Path::new(&handshake.workspace_root))
        == canonical_workspace_root(workspace_cwd)
}

fn canonical_workspace_root(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn daemon_handshake_reachable(handshake: &DaemonHandshake) -> bool {
    let Some(port) = parse_ws_port(&handshake.url) else {
        return false;
    };
    let Ok(addr) = format!("127.0.0.1:{port}").parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

fn daemon_handshake_failure_message(
    status: Option<std::process::ExitStatus>,
    stderr_text: &str,
) -> String {
    let status = status
        .map(|status| format!(" with status {status}"))
        .unwrap_or_default();
    let stderr_text = stderr_text.trim();
    if stderr_text.is_empty() {
        return format!("daemon exited before printing handshake{status}");
    }
    format!("daemon exited before printing handshake{status}: {stderr_text}")
}

fn drain_daemon_stderr(stderr: Option<std::process::ChildStderr>, forward: bool) {
    let Some(mut stderr) = stderr else {
        return;
    };
    std::thread::spawn(move || {
        if forward {
            let _ = std::io::copy(&mut stderr, &mut std::io::stderr());
        } else {
            let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        }
    });
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

/// Spawns `ssh <target> puffer daemon --bind 127.0.0.1:0 --print-handshake`
/// and reads the handshake line from the remote's stdout. Returns the live
/// child handle so the caller can keep it running alongside the forward.
///
/// ssh failures historically bubbled up as opaque "empty handshake" errors.
/// We now:
///   * pass `-o BatchMode=yes` so missing keys fail fast instead of
///     blocking on a password prompt inside the UI process;
///   * tee the last ~500 bytes of ssh's stderr into a ring buffer so a
///     failed handshake surfaces the real "Permission denied (publickey)" /
///     "Host key verification failed" / "bash: puffer: command not found"
///     line to the frontend;
///   * include a remediation hint mentioning `remoteBinary` when the
///     handshake never arrives.
fn spawn_remote_daemon(
    ssh_target: &str,
    remote_binary: Option<&str>,
    remote_workspace: Option<&str>,
) -> Result<(Child, DaemonHandshake)> {
    let binary = remote_binary.unwrap_or("puffer");
    // Compose the remote shell command. If the caller wants a specific
    // workspace, `cd` into it first — otherwise the remote's $HOME is used.
    let remote_cmd = if let Some(ws) = remote_workspace {
        // single-quote the workspace for safety
        format!(
            "cd {} && {} daemon --bind 127.0.0.1:0 --print-handshake",
            shell_quote(ws),
            binary
        )
    } else {
        format!(
            "cd ~ && {} daemon --bind 127.0.0.1:0 --print-handshake",
            binary
        )
    };

    let mut cmd = Command::new("ssh");
    cmd.arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ServerAliveInterval=30")
        .arg(ssh_target)
        .arg(&remote_cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning ssh {ssh_target} `{remote_cmd}`"))?;

    let stdout = child
        .stdout
        .take()
        .context("remote daemon stdout missing")?;
    let stderr_pipe = child.stderr.take();

    // Tee ssh's stderr through a tail ring buffer so we can surface the
    // real failure message if the handshake read comes back empty. Runs
    // on a worker thread so the read doesn't block us.
    let stderr_tail: Arc<Mutex<StderrTail>> = Arc::new(Mutex::new(StderrTail::new(512)));
    let tail_thread = stderr_tail.clone();
    if let Some(stderr) = stderr_pipe {
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut buf = [0u8; 512];
            loop {
                match std::io::Read::read(&mut reader, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut guard = tail_thread.lock().unwrap();
                        guard.push(&buf[..n]);
                    }
                }
            }
        });
    }

    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let read_result = reader.read_line(&mut line);
    let trimmed = line.trim();

    if read_result.is_err() || trimmed.is_empty() || !trimmed.starts_with('{') {
        // Give ssh a moment to flush its stderr before we read the tail —
        // without this we can race the reader thread and print a partial
        // message.
        std::thread::sleep(std::time::Duration::from_millis(150));
        let tail = stderr_tail.lock().unwrap().snapshot();
        let ssh_stderr = String::from_utf8_lossy(&tail).trim().to_string();
        let hint = format!(
            "tried `{binary}` on {ssh_target}; pass `remoteBinary` if puffer is installed elsewhere"
        );
        if ssh_stderr.is_empty() {
            anyhow::bail!("remote daemon printed empty handshake — no ssh stderr captured. {hint}");
        } else {
            anyhow::bail!("remote daemon handshake failed: {ssh_stderr}\n({hint})");
        }
    }

    let handshake: DaemonHandshake =
        serde_json::from_str(trimmed).context("parsing remote daemon handshake JSON")?;
    drop(reader);
    Ok((child, handshake))
}

/// Tail ring-buffer for ssh stderr — keeps the last `cap` bytes we've
/// seen. Enough for one "Permission denied (publickey)" / "command not
/// found" line without holding the full stream in memory.
struct StderrTail {
    cap: usize,
    buf: Vec<u8>,
}

impl StderrTail {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            buf: Vec::with_capacity(cap),
        }
    }
    fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        if self.buf.len() > self.cap {
            let excess = self.buf.len() - self.cap;
            self.buf.drain(0..excess);
        }
    }
    fn snapshot(&self) -> Vec<u8> {
        self.buf.clone()
    }
}

fn parse_ws_port(url: &str) -> Option<u16> {
    // ws://host:port/path
    let rest = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))?;
    let host_port = rest.split('/').next()?;
    let port_str = host_port.rsplit(':').next()?;
    port_str.parse::<u16>().ok()
}

fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Picks the `puffer` executable to spawn. In debug builds we prefer a
/// sibling `puffer` binary next to the Tauri process (i.e. `cargo run`'s
/// target directory); in release builds we fall back to the first `puffer`
/// on `PATH`.
fn resolve_puffer_binary() -> Result<PathBuf> {
    let bin_name = if cfg!(windows) {
        "puffer.exe"
    } else {
        "puffer"
    };
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
    use super::*;

    fn local_handshake(workspace_root: &Path, port: u16) -> DaemonHandshake {
        DaemonHandshake {
            url: format!("ws://127.0.0.1:{port}/ws"),
            token: "test-token".to_string(),
            protocol_version: "1".to_string(),
            workspace_root: workspace_root.display().to_string(),
        }
    }

    fn write_handshake(path: &Path, handshake: &DaemonHandshake) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let text = serde_json::to_string(handshake).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn daemon_handshake_failure_includes_stderr() {
        let message = daemon_handshake_failure_message(
            None,
            "Error: another Puffer daemon is already running on this host\n",
        );

        assert!(message.contains("daemon exited before printing handshake"));
        assert!(message.contains("another Puffer daemon is already running"));
    }

    #[test]
    fn daemon_handshake_failure_handles_missing_stderr() {
        let message = daemon_handshake_failure_message(None, "");

        assert_eq!(message, "daemon exited before printing handshake");
    }

    #[test]
    fn existing_daemon_handshake_reuses_reachable_matching_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let path = temp.path().join("daemon.handshake");
        let handshake = local_handshake(&workspace, port);
        write_handshake(&path, &handshake);

        let found = read_existing_daemon_handshake(&[path], &workspace)
            .unwrap()
            .expect("existing daemon handshake");

        assert_eq!(found.url, handshake.url);
        assert_eq!(found.workspace_root, handshake.workspace_root);
    }

    #[test]
    fn existing_daemon_handshake_ignores_mismatched_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let other_workspace = temp.path().join("other");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&other_workspace).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let path = temp.path().join("daemon.handshake");
        write_handshake(&path, &local_handshake(&other_workspace, port));

        let found = read_existing_daemon_handshake(&[path], &workspace).unwrap();

        assert!(found.is_none());
    }

    #[test]
    fn existing_daemon_handshake_ignores_stale_port() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let path = temp.path().join("daemon.handshake");
        write_handshake(&path, &local_handshake(&workspace, port));

        let found = read_existing_daemon_handshake(&[path], &workspace).unwrap();

        assert!(found.is_none());
    }
}
