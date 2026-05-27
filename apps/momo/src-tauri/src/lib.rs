mod backend;
mod codex_app_server;
mod connectors;
mod dtos;
mod events;
mod oauth_listener;
mod websocket;

use std::sync::Arc;
use tauri::Builder;

use crate::backend::BackendState;

pub fn run() {
    let backend = Arc::new(BackendState::new());
    websocket::start_backend_ws(backend.clone());

    Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            oauth_listener::start(app.handle().clone());
            Ok(())
        })
        .manage(backend)
        .invoke_handler(tauri::generate_handler![])
        .run(tauri::generate_context!())
        .expect("error while running momo application");
}
