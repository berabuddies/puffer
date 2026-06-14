use super::super::agent::{key_text, parse_key_combo, scroll_delta};
use super::super::cursor::parse_cursor_response;
use super::super::dom_inspect::dom_inspect_expression;
use super::super::params::{parse_input_event, required_string_array};
use super::super::ref_resolution::{
    checkable_state_expression, fill_expression, focus_expression,
    hosted_fill_focus_check_expression, scroll_into_view_expression, select_expression,
    target_point_expression,
};
use super::super::screenshot::{
    parse_agent_screenshot_options, parse_capture_screenshot_response, snapshot_expression,
    BrowserElementRef, BrowserScreenshotFormat,
};
use super::super::selection::parse_copy_selection_response;
use super::super::upload::parse_upload_handle_response;
use super::super::{parse_evaluation_response, BrowserInputEvent};
use serde_json::json;

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
fn fill_expression_reads_back_value_to_catch_silent_failures() {
    let expression = fill_expression(
        &BrowserElementRef {
            ref_id: "@e1".to_string(),
            role: "textbox".to_string(),
            name: "Card number".to_string(),
            tag: "input".to_string(),
            href: None,
            x: 10.0,
            y: 20.0,
        },
        "4242424242424242",
    )
    .unwrap();
    assert!(expression.contains("did not stick"));
    assert!(expression.contains("IFRAME"));
    assert!(expression.contains("targetEl.value === ''"));
    assert!(!expression.contains("targetEl.value !== expected"));
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
fn dom_inspect_expression_returns_bounded_selector_metadata() {
    let expression = dom_inspect_expression("input[type=email]").unwrap();
    assert!(expression.contains("document.querySelectorAll(selector)"));
    assert!(expression.contains("all.slice(0, 25)"));
    assert!(expression.contains("attributes: attrsOf(el)"));
    assert!(dom_inspect_expression(" ").is_err());
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
fn target_point_expression_scrolls_and_clamps_to_viewport() {
    let expression = target_point_expression(&BrowserElementRef {
        ref_id: "@e1".to_string(),
        role: "button".to_string(),
        name: "Pay".to_string(),
        tag: "button".to_string(),
        href: None,
        x: 10.0,
        y: 20.0,
    })
    .unwrap();
    assert!(expression.contains("scrollIntoView"));
    assert!(expression.contains("Math.min(Math.max"));
    assert!(expression.contains("Target has no stable viewport point"));
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
    let expression =
        super::super::ref_resolution::upload_input_handle_expression(&BrowserElementRef {
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

fn card_container_ref() -> BrowserElementRef {
    BrowserElementRef {
        ref_id: "@e20".to_string(),
        role: "iframe".to_string(),
        name: "Field container for: Card number".to_string(),
        tag: "iframe".to_string(),
        href: None,
        x: 348.0,
        y: 671.0,
    }
}

#[test]
fn fill_expression_hands_hosted_iframe_fields_to_runtime() {
    let expression = fill_expression(&card_container_ref(), "4242424242424242").unwrap();
    assert!(expression.contains("hostedFrameFill: true"));
    assert!(expression.contains("window.__puffer_hosted_fill__"));
    assert!(expression.contains("targetEl.querySelector('iframe')"));
    // The old behavior threw on IFRAME shells; the probe must return a
    // handoff marker instead of throwing.
    assert!(!expression.contains("cannot be filled from the top document"));
}

#[test]
fn hosted_fill_focus_check_requires_pending_frame_focus() {
    let expression = hosted_fill_focus_check_expression();
    assert!(expression.contains("window.__puffer_hosted_fill__"));
    assert!(expression.contains("document.activeElement === pending.frame"));
    assert!(expression.contains("no pending hosted fill frame"));
}

#[test]
fn ref_actions_prefer_stored_snapshot_handles_over_stale_coordinates() {
    let expression = target_point_expression(&card_container_ref()).unwrap();
    assert!(expression.contains("window.__puffer_agent_refs__"));
    assert!(expression.contains("stored.isConnected"));
    // The signature fallback must still exist for post-navigation refs.
    assert!(expression.contains("document.elementFromPoint(target.x, target.y)"));
}

#[test]
fn snapshot_lists_named_iframes_and_stashes_exact_handles() {
    let expression = snapshot_expression();
    assert!(expression.contains("iframe,[role]"));
    assert!(expression.contains("el.tagName !== 'IFRAME' || nameFor(el) !== ''"));
    assert!(expression.contains("window.__puffer_agent_refs__ = { byRef }"));
}

#[test]
fn parses_modifier_key_combos() {
    let combo = parse_key_combo("Meta+A");
    assert_eq!(combo.key, "A");
    assert_eq!(combo.modifiers, 4);
    assert_eq!(combo.commands, vec!["selectAll".to_string()]);

    let combo = parse_key_combo("Ctrl+a");
    assert_eq!(combo.key, "a");
    assert_eq!(combo.modifiers, 2);
    assert_eq!(combo.commands, vec!["selectAll".to_string()]);

    let combo = parse_key_combo("Ctrl+Shift+Z");
    assert_eq!(combo.key, "Z");
    assert_eq!(combo.modifiers, 2 | 8);
    assert!(combo.commands.is_empty());
}

#[test]
fn plain_keys_and_edge_combos_stay_unmodified() {
    let combo = parse_key_combo("Enter");
    assert_eq!(combo.key, "Enter");
    assert_eq!(combo.modifiers, 0);
    assert!(combo.commands.is_empty());

    // The bare plus key is not a combo.
    let combo = parse_key_combo("+");
    assert_eq!(combo.key, "+");
    assert_eq!(combo.modifiers, 0);

    // `Meta++` means Meta plus the `+` key.
    let combo = parse_key_combo("Meta++");
    assert_eq!(combo.key, "+");
    assert_eq!(combo.modifiers, 4);

    // Unknown prefixes are not modifiers; pass the raw key through.
    let combo = parse_key_combo("Foo+A");
    assert_eq!(combo.key, "Foo+A");
    assert_eq!(combo.modifiers, 0);
}
