use super::{LspFileSync, LspSession, ResolvedLspServer};
use crate::runtime::claude_tools::workflow::lsp_live_diagnostics::{
    clear_all_diagnostics, clear_workspace_diagnostics,
};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const LSP_NOTIFICATION_DRAIN_IDLE: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ManagerKey {
    workspace_root: PathBuf,
    server_id: String,
    command: String,
    args_key: String,
    env_key: String,
    workspace_folder_key: Option<String>,
}

#[derive(Debug)]
struct ManagedLspSession {
    session: LspSession,
    open_files: BTreeMap<String, OpenFileState>,
}

#[derive(Debug, Clone)]
struct OpenFileState {
    version: i64,
    content: String,
}

type SessionMap = BTreeMap<ManagerKey, ManagedLspSession>;

pub(super) fn with_lsp_session<T, F>(
    server: &ResolvedLspServer,
    workspace_root: &Path,
    file_sync: Option<LspFileSync>,
    operation: F,
) -> Result<T>
where
    F: FnOnce(&mut LspSession) -> Result<T>,
{
    let key = ManagerKey {
        workspace_root: workspace_root.to_path_buf(),
        server_id: server.id.clone(),
        command: server.command.clone(),
        args_key: server.args.join("\0"),
        env_key: server
            .env
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("\0"),
        workspace_folder_key: server.workspace_folder.clone(),
    };
    let mut sessions = session_manager()
        .lock()
        .map_err(|_| anyhow::anyhow!("LSP session manager lock poisoned"))?;
    let managed = ensure_session(&mut sessions, key, server, workspace_root)?;
    let should_drain_notifications = file_sync.is_some();
    if let Some(file_sync) = file_sync {
        sync_file(managed, &file_sync)?;
    }
    let result = operation(&mut managed.session)?;
    if should_drain_notifications {
        managed
            .session
            .drain_pending_messages_until_idle(LSP_NOTIFICATION_DRAIN_IDLE)?;
    } else {
        managed.session.drain_pending_messages()?;
    }
    Ok(result)
}

pub(super) fn shutdown_all_lsp_sessions() -> Result<()> {
    let mut sessions = session_manager()
        .lock()
        .map_err(|_| anyhow::anyhow!("LSP session manager lock poisoned"))?;
    let mut first_error = None;
    for (key, managed) in sessions.iter_mut() {
        if let Err(error) = managed.session.shutdown() {
            first_error = first_error.or_else(|| {
                Some(anyhow::anyhow!(
                    "failed to shut down LSP session for {}: {error}",
                    key.workspace_root.display()
                ))
            });
        }
    }
    sessions.clear();
    clear_all_diagnostics()?;
    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(())
}

fn session_manager() -> &'static Mutex<SessionMap> {
    static MANAGER: OnceLock<Mutex<SessionMap>> = OnceLock::new();
    MANAGER.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn ensure_session<'a>(
    sessions: &'a mut SessionMap,
    key: ManagerKey,
    server: &ResolvedLspServer,
    workspace_root: &Path,
) -> Result<&'a mut ManagedLspSession> {
    let needs_restart = sessions
        .get_mut(&key)
        .map(|managed| managed.session.has_exited())
        .unwrap_or(false);
    if needs_restart {
        let _ = clear_workspace_diagnostics(&key.workspace_root);
        sessions.remove(&key);
    }
    if !sessions.contains_key(&key) {
        let mut session = LspSession::start(server, workspace_root)?;
        session.initialize(workspace_root)?;
        sessions.insert(
            key.clone(),
            ManagedLspSession {
                session,
                open_files: BTreeMap::new(),
            },
        );
    }
    sessions.get_mut(&key).with_context(|| {
        format!(
            "missing managed LSP session for {}",
            workspace_root.display()
        )
    })
}

fn sync_file(managed: &mut ManagedLspSession, file_sync: &LspFileSync) -> Result<()> {
    match managed.open_files.get_mut(&file_sync.file_uri) {
        Some(state) => {
            if state.content != file_sync.content {
                state.version += 1;
                managed.session.change_file(
                    &file_sync.file_uri,
                    state.version,
                    &file_sync.content,
                )?;
                state.content = file_sync.content.clone();
            }
        }
        None => {
            managed.session.open_file(
                &file_sync.file_uri,
                &file_sync.language_id,
                &file_sync.content,
            )?;
            managed.open_files.insert(
                file_sync.file_uri.clone(),
                OpenFileState {
                    version: 1,
                    content: file_sync.content.clone(),
                },
            );
        }
    }
    Ok(())
}
