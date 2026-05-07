use puffer_provider_openai::{
    extract_chat_completions_reasoning, extract_chat_completions_visible_text,
    parse_chat_completions_response,
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
