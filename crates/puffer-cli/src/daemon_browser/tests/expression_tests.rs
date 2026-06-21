use super::super::agent::{key_text, parse_key_combo, scroll_delta};
use super::super::cursor::parse_cursor_response;
use super::super::dom_inspect::dom_inspect_expression;
use super::super::params::{parse_input_event, required_string_array};
use super::super::ref_resolution::{
    checkable_state_expression, fill_expression, focus_expression,
    hosted_fill_focus_check_expression, in_frame_prepare_fill_fn, in_frame_readback_fn,
    in_frame_select_fn, scroll_into_view_expression, select_expression, target_point_expression,
};
use super::super::screenshot::{
    collect_in_frame_field_nodes, field_nodes_to_refs, parse_agent_screenshot_options,
    parse_capture_screenshot_response, payment_frame_ids_from_tree, snapshot_expression,
    BrowserElementRef, BrowserScreenshotFormat,
};
use super::super::selection::parse_copy_selection_response;
use super::super::upload::parse_upload_handle_response;
use super::super::{parse_cdp_call_response, parse_evaluation_response, BrowserInputEvent};
use serde_json::json;
use std::collections::{HashMap, HashSet};

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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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

/// A `DOM.getDocument { depth: -1, pierce: true }` tree that mirrors the real
/// Amazon checkout probed for #656: a cross-origin `ApxSecureIframe` whose
/// `contentDocument` holds the card fields. The deep pass must surface the
/// fillable interior fields — which the top-frame snapshot can never reach —
/// while ignoring hidden fields and the top-document field outside the frame.
fn dom_node(name: &str, backend: i64, attrs: &[&str]) -> serde_json::Value {
    json!({ "nodeName": name, "backendNodeId": backend, "attributes": attrs })
}

// Wraps children in #document -> HTML -> BODY, mirroring DOM.getDocument shape.
fn dom_document(extra: serde_json::Value, body_children: serde_json::Value) -> serde_json::Value {
    let _ = extra;
    let body = json!({ "nodeName": "BODY", "children": body_children });
    let html = json!({ "nodeName": "HTML", "children": [body] });
    json!({ "nodeName": "#document", "children": [html] })
}

fn amazon_apx_pierced_document() -> serde_json::Value {
    let month = json!({
        "nodeName": "SELECT", "backendNodeId": 558,
        "attributes": ["name", "ppw-expirationDate_month"],
        "children": [{
            "nodeName": "OPTION", "backendNodeId": 8882, "attributes": ["value", "1"],
            "children": [{ "nodeName": "#text", "nodeValue": "01" }]
        }]
    });
    let inner_fields = json!([
        dom_node("INPUT", 8780, &["type", "hidden", "name", "ue_back"]),
        dom_node("INPUT", 561, &["type", "tel", "name", "addCreditCardNumber", "autocomplete", "off"]),
        dom_node("INPUT", 562, &["type", "text", "name", "ppw-accountHolderName"]),
        month,
        dom_node("SELECT", 559, &["name", "ppw-expirationDate_year"]),
        dom_node("INPUT", 9126, &["type", "submit", "name", "ppw-widgetEvent:AddCreditCardEvent"]),
    ]);
    let content_document = dom_document(json!(null), inner_fields);
    let iframe = json!({
        "nodeName": "IFRAME", "backendNodeId": 585, "frameId": "APXFRAME",
        "attributes": ["name", "ApxSecureIframe-pp-N6qjWY-8", "class", "apx-secure-iframe"],
        "contentDocument": content_document
    });
    // Top-document field outside the frame: must NOT be surfaced by the deep pass.
    let top_input = dom_node("INPUT", 99, &["type", "text", "name", "email"]);
    dom_document(json!(null), json!([top_input, iframe]))
}

