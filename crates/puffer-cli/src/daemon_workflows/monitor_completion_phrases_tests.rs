use super::*;

use serde_json::json;

fn discover(tempdir: &tempfile::TempDir) -> ConfigPaths {
    ConfigPaths::discover(tempdir.path())
}

#[test]
fn add_persists_phrases_and_load_reads_them_back() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = discover(&tempdir);

    handle_monitor_completion_phrase_add(
        &paths,
        &json!({ "connectionSlug": "telegram-user", "phrases": ["快递拿了", "稿子发了"] }),
    )
    .unwrap();

    let phrases = load_completion_phrases(&paths, "telegram-user");
    assert_eq!(phrases, vec!["快递拿了".to_string(), "稿子发了".to_string()]);
}

#[test]
fn add_accepts_single_phrase_field_and_dedupes() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = discover(&tempdir);

    handle_monitor_completion_phrase_add(
        &paths,
        &json!({ "connection_slug": "telegram-user", "phrase": "搞定啦" }),
    )
    .unwrap();
    // Adding the same phrase again must not duplicate it.
    handle_monitor_completion_phrase_add(
        &paths,
        &json!({ "connection_slug": "telegram-user", "phrase": "搞定啦" }),
    )
    .unwrap();

    assert_eq!(
        load_completion_phrases(&paths, "telegram-user"),
        vec!["搞定啦".to_string()]
    );
}

#[test]
fn delete_removes_phrase_and_prunes_empty_connection() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = discover(&tempdir);

    handle_monitor_completion_phrase_add(
        &paths,
        &json!({ "connectionSlug": "telegram-user", "phrases": ["快递拿了", "稿子发了"] }),
    )
    .unwrap();
    handle_monitor_completion_phrase_delete(
        &paths,
        &json!({ "connectionSlug": "telegram-user", "phrase": "快递拿了" }),
    )
    .unwrap();

    assert_eq!(
        load_completion_phrases(&paths, "telegram-user"),
        vec!["稿子发了".to_string()]
    );

    // Removing the last phrase prunes the connection entry entirely.
    handle_monitor_completion_phrase_delete(
        &paths,
        &json!({ "connectionSlug": "telegram-user", "phrase": "稿子发了" }),
    )
    .unwrap();
    assert!(load_completion_phrases(&paths, "telegram-user").is_empty());
}

#[test]
fn empty_phrase_add_is_rejected() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = discover(&tempdir);

    let result = handle_monitor_completion_phrase_add(
        &paths,
        &json!({ "connectionSlug": "telegram-user", "phrases": ["   "] }),
    );
    assert!(result.is_err());
}

#[test]
fn load_for_unknown_connection_is_empty() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = discover(&tempdir);
    assert!(load_completion_phrases(&paths, "telegram-user").is_empty());
}

#[test]
fn invalid_connection_slug_is_rejected() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = discover(&tempdir);

    let result = handle_monitor_completion_phrase_add(
        &paths,
        &json!({ "connectionSlug": "../escape", "phrase": "done" }),
    );
    assert!(result.is_err());
}
