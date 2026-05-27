use tauri::Builder;

pub fn run() {
    Builder::default()
        .plugin(tauri_plugin_opener::init())
        .run(tauri::generate_context!())
        .expect("error while running momo application");
}
