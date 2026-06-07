mod backend;
mod browser;
mod browser_debug;
mod cef_host;
mod chat_attachments;
mod codex_app_server;
mod daemon_launcher;
mod dtos;
mod events;
mod files;
mod fs_watch;
mod local_model;
mod lsp;
mod mini_window;
mod minicpm5;
mod pty;
mod remote_client;
mod repo_actions;
mod websocket;

use backend::BackendState;
use daemon_launcher::DaemonLauncher;
use events::EventEmitter;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Builder, State};

type SharedBackend = Arc<BackendState>;
type SharedDaemonLauncher = Arc<DaemonLauncher>;

#[cfg(test)]
const REGISTERED_TAURI_COMMANDS: &[&str] = &[
    "backend_request",
    "list_grouped_sessions",
    "load_session_detail",
    "refresh_repo_status",
    "create_pull_request",
    "merge_pull_request",
    "load_settings_snapshot",
    "login_with_oauth",
    "login_with_api_key",
    "logout_provider",
    "list_external_credentials",
    "import_external_credential",
    "run_remote_bash",
    "read_remote_file",
    "write_remote_file",
    "ensure_local_daemon",
    "restart_local_daemon",
    "start_ssh_daemon",
    "stage_chat_attachment",
    "read_chat_attachment_preview",
    "run_agent_turn",
    "resolve_permission",
    "resolve_user_question",
    "cancel_turn",
    "summon_mini_window",
    "minicpm5_recommend",
    "minicpm5_install",
    "browser_cef_native_status",
    "browser_cef_native_open",
    "browser_cef_native_resize",
    "browser_cef_native_navigate",
    "browser_cef_native_state",
    "browser_cef_native_reload",
    "browser_cef_native_history",
    "browser_cef_native_close",
    "browser_cef_native_hide",
];

fn backend_call(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    state
        .handle(EventEmitter::new(app), method, params)
        .map_err(|error| error.to_string())
}

fn json_value<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

fn required_remote_target(remote_target: String) -> Result<String, String> {
    let trimmed = remote_target.trim();
    if trimmed.is_empty() {
        return Err("remote target is required".to_string());
    }
    Ok(trimmed.to_string())
}

#[tauri::command]
fn backend_request(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    method: String,
    params: Value,
) -> Result<Value, String> {
    backend_call(app, state, &method, params)
}

#[tauri::command]
fn list_grouped_sessions(app: AppHandle, state: State<'_, SharedBackend>) -> Result<Value, String> {
    backend_call(app, state, "list_grouped_sessions", json!({}))
}

#[tauri::command]
fn load_session_detail(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    session_id: String,
) -> Result<Value, String> {
    backend_call(
        app,
        state,
        "load_session_detail",
        json!({ "sessionId": session_id }),
    )
}

#[tauri::command]
fn refresh_repo_status(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    session_id: String,
) -> Result<Value, String> {
    backend_call(
        app,
        state,
        "refresh_repo_status",
        json!({ "sessionId": session_id }),
    )
}

#[tauri::command]
fn create_pull_request(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    session_id: String,
    title: Option<String>,
    body: Option<String>,
) -> Result<Value, String> {
    backend_call(
        app,
        state,
        "create_pull_request",
        json!({ "sessionId": session_id, "title": title, "body": body }),
    )
}

#[tauri::command]
fn merge_pull_request(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    session_id: String,
    pull_request_number: Option<u64>,
    merge_method: Option<String>,
) -> Result<Value, String> {
    backend_call(
        app,
        state,
        "merge_pull_request",
        json!({
            "sessionId": session_id,
            "pullRequestNumber": pull_request_number,
            "mergeMethod": merge_method,
        }),
    )
}

#[tauri::command]
fn load_settings_snapshot(
    app: AppHandle,
    state: State<'_, SharedBackend>,
) -> Result<Value, String> {
    backend_call(app, state, "load_settings_snapshot", json!({}))
}

#[tauri::command]
fn login_with_oauth(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    provider_id: String,
) -> Result<Value, String> {
    backend_call(
        app,
        state,
        "login_with_oauth",
        json!({ "providerId": provider_id }),
    )
}

#[tauri::command]
fn login_with_api_key(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    provider_id: String,
    api_key: String,
) -> Result<Value, String> {
    backend_call(
        app,
        state,
        "login_with_api_key",
        json!({ "providerId": provider_id, "apiKey": api_key }),
    )
}

#[tauri::command]
fn logout_provider(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    provider_id: String,
) -> Result<Value, String> {
    backend_call(
        app,
        state,
        "logout_provider",
        json!({ "providerId": provider_id }),
    )
}

#[tauri::command]
fn list_external_credentials(
    app: AppHandle,
    state: State<'_, SharedBackend>,
) -> Result<Value, String> {
    backend_call(app, state, "list_external_credentials", json!({}))
}