#[test]
fn collect_in_frame_field_nodes_descends_only_into_payment_frames() {
    let doc = amazon_apx_pierced_document();
    let payment: HashSet<String> = ["APXFRAME".to_string()].into_iter().collect();
    let nodes = collect_in_frame_field_nodes(&doc, &payment);

    let ids: Vec<i64> = nodes.iter().map(|n| n.backend_node_id).collect();
    // card#, name, expiry month, expiry year, submit — five fillable/actionable fields.
    assert_eq!(nodes.len(), 5, "unexpected fields: {ids:?}");
    // The hidden field is excluded.
    assert!(!ids.contains(&8780), "hidden field must be skipped");
    // The top-document field (outside the payment frame) is excluded.
    assert!(!ids.contains(&99), "top-document field must not be surfaced");

    let role_for = |id: i64| nodes.iter().find(|n| n.backend_node_id == id).map(|n| n.role.as_str());
    assert_eq!(role_for(561), Some("textbox"));
    assert_eq!(role_for(558), Some("combobox"));
    assert_eq!(role_for(9126), Some("button"));
}

#[test]
fn collect_in_frame_field_nodes_ignores_unlisted_frames() {
    // When no frame is gated in as a payment frame, nothing is surfaced —
    // the deep pass must never descend into arbitrary cross-origin iframes.
    let doc = amazon_apx_pierced_document();
    let nodes = collect_in_frame_field_nodes(&doc, &HashSet::new());
    assert!(nodes.is_empty());
}

#[test]
fn collect_in_frame_field_nodes_skips_titled_payment_iframes() {
    // A titled cross-origin field iframe (Stripe/Shopify: one named iframe per
    // field) is already surfaced by the top-document snapshot and handled by
    // hosted_frame_fill. The deep pass must NOT also descend into it, or the
    // agent would see duplicate refs for the same field.
    let inner = json!([dom_node(
        "INPUT",
        700,
        &["type", "tel", "name", "cardnumber"]
    )]);
    let content = dom_document(json!(null), inner);
    let titled_iframe = json!({
        "nodeName": "IFRAME", "backendNodeId": 690, "frameId": "STRIPEFRAME",
        "attributes": ["title", "Secure card number input frame", "name", "__privateStripeFrame"],
        "contentDocument": content
    });
    let doc = dom_document(json!(null), json!([titled_iframe]));
    let payment: HashSet<String> = ["STRIPEFRAME".to_string()].into_iter().collect();
    assert!(
        collect_in_frame_field_nodes(&doc, &payment).is_empty(),
        "titled payment iframe must be left to the existing hosted-fill path"
    );
}

#[test]
fn parse_cdp_call_response_returns_raw_result_object() {
    // Raw CDP methods (DOM.getDocument / getContentQuads) put their payload
    // directly under /result, not /result/result/value like Runtime.evaluate.
    let value = json!({ "id": 7, "result": { "quads": [[1.0, 2.0, 3.0, 4.0]] } });
    let result = parse_cdp_call_response(&value).unwrap();
    assert_eq!(result, json!({ "quads": [[1.0, 2.0, 3.0, 4.0]] }));
}

#[test]
fn parse_cdp_call_response_surfaces_protocol_error() {
    let value = json!({ "id": 7, "error": { "code": -32000, "message": "No node with given id found" } });
    let err = parse_cdp_call_response(&value).unwrap_err();
    assert!(format!("{err:#}").contains("No node with given id"));
}

#[test]
fn parse_cdp_call_response_surfaces_call_function_exception() {
    // Runtime.callFunctionOn reports a thrown script error via
    // result.exceptionDetails, NOT a top-level `error`. It must surface as Err
    // rather than being mistaken for a successful result.
    let value = json!({
        "id": 7,
        "result": {
            "result": { "type": "object" },
            "exceptionDetails": {
                "text": "Uncaught",
                "exception": { "description": "TypeError: this.options is undefined" }
            }
        }
    });
    let err = parse_cdp_call_response(&value).unwrap_err();
    assert!(format!("{err:#}").contains("this.options is undefined"));
}

#[test]
fn in_frame_prepare_fill_fn_verifies_focus_before_typing() {
    // The clear-before-type step runs INSIDE the OOPIF and must confirm focus
    // actually landed on this field (an empty activeElement check is the only
    // #580 guard available cross-origin) before the runtime sends trusted text.
    let body = in_frame_prepare_fill_fn();
    assert!(body.contains("document.activeElement"));
    assert!(body.contains("focused"));
}

#[test]
fn in_frame_readback_fn_reports_value_for_silent_failure_check() {
    // Unlike top-document hosted fills, an in-frame field's value CAN be read
    // back via callFunctionOn — used to catch a fill that did not stick (#580).
    let body = in_frame_readback_fn();
    assert!(body.contains("value"));
}

