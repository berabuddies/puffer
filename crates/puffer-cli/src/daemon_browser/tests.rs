use super::agent::{key_text, scroll_delta};
use super::cursor::parse_cursor_response;
use super::params::{parse_input_event, required_string_array};
use super::ref_resolution::{
    checkable_state_expression, fill_expression, focus_expression, scroll_into_view_expression,
    select_expression, upload_input_handle_expression,
};
use super::screenshot::{
    parse_agent_screenshot_options, parse_capture_screenshot_response, BrowserElementRef,
    BrowserScreenshotFormat,
};
use super::selection::parse_copy_selection_response;
use super::upload::parse_upload_handle_response;
use super::*;
use crate::daemon_browser::tabs::BrowserCurrentTabStatus;

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
fn navigate_updates_cached_state_before_worker_ack() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let session = BrowserSession::new_for_test(
        tx,
        std::sync::Arc::new(std::sync::Mutex::new(BrowserState {
            url: DEFAULT_URL.to_string(),
            title: String::new(),
            loading: false,
            width: 960,
            height: 720,
        })),
        std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
    );

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
fn browser_console_registry_records_and_clears_console_payloads() {
    let mut console_logs = console::BrowserConsoleRegistry::default();
    let payload = devtools::devtools_event_payload(
        "Runtime.consoleAPICalled",
        &json!({
            "params": {
                "type": "error",
                "args": [{ "value": "boom" }],
                "timestamp": 12.0
            }
        }),
    )
    .unwrap();
    console_logs.record("root-session:browser:t1", &payload);
    console_logs.record(
        "root-session:browser:t1",
        &json!({ "kind": "network", "url": "https://example.com" }),
    );

    let logs = console_logs.read("root-session:browser:t1", false);
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].get("level").and_then(Value::as_str), Some("error"));
    assert_eq!(logs[0].get("text").and_then(Value::as_str), Some("boom"));
    assert!(logs[0].get("recordedAtMs").is_some());

    let cleared = console_logs.read("root-session:browser:t1", true);
    assert_eq!(cleared.len(), 1);
    assert!(console_logs
        .read("root-session:browser:t1", false)
        .is_empty());
}

#[test]
fn cleanup_root_metadata_preserves_disconnected_tab_handles() {
    let tabs = std::sync::Arc::new(std::sync::Mutex::new(BrowserTabRegistry::default()));
    let agent_refs =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::from([(
            "root-session:browser:t1".to_string(),
            Vec::<BrowserElementRef>::new(),
        )])));
    let console_logs = std::sync::Arc::new(std::sync::Mutex::new(
        console::BrowserConsoleRegistry::default(),
    ));
    let browser_state = BrowserState {
        url: "https://example.com".to_string(),
        title: "Example".to_string(),
        loading: false,
        width: 960,
        height: 720,
    };
    tabs.lock().unwrap().open_tab(
        "root-session",
        Some("t1".to_string()),
        None,
        "root-session:browser:t1".to_string(),
        browser_state,
        true,
    );

    cleanup_root_metadata(
        &tabs,
        &agent_refs,
        &console_logs,
        "root-session",
        &["root-session:browser:t1".to_string()],
        true,
    );

    let state = tabs.lock().unwrap().list("root-session");
    assert_eq!(state.tabs.len(), 1);
    assert!(!state.tabs[0].connected);
    assert!(agent_refs.lock().unwrap().is_empty());
}

#[test]
fn cleanup_root_metadata_drops_tab_set_on_root_close() {
    let tabs = std::sync::Arc::new(std::sync::Mutex::new(BrowserTabRegistry::default()));
    let agent_refs =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::from([(
            "root-session:browser:t1".to_string(),
            Vec::<BrowserElementRef>::new(),
        )])));
    let console_logs = std::sync::Arc::new(std::sync::Mutex::new(
        console::BrowserConsoleRegistry::default(),
    ));
    let browser_state = BrowserState {
        url: "https://example.com".to_string(),
        title: "Example".to_string(),
        loading: false,
        width: 960,
        height: 720,
    };
    tabs.lock().unwrap().open_tab(
        "root-session",
        Some("t1".to_string()),
        None,
        "root-session:browser:t1".to_string(),
        browser_state,
        true,
    );

    cleanup_root_metadata(
        &tabs,
        &agent_refs,
        &console_logs,
        "root-session",
        &["root-session:browser:t1".to_string()],
        false,
    );

    assert!(tabs.lock().unwrap().list("root-session").tabs.is_empty());
    assert!(agent_refs.lock().unwrap().is_empty());
}

