//! PTY registry — one user-facing shell per `pty_open` RPC.
//!
//! The desktop Terminal tab uses this to drop the user into a live shell
//! rooted at the session's cwd (think "ssh into the session dir"). The
//! agent's own tool-call terminal output stays separate; this is the human
//! escape hatch.
//!
//! Data flow:
//!   pty_open  → spawn $SHELL (or /bin/bash, /bin/sh) in cwd; return pty_id
//!   reader thread → broadcast `pty:<id>:data` events with base64 payloads
//!   pty_write → write base64-decoded bytes to the master
//!   pty_resize → update PtySize
//!   pty_close → kill child + drop handle
//!
//! base64 keeps NDJSON frames ASCII-safe regardless of what the shell
//! emits (control bytes, non-UTF-8, etc.).

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::events::EventEmitter;

const PTY_IDLE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const PTY_REPLAY_MAX_BYTES: usize = 1024 * 1024;
const PTY_WRITE_MAX_BYTES: usize = 64 * 1024;

/// Owns a single live PTY. Dropping this kills the child and joins the
/// reader thread implicitly via the Child/master drops.
pub struct PtyHandle {
    session_id: String,
    title: String,
    cwd: String,
    cols: u16,
    rows: u16,
    created_at_ms: i64,
    last_active: Arc<Mutex<Instant>>,
    replay: Arc<Mutex<PtyReplay>>,
    /// Master side — we write user keystrokes here. The reader thread owns
    /// its own `try_clone_reader()` so we don't need to share the reader.
    master: Box<dyn MasterPty + Send>,
    /// Persistent writer for the master side. This must live for the full
    /// PTY lifetime; recreating and dropping it per keystroke can close
    /// stdin and make interactive shells treat one character as EOF.
    writer: Box<dyn Write + Send>,
    /// Cloned killer handle — the live Child lives inside the reader
    /// thread (so `wait()` can drive it) and this is our remote SIGKILL
    /// pathway from `close()`.
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Reader thread. Joined opportunistically when close() runs so we
    /// don't leak threads; ignored if the child is still alive (close
    /// will drive it to exit by killing the child first).
    reader: Option<JoinHandle<()>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyTabInfo {
    pub(crate) pty_id: String,
    pub(crate) session_id: String,
    pub(crate) title: String,
    pub(crate) cwd: String,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) created_at_ms: i64,
    pub(crate) active: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtySessionInfo {
    pub(crate) tabs: Vec<PtyTabInfo>,
    pub(crate) initialized: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyReplayChunk {
    pub(crate) seq: u64,
    pub(crate) data: String,
    #[serde(skip)]
    bytes: usize,
}

#[derive(Default)]
struct PtyReplay {
    chunks: VecDeque<PtyReplayChunk>,
    bytes: usize,
}

struct PtyRegistryInner {
    handles: HashMap<String, PtyHandle>,
    active_by_session: HashMap<String, String>,
    known_sessions: HashSet<String>,
}

/// Thread-safe map of pty_id → live handle. The map is stored inside
/// `DaemonState` as `Arc<PtyRegistry>` so the RPC handlers (sync) and the
/// reader thread (also sync, via broadcast::Sender) share one registry.
pub struct PtyRegistry {
    inner: Mutex<PtyRegistryInner>,
}

impl PtyRegistry {
    /// Creates an empty PTY registry.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(PtyRegistryInner {
                handles: HashMap::new(),
                active_by_session: HashMap::new(),
                known_sessions: HashSet::new(),
            }),
        }
    }

    /// Starts the idle pruner for this registry.
    pub fn spawn_idle_pruner(self: &Arc<Self>) {
        let registry = Arc::clone(self);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            registry.close_idle(PTY_IDLE_TIMEOUT);
        });
    }

    /// Spawn a shell in `cwd` and register the resulting PTY. Returns the
    /// opaque pty_id the client uses on subsequent write/resize/close RPCs.
    ///
    /// The reader thread owns a clone of the Tauri event emitter and
    /// streams `pty:<id>:data` events until the child exits, at which
    /// point it emits a final `pty:<id>:exit` event and removes itself
    /// from the registry.
    pub fn open(
        self: &Arc<Self>,
        events: EventEmitter,
        session_id: String,
        cwd: PathBuf,
        cols: u16,
        rows: u16,
        title: Option<String>,
    ) -> Result<String> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;

        let shell = resolve_shell();
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(&cwd);
        // Inherit env — users expect $PATH, $HOME, $TERM (we override TERM
        // to xterm-256color since xterm.js is our renderer).
        for (k, v) in std::env::vars_os() {
            cmd.env(k, v);
        }
        cmd.env("TERM", "xterm-256color");

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawn shell {shell}"))?;

        // The slave handle is no longer needed once the child has inherited
        // its file descriptors. Dropping it here avoids keeping an extra
        // reference that would delay PTY teardown.
        drop(pair.slave);

        let pty_id = Uuid::new_v4().to_string();
        let title = title
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Terminal".to_string());
        let cwd_string = cwd.display().to_string();
        let replay = Arc::new(Mutex::new(PtyReplay::default()));
        let next_seq = Arc::new(Mutex::new(0u64));
        let last_active = Arc::new(Mutex::new(Instant::now()));

        let mut reader = pair.master.try_clone_reader().context("clone pty reader")?;
        let writer = pair.master.take_writer().context("take pty writer")?;

        // `Child` isn't Clone, but the cloned killer is Send + Sync and
        // can SIGKILL independently. The live Child moves into the reader
        // thread, which is the only place that blocks on `wait()`.
        let killer = child.clone_killer();

        let events_for_reader = events.clone();
        let id_for_reader = pty_id.clone();
        let registry_for_reader = Arc::clone(self);
        let replay_for_reader = Arc::clone(&replay);
        let seq_for_reader = Arc::clone(&next_seq);

        let reader_handle = std::thread::spawn(move || {
            let data_channel = format!("pty:{id_for_reader}:data");
            let exit_channel = format!("pty:{id_for_reader}:exit");
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF — child closed its end
                    Ok(n) => {
                        let encoded = BASE64.encode(&buf[..n]);
                        let seq = {
                            let mut guard = seq_for_reader.lock().unwrap();
                            *guard += 1;
                            *guard
                        };
                        replay_for_reader
                            .lock()
                            .unwrap()
                            .push(seq, encoded.clone(), n);
                        // Best-effort: if no one's listening, drop the
                        // frame. Keeps the reader thread from deadlocking
                        // on a disconnected UI.
                        events_for_reader
                            .emit(data_channel.clone(), json!({ "data": encoded, "seq": seq }));
                    }
                    Err(e) => {
                        // EIO is the normal "pty closed" error on Linux
                        // after the child exits; treat all read errors as
                        // terminal.
                        let _ = e;
                        break;
                    }
                }
            }

            let exit_code = match child.wait() {
                Ok(status) => status.exit_code() as i64,
                Err(_) => -1,
            };
            events_for_reader.emit(exit_channel, json!({ "exitCode": exit_code }));

            // Drop our handle from the registry so memory doesn't grow
            // unbounded across many terminal-tab lifecycles.
            let mut guard = registry_for_reader.inner.lock().unwrap();
            if let Some(handle) = guard.handles.remove(&id_for_reader) {
                if guard
                    .active_by_session
                    .get(&handle.session_id)
                    .is_some_and(|active| active == &id_for_reader)
                {
                    guard.active_by_session.remove(&handle.session_id);
                }
            }
        });

        let handle = PtyHandle {
            session_id: session_id.clone(),
            title,
            cwd: cwd_string,
            cols,
            rows,
            created_at_ms: now_ms(),
            last_active,
            replay,
            master: pair.master,
            writer,
            killer,
            reader: Some(reader_handle),
        };
        let mut guard = self.inner.lock().unwrap();
        guard.known_sessions.insert(session_id.clone());
        guard.active_by_session.insert(session_id, pty_id.clone());
        guard.handles.insert(pty_id.clone(), handle);
        Ok(pty_id)
    }

    /// Lists live PTYs for one agent session.
    pub fn list(&self, session_id: &str) -> PtySessionInfo {
        let mut guard = self.inner.lock().unwrap();
        let active_id = guard.active_by_session.get(session_id).cloned();
        let mut tabs = guard
            .handles
            .iter_mut()
            .filter_map(|(pty_id, handle)| {
                (handle.session_id == session_id).then(|| {
                    handle.touch();
                    handle.info(pty_id, active_id.as_deref() == Some(pty_id))
                })
            })
            .collect::<Vec<_>>();
        tabs.sort_by_key(|tab| tab.created_at_ms);
        PtySessionInfo {
            tabs,
            initialized: guard.known_sessions.contains(session_id),
        }
    }

    /// Marks one live PTY as the active tab for its session.
    pub fn focus(&self, pty_id: &str) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        let session_id = {
            let handle = guard
                .handles
                .get_mut(pty_id)
                .with_context(|| format!("no pty `{pty_id}`"))?;
            handle.touch();
            handle.session_id.clone()
        };
        guard
            .active_by_session
            .insert(session_id, pty_id.to_string());
        Ok(())
    }

    /// Returns replay chunks for a live PTY.
    pub fn replay(&self, pty_id: &str) -> Result<Vec<PtyReplayChunk>> {
        let guard = self.inner.lock().unwrap();
        let handle = guard
            .handles
            .get(pty_id)
            .with_context(|| format!("no pty `{pty_id}`"))?;
        handle.touch();
        let chunks = handle
            .replay
            .lock()
            .unwrap()
            .chunks
            .iter()
            .cloned()
            .collect();
        Ok(chunks)
    }

    /// Renames a live PTY tab.
    pub fn rename(&self, pty_id: &str, title: String) -> Result<PtyTabInfo> {
        let mut guard = self.inner.lock().unwrap();
        let active_id = guard
            .handles
            .get(pty_id)
            .map(|handle| {
                guard
                    .active_by_session
                    .get(&handle.session_id)
                    .is_some_and(|active| active == pty_id)
            })
            .unwrap_or(false);
        let handle = guard
            .handles
            .get_mut(pty_id)
            .with_context(|| format!("no pty `{pty_id}`"))?;
        let trimmed = title.trim();
        if !trimmed.is_empty() {
            handle.title = trimmed.to_string();
        }
        handle.touch();
        Ok(handle.info(pty_id, active_id))
    }

    /// Writes raw base64-encoded bytes to a live PTY.
    pub fn write(&self, pty_id: &str, data_b64: &str) -> Result<()> {
        let bytes = decode_pty_write_payload(data_b64)?;
        let mut guard = self.inner.lock().unwrap();
        let handle = guard
            .handles
            .get_mut(pty_id)
            .with_context(|| format!("no pty `{pty_id}`"))?;
        handle.touch();
        handle.writer.write_all(&bytes).context("write to pty")?;
        handle.writer.flush().ok();
        Ok(())
    }

    /// Resizes a live PTY.
    pub fn resize(&self, pty_id: &str, cols: u16, rows: u16) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        let handle = guard
            .handles
            .get_mut(pty_id)
            .with_context(|| format!("no pty `{pty_id}`"))?;
        handle.touch();
        handle.cols = cols;
        handle.rows = rows;
        handle
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resize pty")?;
        Ok(())
    }

    /// Closes a live PTY. Missing ids are treated as already closed.
    pub fn close(&self, pty_id: &str) -> Result<()> {
        let mut handle = {
            let mut guard = self.inner.lock().unwrap();
            match guard.handles.remove(pty_id) {
                Some(h) => {
                    if guard
                        .active_by_session
                        .get(&h.session_id)
                        .is_some_and(|active| active == pty_id)
                    {
                        guard.active_by_session.remove(&h.session_id);
                    }
                    h
                }
                None => return Ok(()), // idempotent
            }
        };
        close_handle(&mut handle);
        Ok(())
    }

    fn close_idle(&self, timeout: Duration) {
        let mut stale = {
            let mut guard = self.inner.lock().unwrap();
            let stale_ids = guard
                .handles
                .iter()
                .filter_map(|(pty_id, handle)| {
                    (handle.idle_for() >= timeout).then(|| pty_id.clone())
                })
                .collect::<Vec<_>>();
            let mut removed = Vec::new();
            for pty_id in stale_ids {
                if let Some(handle) = guard.handles.remove(&pty_id) {
                    if guard
                        .active_by_session
                        .get(&handle.session_id)
                        .is_some_and(|active| active == &pty_id)
                    {
                        guard.active_by_session.remove(&handle.session_id);
                    }
                    removed.push(handle);
                }
            }
            removed
        };
        for handle in &mut stale {
            close_handle(handle);
        }
    }
}

