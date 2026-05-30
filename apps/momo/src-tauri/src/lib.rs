mod backend;
mod user_profile;
mod codex_app_server;
mod connectors;
mod daemon_launcher;
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

    {
        let backend = backend.clone();
        std::thread::spawn(move || {
            if let Err(error) = backend.ensure_daemon() {
                eprintln!("momo: failed to pre-start puffer daemon: {error:#}");
            }
        });
    }

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
