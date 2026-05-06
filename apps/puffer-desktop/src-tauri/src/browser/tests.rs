use serde_json::json;

use super::{frame_session_id_string, normalize_url, DEFAULT_URL};

#[test]
fn normalizes_empty_and_bare_hosts() {
    assert_eq!(normalize_url("").unwrap(), DEFAULT_URL);
    assert_eq!(
        normalize_url("localhost:5173").unwrap(),
        "http://localhost:5173"
    );
    assert_eq!(
        normalize_url("127.0.0.1:3000").unwrap(),
        "http://127.0.0.1:3000"
    );
    assert_eq!(normalize_url("example.com").unwrap(), "https://example.com");
}

#[test]
fn preserves_supported_url_schemes() {
    assert_eq!(
        normalize_url("https://example.com/path?q=1").unwrap(),
        "https://example.com/path?q=1"
    );
    assert_eq!(
        normalize_url("file:///tmp/corbina.html").unwrap(),
        "file:///tmp/corbina.html"
    );
    assert_eq!(
        normalize_url("data:text/plain,corbina").unwrap(),
        "data:text/plain,corbina"
    );
}

#[test]
fn formats_frame_session_ids_for_child_cdp_sessions() {
    assert_eq!(frame_session_id_string("root", None), "root");
    assert_eq!(frame_session_id_string("root", Some(&json!(42))), "root:42");
    assert_eq!(frame_session_id_string("root", Some(&json!("42"))), "root");
}