#[test]
fn in_frame_select_fn_matches_by_value_or_visible_text_and_fires_change() {
    let body = in_frame_select_fn("2030").unwrap();
    // The chosen value is embedded safely (JSON-encoded).
    assert!(body.contains("\"2030\""));
    // Matching falls back to the option's visible label, and fires change/input
    // so the payment widget observes the selection.
    assert!(body.contains("matched"));
    assert!(body.contains("change"));
    assert!(body.contains("textContent") || body.contains("text"));
}

#[test]
fn payment_frame_ids_from_tree_matches_known_processor_hosts() {
    // Page.getFrameTree result: an Amazon APX payment frame, an ad frame, and
    // the main frame. Only the payment frame is gated in for the deep pierce.
    let result = json!({
        "frameTree": {
            "frame": { "id": "MAIN", "url": "https://www.amazon.com/checkout/p/pay" },
            "childFrames": [
                { "frame": { "id": "APX", "url": "https://apx-security.amazon.com/cpe/pm/register", "name": "ApxSecureIframe-pp-x-8" } },
                { "frame": { "id": "AD", "url": "https://s.amazon-adsystem.com/iu3?d=amazon.com" } }
            ]
        }
    });
    let frames = payment_frame_ids_from_tree(&result);
    assert!(frames.contains("APX"), "Amazon APX frame must be gated in");
    assert!(!frames.contains("AD"), "ad frames must be excluded");
    assert!(!frames.contains("MAIN"), "the main frame is never a payment frame");
}

#[test]
fn payment_frame_gate_anchors_host_match_to_domain_boundary() {
    // A host that merely *contains* a processor name as a substring must NOT be
    // gated in (e.g. `notpaypal.com`, `foo-apx-security.amazon.com.evil.com`).
    // Only an exact host or a real subdomain of a known payment host qualifies.
    let result = json!({
        "frameTree": {
            "frame": { "id": "MAIN", "url": "https://shop.example.com/checkout" },
            "childFrames": [
                { "frame": { "id": "LOOKALIKE", "url": "https://notpaypal.com/x", "name": "" } },
                { "frame": { "id": "SUFFIX", "url": "https://apx-security.amazon.com.evil.com/x", "name": "" } },
                { "frame": { "id": "SUBDOMAIN", "url": "https://a.apx-security.amazon.com/cpe", "name": "" } }
            ]
        }
    });
    let frames = payment_frame_ids_from_tree(&result);
    assert!(!frames.contains("LOOKALIKE"), "substring lookalike host must not match");
    assert!(!frames.contains("SUFFIX"), "suffix-injection host must not match");
    assert!(frames.contains("SUBDOMAIN"), "a real subdomain of a payment host must match");
}

#[test]
fn field_nodes_to_refs_builds_addressable_refs_with_quad_centers() {
    let doc = amazon_apx_pierced_document();
    let payment: HashSet<String> = ["APXFRAME".to_string()].into_iter().collect();
    let nodes = collect_in_frame_field_nodes(&doc, &payment);

    let mut quads: HashMap<i64, (f64, f64)> = HashMap::new();
    quads.insert(561, (410.0, 398.0));
    quads.insert(562, (410.0, 438.0));
    quads.insert(558, (344.0, 471.0));
    quads.insert(559, (407.0, 471.0));
    // The submit (9126) has no quad center (e.g. scrolled out) and must be dropped:
    // a ref the runtime can't click is worse than no ref.
    let refs: Vec<BrowserElementRef> = field_nodes_to_refs(&nodes, &quads, 11);

    assert_eq!(refs.len(), 4, "fields without a quad center must be skipped");
    // Ref numbering continues from the top-document count (11 -> @e12..).
    assert_eq!(refs[0].ref_id, "@e12");
    assert_eq!(refs[3].ref_id, "@e15");

    // The agent-facing ref looks plain, but carries OOPIF routing identity
    // (in_frame + backend node id; DOM.resolveNode resolves the node into its
    // owning frame, so the ref needs no separate frame id).
    let card = &refs[0];
    assert!(card.in_frame, "in-frame refs must be flagged for fill routing");
    assert_eq!(card.backend_node_id, Some(561));
    assert_eq!(card.role, "textbox");
    assert_eq!(card.x, 410.0);
    assert_eq!(card.y, 398.0);
}
