use puffer_provider_openai::{
    extract_chat_completions_reasoning, extract_chat_completions_visible_text,
    parse_chat_completions_response, sanitize_reasoning_text,
};

#[test]
fn picks_up_dedicated_reasoning_content_field() {
    let payload = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"hi","reasoning_content":"thoughts"},"finish_reason":"stop"}]}"#;
    let parsed = parse_chat_completions_response(payload).unwrap();
    assert_eq!(
        extract_chat_completions_reasoning(&parsed),
        Some("thoughts".to_string())
    );
    assert_eq!(extract_chat_completions_visible_text(&parsed), "hi");
}

#[test]
fn picks_up_reasoning_alias_used_by_openrouter() {
    let payload = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"hi","reasoning":"thoughts2"},"finish_reason":"stop"}]}"#;
    let parsed = parse_chat_completions_response(payload).unwrap();
    assert_eq!(
        extract_chat_completions_reasoning(&parsed),
        Some("thoughts2".to_string())
    );
}

#[test]
fn falls_back_to_think_tag_inside_content() {
    let payload = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"<think>step 1\nstep 2</think>visible answer"},"finish_reason":"stop"}]}"#;
    let parsed = parse_chat_completions_response(payload).unwrap();
    assert_eq!(
        extract_chat_completions_reasoning(&parsed),
        Some("step 1\nstep 2".to_string())
    );
    assert_eq!(
        extract_chat_completions_visible_text(&parsed),
        "visible answer"
    );
}

#[test]
fn handles_uppercase_think_tag() {
    let payload = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"<Think>thoughts</Think>answer"},"finish_reason":"stop"}]}"#;
    let parsed = parse_chat_completions_response(payload).unwrap();
    assert_eq!(
        extract_chat_completions_reasoning(&parsed),
        Some("thoughts".to_string())
    );
    assert_eq!(extract_chat_completions_visible_text(&parsed), "answer");
}

#[test]
fn no_reasoning_returns_none_and_full_text() {
    let payload = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"plain answer"},"finish_reason":"stop"}]}"#;
    let parsed = parse_chat_completions_response(payload).unwrap();
    assert_eq!(extract_chat_completions_reasoning(&parsed), None);
    assert_eq!(
        extract_chat_completions_visible_text(&parsed),
        "plain answer"
    );
}

#[test]
fn empty_reasoning_content_returns_none() {
    let payload = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"answer","reasoning_content":""},"finish_reason":"stop"}]}"#;
    let parsed = parse_chat_completions_response(payload).unwrap();
    assert_eq!(extract_chat_completions_reasoning(&parsed), None);
}

#[test]
fn strips_nul_byte_from_reasoning_content() {
    // Kimi K2.6 has been observed emitting a stray \x00 mid-reasoning,
    // then refusing the same string on replay with HTTP 400. Verify
    // the NUL is filtered while \t \n \r survive.
    let payload = "{\"id\":\"x\",\"object\":\"chat.completion\",\"choices\":[{\"index\":0,\"message\":{\"role\":\"assistant\",\"content\":\"answer\",\"reasoning_content\":\"first line\\nsecond\\u0000third\\tfourth\"},\"finish_reason\":\"stop\"}]}";
    let parsed = parse_chat_completions_response(payload).unwrap();
    let got = extract_chat_completions_reasoning(&parsed).expect("reasoning");
    assert!(!got.contains('\u{0000}'), "NUL leaked: {got:?}");
    assert!(got.contains('\n'), "newline got stripped: {got:?}");
    assert!(got.contains('\t'), "tab got stripped: {got:?}");
    assert!(got.contains("secondthird"), "NUL boundary not spliced: {got:?}");
}

#[test]
fn strips_control_bytes_from_think_block() {
    // The <think> fallback path should also sanitize so DeepSeek-R1
    // distill outputs round-trip cleanly when the reasoning leaks a
    // C0 byte.
    let payload = "{\"id\":\"x\",\"object\":\"chat.completion\",\"choices\":[{\"index\":0,\"message\":{\"role\":\"assistant\",\"content\":\"<think>good\\u0000bad\\u0001end</think>visible\"},\"finish_reason\":\"stop\"}]}";
    let parsed = parse_chat_completions_response(payload).unwrap();
    let got = extract_chat_completions_reasoning(&parsed).expect("reasoning");
    assert_eq!(got, "goodbadend");
}

#[test]
fn sanitize_preserves_whitespace_and_strips_del() {
    assert_eq!(
        sanitize_reasoning_text("a\tb\nc\rd\u{0000}e\u{007f}f"),
        "a\tb\nc\rdef"
    );
}
