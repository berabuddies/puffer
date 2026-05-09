use super::*;
use crate::runtime::tests::refresh_env_lock;
use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
use puffer_provider_registry::{AuthMode, AuthStore, ProviderDescriptor, ProviderRegistry};
use puffer_resources::{AgentSpec, LoadedItem, LoadedResources, SourceInfo, SourceKind};
use puffer_session_store::SessionMetadata;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::thread;
use uuid::Uuid;

fn provider() -> ProviderDescriptor {
    ProviderDescriptor {
        id: "anthropic".to_string(),
        display_name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        default_api: "anthropic-messages".to_string(),
        auth_modes: vec![AuthMode::ApiKey],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![puffer_provider_registry::ModelDescriptor {
            id: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            provider: "anthropic".to_string(),
            api: "anthropic-messages".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    }
}

fn loaded_agent(
    id: &str,
    description: &str,
    prompt: &str,
    tools: &[&str],
) -> LoadedItem<AgentSpec> {
    LoadedItem {
        value: AgentSpec {
            id: id.to_string(),
            description: description.to_string(),
            prompt: prompt.to_string(),
            tools: tools.iter().map(|tool| tool.to_string()).collect(),
            model: None,
            ..AgentSpec::default()
        },
        source_info: SourceInfo {
            path: format!("{id}.yaml").into(),
            kind: SourceKind::Builtin,
        },
    }
}

fn init_git_repo(root: &Path) {
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());
    let status = Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());
    let status = Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    let status = Command::new("git")
        .args(["add", "README.md"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());
    let status = Command::new("git")
        .args(["commit", "-qm", "init"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn execute_agent_tool_background_returns_async_payload_and_output_file() {
    let temp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = json!({
                "content": [
                    {
                        "type": "text",
                        "text": "background ok"
                    }
                ]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut provider = provider();
    provider.id = "local-anthropic".to_string();
    provider.base_url = format!("http://{address}");
    provider.auth_modes.clear();
    provider.models[0].provider = "local-anthropic".to_string();

    let mut providers = ProviderRegistry::new();
    providers.register(provider);
    let session = SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
        generated_title: None,
        cwd: temp.path().to_path_buf(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let mut state = AppState::new(PufferConfig::default(), temp.path().to_path_buf(), session);
    state.current_provider = Some("local-anthropic".to_string());
    state.current_model = Some("local-anthropic/claude-sonnet-4-5".to_string());

    let resources = LoadedResources {
        agents: vec![loaded_agent(
            "general-purpose",
            "Default agent",
            "You are a coding subagent.",
            &["read_file"],
        )],
        ..LoadedResources::default()
    };
    let output = super::agents::execute_agent_tool(
        &state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        temp.path(),
        json!({
            "description": "Background request",
            "prompt": "Do the thing",
            "run_in_background": true
        }),
    )
    .unwrap();
    let payload: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(payload["status"], "async_launched");
    let output_file = payload["outputFile"].as_str().unwrap();
    let agent_id = payload["agentId"].as_str().unwrap().to_string();
    assert!(Path::new(output_file).exists());

    let actions = crate::render_task_actions(&mut state).unwrap();
    assert!(actions
        .iter()
        .any(|entry| entry.command == format!("/tasks show {agent_id}")));
    assert!(actions
        .iter()
        .any(|entry| entry.command == format!("/tasks output {agent_id}")));
    assert!(!actions
        .iter()
        .any(|entry| entry.command == format!("/tasks stop {agent_id}")));

    let agents_panel = crate::render_tasks_panel_text(&mut state, "agents")
        .unwrap()
        .expect("agents panel");
    assert!(agents_panel.contains("Background agents:"));
    assert!(agents_panel.contains(&agent_id));
    assert!(agents_panel.contains("Background request"));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let raw = std::fs::read_to_string(output_file).unwrap();
        let written: Value = serde_json::from_str(&raw).unwrap();
        if written["status"] == "completed" {
            assert_eq!(written["result"], "background ok");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "background agent did not finish in time"
        );
        thread::sleep(std::time::Duration::from_millis(20));
    }
    server.join().unwrap();
}

#[test]
fn execute_agent_tool_sync_reports_worktree_isolation_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = refresh_env_lock().lock().unwrap();
    let old_home = std::env::var_os("PUFFER_HOME");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("PUFFER_HOME", &home);
    init_git_repo(temp.path());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = json!({
                "content": [
                    {
                        "type": "text",
                        "text": "sync ok"
                    }
                ]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut provider = provider();
    provider.id = "local-anthropic".to_string();
    provider.base_url = format!("http://{address}");
    provider.auth_modes.clear();
    provider.models[0].provider = "local-anthropic".to_string();

    let mut providers = ProviderRegistry::new();
    providers.register(provider);
    let session = SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
        generated_title: None,
        cwd: temp.path().to_path_buf(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let mut state = AppState::new(PufferConfig::default(), temp.path().to_path_buf(), session);
    state.current_provider = Some("local-anthropic".to_string());
    state.current_model = Some("local-anthropic/claude-sonnet-4-5".to_string());
    super::claude_tools::workflow::team_create::execute_team_create(
        &mut state,
        temp.path(),
        json!({
            "team_name": "alpha",
            "description": "Coordination team"
        }),
    )
    .unwrap();

    let resources = LoadedResources {
        agents: vec![loaded_agent(
            "general-purpose",
            "Default agent",
            "You are a coding subagent.",
            &["read_file"],
        )],
        ..LoadedResources::default()
    };
    let output = super::agents::execute_agent_tool(
        &state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        temp.path(),
        json!({
            "description": "Sync request",
            "prompt": "Do the thing",
            "isolation": "worktree",
            "team_name": "alpha",
            "name": "researcher",
            "mode": "plan"
        }),
    )
    .unwrap();
    let payload: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(payload["status"], "completed");
    assert_eq!(payload["isolation"], "worktree");
    assert_eq!(payload["teamName"], "alpha");
    assert_eq!(payload["mode"], "plan");
    assert!(payload["worktreePath"].as_str().is_some());
    assert!(payload["worktreeBranch"].as_str().is_some());
    let team_file: Value = serde_json::from_str(
        &std::fs::read_to_string(home.join(".claude/teams/alpha/config.json")).unwrap(),
    )
    .unwrap();
    assert!(team_file["members"]
        .as_array()
        .is_some_and(|members| members.iter().any(|member| member["name"] == "researcher")));
    if let Some(value) = old_home {
        std::env::set_var("PUFFER_HOME", value);
    } else {
        std::env::remove_var("PUFFER_HOME");
    }
    server.join().unwrap();
}

#[test]
fn execute_agent_tool_loads_workspace_agent_resources_from_disk() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();
    std::fs::create_dir_all(paths.workspace_config_dir.join("resources/agents")).unwrap();
    std::fs::write(
        paths.workspace_config_dir.join("resources/agents/reviewer.yaml"),
        "id: reviewer\ndescription: Reviews code carefully.\nprompt: |\n  You are a reviewer.\ntools:\n  - Read\nmodel: local-anthropic/claude-sonnet-4-5\n",
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = json!({
                "content": [
                    {
                        "type": "text",
                        "text": "disk agent ok"
                    }
                ]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut provider = provider();
    provider.id = "local-anthropic".to_string();
    provider.base_url = format!("http://{address}");
    provider.auth_modes.clear();
    provider.models[0].provider = "local-anthropic".to_string();

    let mut providers = ProviderRegistry::new();
    providers.register(provider);
    let session = SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
        generated_title: None,
        cwd: temp.path().to_path_buf(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let mut state = AppState::new(PufferConfig::default(), temp.path().to_path_buf(), session);
    state.current_provider = Some("local-anthropic".to_string());
    state.current_model = Some("local-anthropic/claude-sonnet-4-5".to_string());

    let output = super::agents::execute_agent_tool(
        &state,
        &LoadedResources::default(),
        &providers,
        &mut AuthStore::default(),
        temp.path(),
        json!({
            "description": "Workspace agent request",
            "prompt": "Review the worktree",
            "subagent_type": "reviewer"
        }),
    )
    .unwrap();
    let payload: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(payload["status"], "completed");
    assert_eq!(payload["agentType"], "reviewer");
    assert_eq!(payload["result"], "disk agent ok");
    server.join().unwrap();
}

#[test]
fn execute_agent_tool_background_preserves_worktree_path() {
    let temp = tempfile::tempdir().unwrap();
    init_git_repo(temp.path());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = json!({
                "content": [
                    {
                        "type": "text",
                        "text": "background worktree ok"
                    }
                ]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut provider = provider();
    provider.id = "local-anthropic".to_string();
    provider.base_url = format!("http://{address}");
    provider.auth_modes.clear();
    provider.models[0].provider = "local-anthropic".to_string();

    let mut providers = ProviderRegistry::new();
    providers.register(provider);
    let session = SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
        generated_title: None,
        cwd: temp.path().to_path_buf(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let mut state = AppState::new(PufferConfig::default(), temp.path().to_path_buf(), session);
    state.current_provider = Some("local-anthropic".to_string());
    state.current_model = Some("local-anthropic/claude-sonnet-4-5".to_string());

    let resources = LoadedResources {
        agents: vec![loaded_agent(
            "general-purpose",
            "Default agent",
            "You are a coding subagent.",
            &["read_file"],
        )],
        ..LoadedResources::default()
    };
    let output = super::agents::execute_agent_tool(
        &state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        temp.path(),
        json!({
            "description": "Background request",
            "prompt": "Do the thing",
            "run_in_background": true,
            "isolation": "worktree"
        }),
    )
    .unwrap();
    let payload: Value = serde_json::from_str(&output).unwrap();
    let worktree_path = payload["worktreePath"].as_str().unwrap();
    assert!(Path::new(worktree_path).exists());
    server.join().unwrap();
}

#[test]
fn execute_agent_tool_combines_initial_prompt_skills_and_case_insensitive_model() {
    let temp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let captured_request = captured.clone();
    let server = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 16384];
            let read = stream.read(&mut buffer).unwrap_or_default();
            *captured_request.lock().unwrap() =
                String::from_utf8_lossy(&buffer[..read]).to_string();
            let body = json!({
                "output": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [
                            {
                                "type": "output_text",
                                "text": "agent ok"
                            }
                        ]
                    }
                ]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut provider = provider();
    provider.id = "openai".to_string();
    provider.base_url = format!("http://{address}");
    provider.default_api = "openai-responses".to_string();
    provider.auth_modes.clear();
    provider.models[0].id = "gpt-5".to_string();
    provider.models[0].display_name = "GPT-5".to_string();
    provider.models[0].provider = "openai".to_string();
    provider.models[0].api = "openai-responses".to_string();

    let mut providers = ProviderRegistry::new();
    providers.register(provider);
    let session = SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
        generated_title: None,
        cwd: temp.path().to_path_buf(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let mut state = AppState::new(PufferConfig::default(), temp.path().to_path_buf(), session);
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());

    let mut agent = loaded_agent("reviewer", "Reviews code", "You are a reviewer.", &["Read"]);
    agent.value.skills = vec!["reviewer".to_string()];
    agent.value.initial_prompt = Some("Start with the review plan.".to_string());
    agent.value.effort = Some("high".to_string());
    let resources = LoadedResources {
        agents: vec![agent],
        skills: vec![LoadedItem {
            value: puffer_resources::SkillSpec {
                name: "reviewer".to_string(),
                description: "Review skill".to_string(),
                content: "Check correctness first.".to_string(),
                disable_model_invocation: false,
                ..puffer_resources::SkillSpec::default()
            },
            source_info: SourceInfo {
                path: "reviewer.md".into(),
                kind: SourceKind::Builtin,
            },
        }],
        ..LoadedResources::default()
    };

    let output = super::agents::execute_agent_tool(
        &state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        temp.path(),
        json!({
            "description": "Review request",
            "prompt": "Inspect the change",
            "subagent_type": "reviewer",
            "model": "OPENAI/GPT-5"
        }),
    )
    .unwrap();
    let payload: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(payload["status"], "completed");
    assert_eq!(
        payload["prompt"],
        "Start with the review plan.\n\nInspect the change"
    );
    assert_eq!(payload["effort"], "high");
    assert_eq!(payload["model"], "openai/gpt-5");

    let raw_request = captured.lock().unwrap().clone();
    assert!(raw_request.contains("<skill name"));
    assert!(raw_request.contains("Check correctness first."));
    server.join().unwrap();
}

#[test]
fn execute_agent_tool_rejects_missing_required_mcp_servers() {
    let temp = tempfile::tempdir().unwrap();
    let session = SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
        generated_title: None,
        cwd: temp.path().to_path_buf(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let state = AppState::new(PufferConfig::default(), temp.path().to_path_buf(), session);
    let mut agent = loaded_agent("reviewer", "Reviews code", "You are a reviewer.", &["Read"]);
    agent.value.required_mcp_servers = vec!["docs".to_string()];
    let error = super::agents::execute_agent_tool(
        &state,
        &LoadedResources {
            agents: vec![agent],
            ..LoadedResources::default()
        },
        &ProviderRegistry::new(),
        &mut AuthStore::default(),
        temp.path(),
        json!({
            "description": "Review request",
            "prompt": "Inspect the change",
            "subagent_type": "reviewer"
        }),
    )
    .unwrap_err();
    assert!(error.to_string().contains("required MCP servers"));
}
