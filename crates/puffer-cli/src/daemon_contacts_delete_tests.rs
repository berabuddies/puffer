use super::*;
use puffer_config::ConfigPaths;
use serde_json::json;
use std::path::Path;

#[test]
fn delete_contact_trims_id() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let saved = handle_contacts_save(
        &paths,
        &json!({ "name": "Alice", "contact_ids": ["telegram@alice"] }),
    )
    .unwrap();
    let id = saved["contacts"][0]["id"].as_str().unwrap();

    let deleted = handle_contacts_delete(&paths, &json!({ "id": format!(" {id}\n") })).unwrap();

    assert!(deleted["contacts"].as_array().unwrap().is_empty());
}

fn test_config_paths(root: &Path) -> ConfigPaths {
    ConfigPaths {
        workspace_root: root.to_path_buf(),
        workspace_config_dir: root.join(".puffer"),
        user_config_dir: root.join("user-puffer"),
        builtin_resources_dir: root.join("resources"),
    }
}
