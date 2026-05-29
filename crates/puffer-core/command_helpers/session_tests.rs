use super::handle_memory_command;
use crate::AppState;
use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
use puffer_session_store::SessionStore;
use tempfile::tempdir;

#[test]
fn memory_open_project_initializes_project_memory() {
    let _guard = crate::test_locks::env_lock().lock().unwrap();
    let temp = tempdir().unwrap();
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace/demo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let _home = puffer_config::set_puffer_home_override(&home);
    let paths = ConfigPaths::discover(&cwd);
    ensure_workspace_dirs(&paths).unwrap();
    let store = SessionStore::from_paths(&paths).unwrap();
    let session = store.create_session(cwd.clone()).unwrap();
    let mut state = AppState::new(PufferConfig::default(), cwd, session);

    handle_memory_command(&mut state, &store, "open project").unwrap();

    let context = state
        .project_memory
        .as_ref()
        .expect("project memory should be active");
    assert!(context.memory_file.ends_with("MEMORY.md"));
    assert!(context.memory_file.exists());
    assert!(paths.projects_file().exists());
}