impl PtyHandle {
    fn touch(&self) {
        *self.last_active.lock().unwrap() = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_active.lock().unwrap().elapsed()
    }

    fn info(&self, pty_id: &str, active: bool) -> PtyTabInfo {
        PtyTabInfo {
            pty_id: pty_id.to_string(),
            session_id: self.session_id.clone(),
            title: self.title.clone(),
            cwd: self.cwd.clone(),
            cols: self.cols,
            rows: self.rows,
            created_at_ms: self.created_at_ms,
            active,
        }
    }
}

fn decode_pty_write_payload(data_b64: &str) -> Result<Vec<u8>> {
    let bytes = BASE64
        .decode(data_b64.as_bytes())
        .context("decode pty data")?;
    if bytes.len() > PTY_WRITE_MAX_BYTES {
        anyhow::bail!(
            "pty write payload is too large ({} bytes, hard limit {} bytes)",
            bytes.len(),
            PTY_WRITE_MAX_BYTES
        );
    }
    Ok(bytes)
}

impl PtyReplay {
    fn push(&mut self, seq: u64, data: String, bytes: usize) {
        self.bytes += bytes;
        self.chunks.push_back(PtyReplayChunk { seq, data, bytes });
        while self.bytes > PTY_REPLAY_MAX_BYTES {
            if let Some(chunk) = self.chunks.pop_front() {
                self.bytes = self.bytes.saturating_sub(chunk.bytes);
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_pty_write_payload_rejects_large_input() {
        let encoded = BASE64.encode(vec![b'x'; PTY_WRITE_MAX_BYTES + 1]);

        let error = decode_pty_write_payload(&encoded).unwrap_err();

        assert!(error.to_string().contains("too large"));
    }
}

fn close_handle(handle: &mut PtyHandle) {
    let _ = handle.killer.kill();
    let _ = handle.reader.take();
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// Pick the shell to spawn. Prefer the user's $SHELL, then bash, then sh.
/// Resolves the shell to spawn in a user-facing PTY. Honors $SHELL first,
/// then falls back to platform-appropriate defaults so the Terminal tab
/// works out-of-the-box on macOS, Linux, and Windows.
fn resolve_shell() -> String {
    if let Ok(sh) = std::env::var("SHELL") {
        if !sh.trim().is_empty() {
            return sh;
        }
    }
    #[cfg(windows)]
    {
        if let Ok(comspec) = std::env::var("COMSPEC") {
            if !comspec.trim().is_empty() {
                return comspec;
            }
        }
        for candidate in [
            r"C:\Windows\System32\cmd.exe",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        ] {
            if std::path::Path::new(candidate).exists() {
                return candidate.to_string();
            }
        }
        return "cmd.exe".to_string();
    }
    #[cfg(not(windows))]
    {
        for candidate in ["/bin/bash", "/bin/sh"] {
            if std::path::Path::new(candidate).exists() {
                return candidate.to_string();
            }
        }
        "/bin/sh".to_string()
    }
}
