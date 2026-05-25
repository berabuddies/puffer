use super::*;

#[test]
fn anthropic_stream_final_message_parses_sse_text() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let stream = concat!(
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n",
    );

    let output =
        crate::runtime::claude_tools::workflow::anthropic_stream::execute_anthropic_stream(
            &mut state,
            &cwd,
            json!({
                "action": "finalMessage",
                "stream": stream
            }),
        )
        .unwrap();
    let output: Value = serde_json::from_str(&output).unwrap();

    assert_eq!(output["parsed_ok"], true);
    assert_eq!(output["text"], "hello world");
}

#[test]
fn anthropic_stream_final_message_accepts_stdout_wrappers() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output =
        crate::runtime::claude_tools::workflow::anthropic_stream::execute_anthropic_stream(
            &mut state,
            &cwd,
            json!({
                "action": "finalMessage",
                "stream": {
                    "stdout": "{\"content\":[{\"type\":\"text\",\"text\":\"wrapped\"}]}"
                }
            }),
        )
        .unwrap();
    let output: Value = serde_json::from_str(&output).unwrap();

    assert_eq!(output["text"], "wrapped");
}
