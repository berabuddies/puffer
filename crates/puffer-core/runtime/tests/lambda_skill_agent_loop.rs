//! End-to-end coverage for model-selected verified Lambda Skills.

use super::*;
use puffer_resources::{LoadedItem, SkillSpec, SkillVerificationSpec, SourceInfo, SourceKind};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

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

fn spawn_server<F>(
    content_type: &'static str,
    expected_requests: usize,
    response_body: F,
) -> (String, thread::JoinHandle<()>)
where
    F: Fn(usize) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut handled = 0_usize;
        while handled < expected_requests && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    let mut buffer = vec![0_u8; 65_536];
                    let _ = stream.read(&mut buffer).unwrap();
                    let body = response_body(handled);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                    handled += 1;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("listener accept failed: {error}"),
            }
        }
    });
    (format!("http://{address}"), server)
}

fn openai_responses_skill_turn() -> String {
    json!({
        "id": "resp_skill",
        "output": [{
            "type": "function_call",
            "call_id": "call_skill",
            "name": "Skill",
            "arguments": "{\"skill\":\"issue-triage\"}"
        }],
        "usage": {
            "input_tokens": 100,
            "output_tokens": 5,
            "input_tokens_details": { "cached_tokens": 0 }
        }
    })
    .to_string()
}

fn openai_responses_final_turn() -> String {
    json!({
        "id": "resp_2",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": "all set"
            }]
        }],
        "output_text": "all set",
        "usage": {
            "input_tokens": 110,
            "output_tokens": 3,
            "input_tokens_details": { "cached_tokens": 5 }
        }
    })
    .to_string()
}

#[test]
fn openai_responses_model_selected_lambda_skill_scope_is_turn_local() {
    let temp = tempfile::tempdir().unwrap();
    let host_path = temp.path().join("verified-host.json");
    std::fs::write(
        &host_path,
        r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":"formal"}},"effects":[]}]}"#,
    )
    .unwrap();
    let (base_url, server) = spawn_server("application/json", 2, |index| {
        if index == 0 {
            openai_responses_skill_turn()
        } else {
            openai_responses_final_turn()
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(base_url));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");

    let mut state = AppState::new(
        PufferConfig::default(),
        temp.path().to_path_buf(),
        session_for(temp.path()),
    );
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    state.grant_all_tools_for_session();

    let mut skill_tool = loaded_tool("Skill", "Load a skill", "runtime:skill");
    skill_tool.value.approval_policy = Some("auto".to_string());
    skill_tool.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![skill_tool],
        skills: vec![LoadedItem {
            value: SkillSpec {
                name: "issue-triage".to_string(),
                description: "Triage issues".to_string(),
                content: "Use the verified issue triage harness.".to_string(),
                allowed_tools: vec!["ToolSearch".to_string()],
                disable_model_invocation: false,
                verification: Some(SkillVerificationSpec {
                    system: "lambda-skill".to_string(),
                    source_path: Some("skills/issue-triage/skill.lskill".to_string()),
                    generated_path: Some("skills/issue-triage/out/GENERATED.SKILL.md".to_string()),
                    host_catalogue_path: Some(host_path.display().to_string()),
                    compiler_path: None,
                    host_tool_bindings: Default::default(),
                    require_approval: false,
                    tools: Some(1),
                    actions: Some(1),
                }),
                ..SkillSpec::default()
            },
            source_info: SourceInfo {
                path: "skills/issue-triage/skill.lskill".into(),
                kind: SourceKind::Workspace,
            },
        }],
        ..LoadedResources::default()
    };

    let turn = execute_user_prompt(
        &mut state,
        &resources,
        &registry,
        &mut auth_store,
        "triage the current issues",
    )
    .unwrap();

    server.join().unwrap();
    assert_eq!(turn.assistant_text, "all set");
    assert_eq!(turn.tool_invocations[0].tool_id, "Skill");
    assert!(state.lambda_gate.is_none());
    assert!(state.pending_lambda_host_call.is_none());
}