#[tauri::command]
fn import_external_credential(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    provider_id: String,
    source: String,
) -> Result<Value, String> {
    backend_call(
        app,
        state,
        "import_external_credential",
        json!({ "providerId": provider_id, "source": source }),
    )
}

#[tauri::command]
fn run_remote_bash(
    remote_target: String,
    remote_cwd: Option<String>,
    remote_password: Option<String>,
    command: String,
) -> Result<Value, String> {
    let target = required_remote_target(remote_target)?;
    json_value(
        remote_client::run_remote_shell(
            &target,
            remote_cwd.as_deref(),
            remote_password.as_deref(),
            &command,
        )
        .map_err(|error| error.to_string())?,
    )
}

#[tauri::command]
fn read_remote_file(
    remote_target: String,
    remote_cwd: Option<String>,
    remote_password: Option<String>,
    path: String,
) -> Result<Value, String> {
    let target = required_remote_target(remote_target)?;
    json_value(
        remote_client::read_remote_file(
            &target,
            remote_cwd.as_deref(),
            remote_password.as_deref(),
            &path,
        )
        .map_err(|error| error.to_string())?,
    )
}

#[tauri::command]
fn write_remote_file(
    remote_target: String,
    remote_cwd: Option<String>,
    remote_password: Option<String>,
    path: String,
    contents_base64: String,
) -> Result<Value, String> {
    let target = required_remote_target(remote_target)?;
    json_value(
        remote_client::write_remote_file(
            &target,
            remote_cwd.as_deref(),
            remote_password.as_deref(),
            &path,
            &contents_base64,
        )
        .map_err(|error| error.to_string())?,
    )
}

#[tauri::command]
fn ensure_local_daemon(
    launcher: State<'_, SharedDaemonLauncher>,
) -> Result<daemon_launcher::DaemonHandshake, String> {
    launcher.ensure_started().map_err(|error| error.to_string())
}