#[test]
fn current_tab_context_reports_no_active_tab() {
    let registry = BrowserTabRegistry::default();
    let context = registry
        .active_tab("root-session")
        .map(|tab| BrowserCurrentTabContext::from_tab(&tab))
        .unwrap_or_else(BrowserCurrentTabContext::no_active_tab);

    assert_eq!(context.status, BrowserCurrentTabStatus::NoActiveTab);
    assert_eq!(context.tab_id, None);
    assert_eq!(context.url, None);
    assert_eq!(context.origin, None);
    assert_eq!(context.host, None);
    assert_eq!(context.port, None);
    assert_eq!(context.title, None);
}

#[test]
fn current_tab_context_reports_empty_url() {
    let mut registry = BrowserTabRegistry::default();
    let tab = registry.open_tab(
        "root-session",
        Some("t1".to_string()),
        None,
        "root-session:browser:t1".to_string(),
        BrowserState {
            url: String::new(),
            title: "Blank".to_string(),
            loading: false,
            width: 960,
            height: 720,
        },
        true,
    );

    let context = BrowserCurrentTabContext::from_tab(&tab);
    assert_eq!(context.status, BrowserCurrentTabStatus::EmptyUrl);
    assert_eq!(context.tab_id.as_deref(), Some("t1"));
    assert_eq!(context.url.as_deref(), Some(""));
    assert_eq!(context.origin, None);
    assert_eq!(context.host, None);
    assert_eq!(context.port, None);
    assert_eq!(context.title.as_deref(), Some("Blank"));
}

#[test]
fn current_tab_context_reports_about_blank() {
    let mut registry = BrowserTabRegistry::default();
    let tab = registry.open_tab(
        "root-session",
        Some("t1".to_string()),
        None,
        "root-session:browser:t1".to_string(),
        BrowserState {
            url: "about:blank".to_string(),
            title: String::new(),
            loading: false,
            width: 960,
            height: 720,
        },
        true,
    );

    let context = BrowserCurrentTabContext::from_tab(&tab);
    assert_eq!(context.status, BrowserCurrentTabStatus::AboutBlank);
    assert_eq!(context.tab_id.as_deref(), Some("t1"));
    assert_eq!(context.url.as_deref(), Some("about:blank"));
    assert_eq!(context.origin, None);
    assert_eq!(context.host, None);
    assert_eq!(context.port, None);
}

#[test]
fn current_tab_context_extracts_origin_host_port_and_title() {
    let mut registry = BrowserTabRegistry::default();
    let tab = registry.open_tab(
        "root-session",
        Some("t1".to_string()),
        None,
        "root-session:browser:t1".to_string(),
        BrowserState {
            url: "https://docs.example.com:8443/path?q=1".to_string(),
            title: "Docs".to_string(),
            loading: false,
            width: 960,
            height: 720,
        },
        true,
    );

    let context = BrowserCurrentTabContext::from_tab(&tab);
    assert_eq!(context.status, BrowserCurrentTabStatus::Available);
    assert_eq!(context.tab_id.as_deref(), Some("t1"));
    assert_eq!(
        context.url.as_deref(),
        Some("https://docs.example.com:8443/path?q=1")
    );
    assert_eq!(
        context.origin.as_deref(),
        Some("https://docs.example.com:8443")
    );
    assert_eq!(context.host.as_deref(), Some("docs.example.com"));
    assert_eq!(context.port, Some(8443));
    assert_eq!(context.title.as_deref(), Some("Docs"));
}

#[test]
fn shutdown_ack_wait_uses_one_shared_deadline() {
    let (_tx1, rx1) = std::sync::mpsc::channel::<()>();
    let (_tx2, rx2) = std::sync::mpsc::channel::<()>();
    let (_tx3, rx3) = std::sync::mpsc::channel::<()>();
    let start = std::time::Instant::now();

    wait_for_shutdown_acks(vec![rx1, rx2, rx3], Duration::from_millis(60));

    assert!(start.elapsed() < Duration::from_millis(140));
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
fn screenshot_options_default_to_plain_png_capture() {
    let options = parse_agent_screenshot_options(&json!({})).unwrap();
    assert_eq!(options.capture.format, BrowserScreenshotFormat::Png);
    assert_eq!(options.capture.quality, None);
    assert!(!options.annotate);
}

#[test]
fn screenshot_options_require_jpeg_for_quality() {
    let error = parse_agent_screenshot_options(&json!({
        "screenshotQuality": 80
    }))
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("`screenshotQuality` requires `screenshotFormat` `jpeg`"));
}

#[test]
fn parses_capture_screenshot_response() {
    let screenshot = parse_capture_screenshot_response(
        &json!({
            "id": 10,
            "result": {
                "data": "ZmFrZS1pbWFnZS1ieXRlcw=="
            }
        }),
        BrowserScreenshotFormat::Jpeg,
    )
    .unwrap();
    assert_eq!(screenshot.format, BrowserScreenshotFormat::Jpeg);
    assert_eq!(screenshot.data, "ZmFrZS1pbWFnZS1ieXRlcw==");
}

