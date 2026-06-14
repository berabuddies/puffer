//! End-to-end coverage for action-obligated skills in the streaming loop.

use super::*;
use puffer_resources::{LoadedItem, SkillSpec, SourceInfo, SourceKind};

fn session_for(cwd: &std::path::Path) -> SessionMetadata {
    SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
        generated_title: None,
        cwd: cwd.to_path_buf(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    }
}

fn loaded_skill(name: &str, requires_action: bool) -> LoadedItem<SkillSpec> {
    LoadedItem {
        value: SkillSpec {
            name: name.to_string(),
            description: format!("{name} description"),
            content: format!("{name} body"),
            requires_action,
            ..SkillSpec::default()
        },
        source_info: SourceInfo {
            path: format!("skills/{name}/SKILL.md").into(),
            kind: SourceKind::Builtin,
        },
    }
}

fn sse_event(event: &str, data: Value) -> String {
    format!("event: {event}\ndata: {}\n\n", data)
}

fn openai_responses_function_call_sse(
    response_id: &str,
    item_id: &str,
    call_id: &str,
    name: &str,
    arguments: &str,
) -> String {
    let mut body = String::new();
    body.push_str(&sse_event(
        "response.created",
        json!({"type":"response.created","response":{"id":response_id}}),
    ));
    body.push_str(&sse_event(
        "response.output_item.added",
        json!({
            "type":"response.output_item.added",
            "output_index":0,
            "item":{
                "type":"function_call",
                "id":item_id,
                "call_id":call_id,
                "name":name,
                "arguments":""
            }
        }),
    ));
    body.push_str(&sse_event(
        "response.function_call_arguments.delta",
        json!({
            "type":"response.function_call_arguments.delta",
            "item_id":item_id,
            "delta":arguments
        }),
    ));
    body.push_str(&sse_event(
        "response.output_item.done",
        json!({
            "type":"response.output_item.done",
            "item":{
                "type":"function_call",
                "id":item_id,
                "call_id":call_id,
                "name":name,
                "arguments":arguments
            }
        }),
    ));
    body.push_str(&sse_event(
        "response.completed",
        json!({
            "type":"response.completed",
            "response":{
                "id":response_id,
                "status":"completed",
                "usage":{
                    "input_tokens":100,
                    "output_tokens":5,
                    "input_tokens_details":{"cached_tokens":0}
                }
            }
        }),
    ));
    body
}

fn openai_responses_text_sse(response_id: &str, text: &str) -> String {
    let mut body = String::new();
    body.push_str(&sse_event(
        "response.created",
        json!({"type":"response.created","response":{"id":response_id}}),
    ));
    body.push_str(&sse_event(
        "response.output_text.delta",
        json!({"type":"response.output_text.delta","delta":text}),
    ));
    body.push_str(&sse_event(
        "response.output_item.done",
        json!({
            "type":"response.output_item.done",
            "item":{
                "type":"message",
                "role":"assistant",
                "content":[{"type":"output_text","text":text}]
            }
        }),
    ));
    body.push_str(&sse_event(
        "response.completed",
        json!({
            "type":"response.completed",
            "response":{
                "id":response_id,
                "status":"completed",
                "usage":{
                    "input_tokens":110,
                    "output_tokens":3,
                    "input_tokens_details":{"cached_tokens":5}
                }
            }
        }),
    ));
    body
}

fn resources() -> LoadedResources {
    LoadedResources {
        tools: vec![
            loaded_tool("Skill", "Load a skill", "runtime:skill"),
            loaded_tool("Write", "Write file", "runtime:claude_write"),
        ],
        skills: vec![loaded_skill("short-drama-generation", true)],
        ..LoadedResources::default()
    }
}

fn openai_state(cwd: &std::path::Path) -> AppState {
    let mut state = AppState::new(PufferConfig::default(), cwd.to_path_buf(), session_for(cwd));
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    state.grant_all_tools_for_session();
    state
}

#[test]
fn openai_responses_skill_action_obligation_reminds_then_accepts_write() {
    let temp = tempfile::tempdir().unwrap();
    let write_path = temp.path().join("started.txt");
    let write_args =
        json!({"file_path": write_path.display().to_string(), "content": "started"}).to_string();
    let (base_url, requests, server) =
        spawn_server("text/event-stream", 4, move |index| match index {
            0 => openai_responses_function_call_sse(
                "resp_skill",
                "fc_skill",
                "call_skill",
                "Skill",
                r#"{"skill":"short-drama-generation"}"#,
            ),
            1 => openai_responses_text_sse("resp_promise", "I'll start..."),
            2 => openai_responses_function_call_sse(
                "resp_write",
                "fc_write",
                "call_write",
                "Write",
                &write_args,
            ),
            _ => openai_responses_text_sse("resp_final", "done"),
        });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(base_url));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");

    let mut state = openai_state(temp.path());
    let resources = resources();
    let turn = execute_user_prompt_streaming(
        &mut state,
        &resources,
        &registry,
        &mut auth_store,
        "make a short drama",
        |_| {},
    )
    .unwrap();

    server.join().unwrap();

    assert_eq!(turn.assistant_text, "done");
    assert!(turn
        .tool_invocations
        .iter()
        .any(|invocation| invocation.tool_id == "Write" && invocation.success));
    assert_eq!(
        requests.lock().unwrap().len(),
        4,
        "the promise-only second response must not end the turn"
    );
}

#[test]
fn openai_responses_skill_action_obligation_fails_after_second_no_tool_text() {
    let temp = tempfile::tempdir().unwrap();
    let (base_url, requests, server) = spawn_server("text/event-stream", 3, |index| match index {
        0 => openai_responses_function_call_sse(
            "resp_skill",
            "fc_skill",
            "call_skill",
            "Skill",
            r#"{"skill":"short-drama-generation"}"#,
        ),
        1 => openai_responses_text_sse("resp_promise", "I'll start..."),
        _ => openai_responses_text_sse("resp_still_promise", "I will begin now."),
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(base_url));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");

    let mut state = openai_state(temp.path());
    let resources = resources();
    let turn = execute_user_prompt_streaming(
        &mut state,
        &resources,
        &registry,
        &mut auth_store,
        "make a short drama",
        |_| {},
    )
    .unwrap();

    server.join().unwrap();

    assert!(turn.assistant_text.contains("No work was started"));
    assert_eq!(
        requests.lock().unwrap().len(),
        3,
        "the second promise-only response should fail without another retry"
    );
}

#[test]
fn openai_responses_skill_action_obligation_fails_when_turn_budget_expires() {
    let temp = tempfile::tempdir().unwrap();
    let (base_url, requests, server) = spawn_server("text/event-stream", 1, |_| {
        openai_responses_function_call_sse(
            "resp_skill",
            "fc_skill",
            "call_skill",
            "Skill",
            r#"{"skill":"short-drama-generation"}"#,
        )
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(base_url));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");

    let mut state = openai_state(temp.path());
    let resources = resources();
    let turn = execute_user_prompt_streaming_with_options(
        &mut state,
        &resources,
        &registry,
        &mut auth_store,
        "make a short drama",
        TurnRequestOptions {
            max_turns: Some(1),
            ..TurnRequestOptions::default()
        },
        &mut |_| {},
    )
    .unwrap();

    server.join().unwrap();

    assert!(turn.assistant_text.contains("No work was started"));
    assert_eq!(turn.tool_invocations.len(), 1);
    assert_eq!(requests.lock().unwrap().len(), 1);
}
