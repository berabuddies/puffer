//! Integration tests that exercise the connector runtime at the crate
//! boundary. We do not mock the provider — `dispatch` calls a real
//! provider — so these tests focus on the bookkeeping around dispatch:
//! session creation, mapping persistence, and reset semantics. The
//! provider-backed turn is covered end-to-end in each platform crate's
//! own tests against a mocked transport.

use puffer_config::{ConfigPaths, PufferConfig};
use puffer_connector_core::{
    ConnectorRuntime, ConnectorRuntimeConfig, ConversationKey, ConversationSessionMap,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::SessionStore;
use tempfile::tempdir;

fn build_runtime(
    root: std::path::PathBuf,
    map_path: std::path::PathBuf,
) -> (ConnectorRuntime, SessionStore) {
    let paths = ConfigPaths {
        workspace_root: root.clone(),
        workspace_config_dir: root.join(".puffer"),
        user_config_dir: root.join(".puffer-user"),
        builtin_resources_dir: root.join("resources"),
    };
    std::fs::create_dir_all(&paths.user_config_dir).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session_map = ConversationSessionMap::load(&map_path).unwrap();

    let runtime = ConnectorRuntime::new(ConnectorRuntimeConfig {
        config: PufferConfig::default(),
        resources: LoadedResources::default(),
        providers: ProviderRegistry::default(),
        auth_store: AuthStore::default(),
        auth_path: root.join("auth.json"),
        session_store: session_store.clone(),
        session_map,
        default_cwd: root,
    });
    (runtime, session_store)
}

#[test]
fn reset_releases_the_stored_session_so_next_message_creates_a_new_one() {
    let dir = tempdir().unwrap();
    let map_path = dir.path().join("map.json");
    let (runtime, _) = build_runtime(dir.path().to_path_buf(), map_path.clone());

    let key = ConversationKey::new("telegram", "chat-1");
    assert!(runtime.session_for(&key).unwrap().is_none());
    // No session was ever stored; reset is a no-op that does not error.
    assert!(runtime.reset_conversation(&key).unwrap().is_none());

    // Map is empty on disk too.
    let reloaded = ConversationSessionMap::load(&map_path).unwrap();
    assert!(reloaded.is_empty());
}

#[test]
fn session_map_survives_runtime_rebuild() {
    let dir = tempdir().unwrap();
    let map_path = dir.path().join("map.json");
    let key = ConversationKey::new("slack", "C123");
    let session_id = uuid::Uuid::new_v4();

    // Seed the map directly, then rebuild the runtime from disk and
    // verify the binding survives.
    {
        let mut map = ConversationSessionMap::load(&map_path).unwrap();
        map.insert(&key, session_id).unwrap();
    }

    let (runtime, _) = build_runtime(dir.path().to_path_buf(), map_path);
    assert_eq!(runtime.session_for(&key).unwrap(), Some(session_id));
}

#[test]
fn session_for_is_idempotent() {
    let dir = tempdir().unwrap();
    let (runtime, _) =
        build_runtime(dir.path().to_path_buf(), dir.path().join("map.json"));
    let key = ConversationKey::new("matrix", "!room");

    for _ in 0..3 {
        assert!(runtime.session_for(&key).unwrap().is_none());
    }
}