#[test]
fn parses_required_string_array_for_upload_files() {
    let files =
        required_string_array(&json!({ "files": ["a.txt", "nested/b.txt"] }), "files").unwrap();
    assert_eq!(files, vec!["a.txt", "nested/b.txt"]);
    assert!(required_string_array(&json!({ "files": [] }), "files").is_err());
}

#[test]
fn parses_upload_handle_response_object_id() {
    let object_id = parse_upload_handle_response(&json!({
        "id": 10,
        "result": {
            "result": {
                "type": "object",
                "subtype": "node",
                "className": "HTMLInputElement",
                "objectId": "123.456.789"
            }
        }
    }))
    .unwrap();
    assert_eq!(object_id, "123.456.789");
}

#[test]
fn fill_expression_uses_ref_resolution() {
    let expression = fill_expression(
        &BrowserElementRef {
            ref_id: "@e1".to_string(),
            role: "textbox".to_string(),
            name: "Name".to_string(),
            tag: "textarea".to_string(),
            href: None,
            x: 10.0,
            y: 20.0,
        },
        "pufferfish",
    )
    .unwrap();
    assert!(expression.contains("findTarget(refTarget)"));
    assert!(expression.contains("Target is not editable"));
}

#[test]
fn fill_expression_uses_native_value_setter() {
    let expression = fill_expression(
        &BrowserElementRef {
            ref_id: "@e1".to_string(),
            role: "textbox".to_string(),
            name: "Name".to_string(),
            tag: "textarea".to_string(),
            href: None,
            x: 10.0,
            y: 20.0,
        },
        "pufferfish",
    )
    .unwrap();
    assert!(expression.contains("Object.getOwnPropertyDescriptor(prototype, 'value')"));
    assert!(expression.contains("descriptor.set.call(target"));
}

#[test]
fn focus_expression_targets_focusable_elements() {
    let expression = focus_expression(&BrowserElementRef {
        ref_id: "@e1".to_string(),
        role: "button".to_string(),
        name: "Submit".to_string(),
        tag: "button".to_string(),
        href: None,
        x: 10.0,
        y: 20.0,
    })
    .unwrap();
    assert!(expression.contains("targetEl.focus"));
    assert!(expression.contains("Target is not focusable"));
}

#[test]
fn scroll_helpers_cover_alias_behaviour() {
    assert_eq!(scroll_delta("down", 480).unwrap(), (0.0, 480.0));
    assert!(scroll_delta("diagonal", 480).is_err());
    assert_eq!(key_text("A").as_deref(), Some("A"));
    assert_eq!(key_text("Enter"), None);
    let expression = scroll_into_view_expression(&BrowserElementRef {
        ref_id: "@e1".to_string(),
        role: "button".to_string(),
        name: "Save".to_string(),
        tag: "button".to_string(),
        href: None,
        x: 10.0,
        y: 20.0,
    })
    .unwrap();
    assert!(expression.contains("findTarget(refTarget)"));
    assert!(expression.contains("scrollIntoView"));
}

#[test]
fn select_expression_supports_label_bound_selects() {
    let expression = select_expression(
        &BrowserElementRef {
            ref_id: "@e1".to_string(),
            role: "combobox".to_string(),
            name: "State".to_string(),
            tag: "select".to_string(),
            href: None,
            x: 10.0,
            y: 20.0,
        },
        "New York",
    )
    .unwrap();
    assert!(expression.contains("findTarget(refTarget)"));
    assert!(expression.contains("dispatchEvent(new Event('change'"));
}

#[test]
fn upload_expression_supports_direct_inputs_and_labels() {
    let expression = upload_input_handle_expression(&BrowserElementRef {
        ref_id: "@e1".to_string(),
        role: "file".to_string(),
        name: "Upload".to_string(),
        tag: "input".to_string(),
        href: None,
        x: 10.0,
        y: 20.0,
    })
    .unwrap();
    assert!(expression.contains("resolveFileInputTarget(refElement)"));
    assert!(expression.contains("Target is not a native file input"));
}

#[test]
fn checkable_state_expression_supports_labels_and_roles() {
    let expression = checkable_state_expression(&BrowserElementRef {
        ref_id: "@e1".to_string(),
        role: "checkbox".to_string(),
        name: "Accept".to_string(),
        tag: "input".to_string(),
        href: None,
        x: 10.0,
        y: 20.0,
    })
    .unwrap();
    assert!(expression.contains("resolveCheckableTarget(refElement)"));
    assert!(expression.contains("Target is not a checkbox or radio control"));
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
