mod backend;
mod browser;
mod codex_app_server;
mod dtos;
mod events;
mod files;
mod fs_watch;
mod lsp;
mod pty;
mod repo_actions;
mod websocket;

use backend::BackendState;
use events::EventEmitter;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Builder, State};

type SharedBackend = Arc<BackendState>;

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
    app: AppHandle,
    state: State<'_, SharedBackend>,
    command: String,
) -> Result<Value, String> {
    backend_call(app, state, "run_remote_bash", json!({ "command": command }))
}

#[tauri::command]
fn read_remote_file(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    path: String,
) -> Result<Value, String> {
    backend_call(app, state, "read_remote_file", json!({ "path": path }))
}

#[tauri::command]
fn write_remote_file(
    app: AppHandle,
    state: State<'_, SharedBackend>,
    path: String,
    contents_base64: String,
) -> Result<Value, String> {
    backend_call(
        app,
        state,
        "write_remote_file",
        json!({ "path": path, "contentsBase64": contents_base64 }),
    )
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
) -> Result<String, String> {
    let value = backend_call(
        app,
        state,
        "run_agent_turn",
        json!({
            "sessionId": session_id,
            "message": message,
            "providerId": provider_id,
            "modelId": model_id,
            "fastMode": fast_mode.unwrap_or(false),
            "permissionMode": permission_mode,
        }),
    )?;
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
    websocket::start_backend_ws(backend.clone());

    Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(backend)
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
            run_agent_turn,
            resolve_permission,
            resolve_user_question,
            cancel_turn,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Corbina desktop");
}