#[tauri::command]
fn restart_local_daemon(
    launcher: State<'_, SharedDaemonLauncher>,
    cwd: String,
) -> Result<daemon_launcher::DaemonHandshake, String> {
    launcher
        .restart_local(PathBuf::from(cwd))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn start_ssh_daemon(
    launcher: State<'_, SharedDaemonLauncher>,
    ssh_target: String,
    remote_binary: Option<String>,
    remote_workspace: Option<String>,
) -> Result<daemon_launcher::DaemonHandshake, String> {
    launcher
        .start_ssh(
            &ssh_target,
            remote_binary.as_deref(),
            remote_workspace.as_deref(),
        )
        .map_err(|error| error.to_string())
}

fn agent_turn_payload(
    session_id: String,
    message: String,
    provider_id: Option<String>,
    model_id: Option<String>,
    fast_mode: Option<bool>,
    permission_mode: Option<String>,
    mode: Option<String>,
    attachment_ids: Option<Vec<String>>,
) -> Value {
    let mut payload = json!({
        "sessionId": session_id,
        "message": message,
        "providerId": provider_id,
        "modelId": model_id,
        "fastMode": fast_mode.unwrap_or(false),
        "permissionMode": permission_mode,
        "mode": mode,
    });
    if let Some(attachment_ids) = attachment_ids {
        payload["attachmentIds"] = json!(attachment_ids);
    }
    payload
}

#[tauri::command]
fn run_agent_turn(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    session_id: String,
    message: String,
    provider_id: Option<String>,
    model_id: Option<String>,
    fast_mode: Option<bool>,
    permission_mode: Option<String>,
    mode: Option<String>,
    attachment_ids: Option<Vec<String>>,
) -> Result<String, String> {
    let payload = agent_turn_payload(
        session_id,
        message,
        provider_id,
        model_id,
        fast_mode,
        permission_mode,
        mode,
        attachment_ids,
    );
    let value = backend_call(app, state, "run_agent_turn", payload)?;
    Ok(value
        .get("turnId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

#[tauri::command]
fn resolve_permission(
    _turn_id: String,
    _request_id: String,
    _action: String,
) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn resolve_user_question(
    _turn_id: String,
    _request_id: String,
    _answers: Value,
    _annotations: Value,
) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn cancel_turn(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    turn_id: String,
) -> Result<(), String> {
    backend_call(app, state, "cancel_turn", json!({ "turnId": turn_id })).map(|_| ())
}

pub fn run() {
    let backend = Arc::new(BackendState::new());
    let launcher = Arc::new(DaemonLauncher::new());
    websocket::start_backend_ws(backend.clone());

    // Cmd+Shift+Space (macOS) / Ctrl+Shift+Space (Windows/Linux) summons the
    // mini floating window. Avoids Cmd+Space (Spotlight). Same chord hides it.
    use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};
    #[cfg(target_os = "macos")]
    let primary_modifier = Modifiers::SUPER;
    #[cfg(not(target_os = "macos"))]
    let primary_modifier = Modifiers::CONTROL;
    let mini_shortcut = Shortcut::new(Some(primary_modifier | Modifiers::SHIFT), Code::Space);

    Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            // Register the shortcut in setup() (below) instead of here so an OS
            // registration failure (e.g. another app owns the chord) doesn't
            // abort startup — the handler is harmless until something registers.
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if shortcut == &mini_shortcut && event.state() == ShortcutState::Pressed {
                        mini_window::toggle_mini_window(app);
                    }
                })
                .build(),
        )
        .setup(move |app| {
            use tauri_plugin_global_shortcut::GlobalShortcutExt;
            if let Err(err) = app.global_shortcut().register(mini_shortcut) {
                eprintln!("mini-window shortcut unavailable (continuing without it): {err}");
            }
            Ok(())
        })
        .manage(backend)
        .manage(launcher)
        .invoke_handler(tauri::generate_handler![
            backend_request,
            list_grouped_sessions,
            load_session_detail,
            refresh_repo_status,
            create_pull_request,
            merge_pull_request,
            load_settings_snapshot,
            login_with_oauth,
            login_with_api_key,
            logout_provider,
            list_external_credentials,
            import_external_credential,
            run_remote_bash,
            read_remote_file,
            write_remote_file,
            ensure_local_daemon,
            restart_local_daemon,
            start_ssh_daemon,
            chat_attachments::stage_chat_attachment,
            chat_attachments::read_chat_attachment_preview,
            run_agent_turn,
            resolve_permission,
            resolve_user_question,
            cancel_turn,
            mini_window::summon_mini_window,
            minicpm5::minicpm5_recommend,
            minicpm5::minicpm5_install,
            cef_host::browser_cef_native_status,
            cef_host::browser_cef_native_open,
            cef_host::browser_cef_native_resize,
            cef_host::browser_cef_native_navigate,
            cef_host::browser_cef_native_state,
            cef_host::browser_cef_native_reload,
            cef_host::browser_cef_native_history,
            cef_host::browser_cef_native_close,
            cef_host::browser_cef_native_hide,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Corbina desktop");
}

#[cfg(test)]
mod tests {
    use super::REGISTERED_TAURI_COMMANDS;
    use serde_json::json;
    use std::collections::BTreeSet;

    fn direct_invoke_commands(source: &str) -> BTreeSet<String> {
        let mut commands = BTreeSet::new();
        let mut offset = 0;
        while let Some(found) = source[offset..].find("invoke") {
            let invoke_at = offset + found;
            let Some(open_paren_at) = source[invoke_at..].find('(').map(|idx| invoke_at + idx)
            else {
                break;
            };
            let mut cursor = open_paren_at + 1;
            while let Some(ch) = source[cursor..].chars().next() {
                if !ch.is_whitespace() {
                    break;
                }
                cursor += ch.len_utf8();
            }
            let Some(quote) = source[cursor..].chars().next() else {
                break;
            };
            if quote == '"' || quote == '\'' {
                cursor += quote.len_utf8();
                if let Some(end) = source[cursor..].find(quote) {
                    commands.insert(source[cursor..cursor + end].to_string());
                    offset = cursor + end + quote.len_utf8();
                    continue;
                }
            }
            offset = open_paren_at + 1;
        }
        commands
    }

    #[test]
    fn direct_frontend_invokes_have_registered_tauri_commands() {
        let mut invoked = direct_invoke_commands(include_str!("../../src/lib/api/desktop.ts"));
        invoked.extend(direct_invoke_commands(include_str!(
            "../../src/lib/api/daemonClient.ts"
        )));
        let registered = REGISTERED_TAURI_COMMANDS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let missing = invoked
            .iter()
            .filter(|command| !registered.contains(command.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "frontend invokes missing Tauri command registration: {missing:?}"
        );
    }

    #[test]
    fn registered_tauri_commands_include_chat_attachment_storage() {
        let registered = REGISTERED_TAURI_COMMANDS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();

        assert!(registered.contains("stage_chat_attachment"));
        assert!(registered.contains("read_chat_attachment_preview"));
    }

    #[test]
    fn run_agent_turn_payload_forwards_attachment_ids_only() {
        let payload = super::agent_turn_payload(
            "session-1".to_string(),
            "hello".to_string(),
            None,
            None,
            None,
            None,
            None,
            Some(vec!["attachment-1".to_string(), "attachment-2".to_string()]),
        );

        assert_eq!(
            payload["attachmentIds"],
            json!(["attachment-1", "attachment-2"])
        );
        assert!(payload.get("attachments").is_none());
    }

    #[test]
    fn remote_scratchpad_rejects_empty_target() {
        assert_eq!(
            super::required_remote_target("  ".to_string()).unwrap_err(),
            "remote target is required"
        );
    }
}
