use super::params::parse_input_event;
use super::*;

#[test]
fn normalizes_empty_and_full_urls() {
    assert_eq!(normalize_url("").unwrap(), "about:blank");
    assert_eq!(
        normalize_url("https://example.com/a").unwrap(),
        "https://example.com/a"
    );
}

#[test]
fn normalizes_local_and_inline_urls() {
    assert_eq!(
        normalize_url("file:///Users/shou/puffer/helloworld.html").unwrap(),
        "file:///Users/shou/puffer/helloworld.html"
    );
    assert_eq!(
        normalize_url("data:text/html,<h1>Hello</h1>").unwrap(),
        "data:text/html,<h1>Hello</h1>"
    );
}

#[test]
fn normalizes_bare_hosts() {
    assert_eq!(normalize_url("example.com").unwrap(), "https://example.com");
    assert_eq!(
        normalize_url("localhost:3000").unwrap(),
        "http://localhost:3000"
    );
    assert_eq!(
        normalize_url("127.0.0.1:1420").unwrap(),
        "http://127.0.0.1:1420"
    );
}

#[test]
fn profile_names_are_filesystem_safe() {
    assert_eq!(safe_profile_name("abc/def gh"), "abc_def_gh");
}

#[test]
fn navigate_updates_cached_state_before_worker_ack() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let session = BrowserSession {
        tx,
        state: std::sync::Arc::new(std::sync::Mutex::new(BrowserState {
            url: DEFAULT_URL.to_string(),
            title: String::new(),
            loading: false,
            width: 960,
            height: 720,
        })),
        last_active: std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
    };

    session
        .navigate("file:///Users/shou/puffer/hello-world.html".to_string())
        .unwrap();

    let state = session.state();
    assert_eq!(state.url, "file:///Users/shou/puffer/hello-world.html");
    assert!(state.loading);
}

#[test]
fn browser_recording_requires_agent_activity_window() {
    let mut recordings = recording::BrowserRecordingRegistry::default();
    let state = BrowserState {
        url: "https://example.com".to_string(),
        title: "Example".to_string(),
        loading: false,
        width: 960,
        height: 720,
    };
    let backend_id = "root-session:browser:t1";

    assert!(recordings
        .record_frame(backend_id, "frame-1", "image-a", 960, 720, &state)
        .is_none());

    recordings.arm_backend(backend_id, Duration::from_secs(1));
    assert!(recordings
        .record_frame(backend_id, "frame-2", "image-a", 960, 720, &state)
        .is_some());
}

#[test]
fn parses_text_input_event() {
    let event = parse_input_event(&json!({ "kind": "text", "text": "hello" })).unwrap();
    match event {
        BrowserInputEvent::Text { text } => assert_eq!(text, "hello"),
        _ => panic!("unexpected event"),
    }
}

#[test]
fn parses_mouse_buttons_input_event() {
    let event = parse_input_event(&json!({
        "kind": "mouse",
        "eventType": "mouseMoved",
        "x": 10.0,
        "y": 20.0,
        "button": "left",
        "buttons": 1,
        "clickCount": 0
    }))
    .unwrap();
    match event {
        BrowserInputEvent::Mouse {
            button,
            buttons,
            click_count,
            ..
        } => {
            assert_eq!(button, "left");
            assert_eq!(buttons, Some(1));
            assert_eq!(click_count, 0);
        }
        _ => panic!("unexpected event"),
    }
}

#[test]
fn parses_copy_selection_response() {
    let copied = parse_copy_selection_response(&json!({
        "id": 7,
        "result": {
            "result": {
                "type": "object",
                "value": {
                    "text": "selected text",
                    "copiedFrom": "document-selection"
                }
            }
        }
    }))
    .unwrap();
    assert_eq!(copied.text, "selected text");
    assert_eq!(copied.copied_from, "document-selection");
}

#[test]
fn parses_cursor_response() {
    let cursor = parse_cursor_response(&json!({
        "id": 8,
        "result": {
            "result": {
                "type": "object",
                "value": {
                    "cursor": "pointer"
                }
            }
        }
    }))
    .unwrap();
    assert_eq!(cursor.cursor, "pointer");
}

#[test]
fn evaluation_errors_prefer_exception_description() {
    let error = parse_evaluation_response(&json!({
        "id": 9,
        "result": {
            "exceptionDetails": {
                "text": "Uncaught",
                "lineNumber": 4,
                "columnNumber": 12,
                "exception": {
                    "description": "Error: Target is not editable"
                }
            }
        }
    }))
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("line 5, column 13"));
    assert!(message.contains("Target is not editable"));
}
