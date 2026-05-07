//! End-to-end agent_loop test through the real TUI inside tmux.
//!
//! Spawns the actual `puffer` binary against a workspace-local
//! Anthropic provider override that points at an in-process
//! `TcpListener`. The mock returns a single non-streaming Messages
//! response. We send a prompt via tmux key events, then wait for the
//! assistant text to render in the pane. This exercises the full
//! stack: TUI ⇄ runtime::execute_user_prompt ⇄ adapter ⇄ agent_loop ⇄
//! AnthropicTurnSession ⇄ HTTP wire ⇄ mock.
//!
//! Skipped automatically when tmux is not available (CI on minimal
//! images).

use puffer_test_support::{
    capture_tmux_visible_pane, send_tmux_keys, start_tmux_command_with_size, temp_workspace,
    tmux_available, wait_for_tmux_text, TerminalSize,
};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Spawns a bg thread that accepts `expected_requests` connections and
/// replies to each with the SSE body returned by `response_body(index)`.
/// puffer's TUI dispatch routes through `execute_user_prompt_streaming`,
/// which sends `stream: true` and parses Anthropic SSE — replying with
/// plain JSON makes the SSE parser fail with "stream ended without
/// message_stop" (verified by running puffer manually against a JSON
/// mock during test development).
fn spawn_mock_anthropic<F>(
    expected_requests: usize,
    response_body: F,
) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>)
where
    F: Fn(usize) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requests);
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut handled = 0_usize;
        while handled < expected_requests && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = vec![0_u8; 65_536];
                    let bytes = stream.read(&mut buffer).unwrap_or(0);
                    if bytes == 0 {
                        continue;
                    }
                    let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
                    log.lock().unwrap().push(request);
                    let body = response_body(handled);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    handled += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => {
                    eprintln!("mock listener accept failed: {error}");
                    break;
                }
            }
        }
    });
    (format!("http://{address}"), requests, server)
}

fn sse_text_response(text: &str) -> String {
    format!(
        "event: message_start\n\
         data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_x\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-sonnet-4-5\",\"usage\":{{\"input_tokens\":12,\"output_tokens\":0}}}}}}\n\n\
         event: content_block_start\n\
         data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n\
         event: content_block_delta\n\
         data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{text}\"}}}}\n\n\
         event: content_block_stop\n\
         data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n\
         event: message_delta\n\
         data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":8}}}}\n\n\
         event: message_stop\n\
         data: {{\"type\":\"message_stop\"}}\n\n"
    )
}

fn sse_tool_use_response(tool_use_id: &str, tool_name: &str, input_json: &str) -> String {
    // The `input_json` is embedded into a content_block_delta as
    // partial_json. Backslash-escape per JSON rules so the SSE event
    // parses cleanly on the puffer side.
    let escaped = input_json.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "event: message_start\n\
         data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_t1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-sonnet-4-5\",\"usage\":{{\"input_tokens\":20,\"output_tokens\":0}}}}}}\n\n\
         event: content_block_start\n\
         data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"{tool_use_id}\",\"name\":\"{tool_name}\"}}}}\n\n\
         event: content_block_delta\n\
         data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{escaped}\"}}}}\n\n\
         event: content_block_stop\n\
         data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n\
         event: message_delta\n\
         data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}},\"usage\":{{\"output_tokens\":8}}}}\n\n\
         event: message_stop\n\
         data: {{\"type\":\"message_stop\"}}\n\n"
    )
}

/// Drops a workspace-level provider yaml that overrides the embedded
/// `anthropic` provider's `base_url` so puffer routes to our mock.
fn write_anthropic_override(workspace: &Path, mock_url: &str) {
    let provider_yaml = format!(
        r#"id: anthropic
display_name: Mock Anthropic
base_url: {mock_url}
default_api: anthropic-messages
auth_modes:
  - api_key
discovery: null
models:
  - id: claude-sonnet-4-5
    display_name: Mock Claude Sonnet 4.5
    provider: anthropic
    api: anthropic-messages
    context_window: 200000
    max_output_tokens: 8192
    supports_reasoning: false
"#,
    );
    let dir = workspace.join(".puffer/resources/providers");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("anthropic.yaml"), provider_yaml).unwrap();
}

fn write_config(workspace: &Path) {
    fs::create_dir_all(workspace.join(".puffer")).unwrap();
    fs::write(
        workspace.join(".puffer/config.toml"),
        r#"
app_name = "Puffer Code"
default_provider = "anthropic"
theme = "puffer"

[mascot]
id = "clawd"
display_name = "Clawd"
enabled = true

[ui]
no_alt_screen = true
tmux_golden_mode = true
"#,
    )
    .unwrap();
    fs::write(
        workspace.join(".puffer/auth.json"),
        r#"{
  "providers": {
    "anthropic": {
      "kind": "api_key",
      "key": "tmux-mock-key"
    }
  }
}"#,
    )
    .unwrap();
}

fn link_repo_resources(workspace: &Path) {
    // Some tools and agents resolve relative to a repo `resources/`
    // dir; symlink the repo's resources into the workspace so the
    // builtin filesystem layer finds them. (The provider override at
    // .puffer/resources/providers/anthropic.yaml takes precedence —
    // workspace > builtin.)
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    std::os::unix::fs::symlink(repo_root.join("resources"), workspace.join("resources")).unwrap();
}

#[test]
fn tmux_agent_loop_renders_assistant_reply_from_mock_anthropic() {
    if !tmux_available() {
        return;
    }

    let final_text = "puffer-tmux-agent-loop-ok";
    let final_text_owned = final_text.to_string();
    let (mock_url, requests, server) =
        spawn_mock_anthropic(1, move |_| sse_text_response(&final_text_owned));

    let (_tempdir, workspace) = temp_workspace().unwrap();
    link_repo_resources(workspace.as_path());
    write_anthropic_override(workspace.as_path(), &mock_url);
    write_config(workspace.as_path());

    let binary = env!("CARGO_BIN_EXE_puffer");
    let session = start_tmux_command_with_size(
        "sh",
        &[
            "-lc",
            // HOME=workspace makes `.puffer/` in the workspace be the
            // workspace config dir. Tracing PUFFER_HTTP_TRACE_PATH lets
            // post-mortem inspection see the wire bytes if the test fails.
            &format!(
                "HOME='{ws}' PUFFER_HTTP_TRACE_PATH='{ws}/wire.log' '{bin}'",
                ws = workspace.display(),
                bin = binary
            ),
        ],
        Some(workspace.as_path()),
        TerminalSize {
            cols: 120,
            rows: 30,
        },
    )
    .unwrap();

    // Wait for the TUI splash + prompt to finish rendering.
    wait_for_tmux_text(&session, "Puffer Code", Duration::from_secs(20)).unwrap();

    // Send a single user turn.
    send_tmux_keys(&session, &["say hi", "Enter"]).unwrap();

    // Wait for the assistant text to appear.
    let capture = wait_for_tmux_text(&session, final_text, Duration::from_secs(30))
        .expect("expected mock assistant reply to render in tmux pane");

    assert!(
        capture.contains(final_text),
        "tmux pane did not render mock reply:\n{capture}"
    );

    server.join().unwrap();
    let captured_requests = requests.lock().unwrap();
    assert_eq!(
        captured_requests.len(),
        1,
        "mock should have received exactly one Anthropic request"
    );

    // The request body must include our user prompt.
    let raw = &captured_requests[0];
    assert!(
        raw.contains("say hi"),
        "mock did not see the user prompt in the request body: {raw}"
    );
    assert!(
        raw.contains("anthropic-version"),
        "request missing anthropic-version header: {raw}"
    );
}

/// Multi-turn tool round-trip through the TUI. Mock first replies with
/// a `tool_use` block, then (after agent_loop runs the tool locally and
/// pushes the result) with the final text. Verifies the tmux pane
/// shows BOTH the tool execution and the final text — proving the
/// full agent_loop survives through the real TUI.
#[test]
fn tmux_agent_loop_drives_tool_round_trip_in_tui() {
    if !tmux_available() {
        return;
    }

    let final_text = "puffer-tmux-tool-round-trip-ok";
    let (_tempdir, workspace) = temp_workspace().unwrap();
    link_repo_resources(workspace.as_path());

    // Plant a fixture file the Read tool will read. Use the absolute
    // path because Anthropic Claude Code tools require absolute paths.
    let fixture_path = workspace.join("fixture.txt");
    fs::write(&fixture_path, "fixture-contents-here").unwrap();
    // macOS resolves `/var/folders/...` to `/private/var/folders/...`;
    // puffer's working_dirs ends up canonicalized via `cwd` (which is
    // the workspace), so the model-supplied path must match the
    // canonical form or the workspace-write sandbox blocks it with
    // "Path … is outside the current working directories".
    let fixture_path_string = fixture_path
        .canonicalize()
        .unwrap_or(fixture_path)
        .to_string_lossy()
        .to_string();

    let final_text_owned = final_text.to_string();
    let fixture_for_mock = fixture_path_string.clone();
    let (mock_url, requests, server) = spawn_mock_anthropic(2, move |index| {
        if index == 0 {
            // Tool name `Read` is the canonical Claude Code tool id;
            // `read_file` (snake_case) does NOT exist in the registry
            // and the agent_loop fails with "unknown tool read_file"
            // (verified manually against the running binary).
            sse_tool_use_response(
                "toolu_mock_1",
                "Read",
                &format!("{{\"file_path\":\"{}\"}}", fixture_for_mock),
            )
        } else {
            sse_text_response(&final_text_owned)
        }
    });

    write_anthropic_override(workspace.as_path(), &mock_url);
    write_config(workspace.as_path());

    let binary = env!("CARGO_BIN_EXE_puffer");
    let session = start_tmux_command_with_size(
        "sh",
        &[
            "-lc",
            &format!(
                "HOME='{ws}' PUFFER_HTTP_TRACE_PATH='{ws}/wire.log' '{bin}'",
                ws = workspace.display(),
                bin = binary
            ),
        ],
        Some(workspace.as_path()),
        TerminalSize {
            cols: 120,
            rows: 40,
        },
    )
    .unwrap();

    wait_for_tmux_text(&session, "Puffer Code", Duration::from_secs(20)).unwrap();
    send_tmux_keys(&session, &[&format!("read {fixture_path_string}"), "Enter"]).unwrap();
    let capture = match wait_for_tmux_text(&session, final_text, Duration::from_secs(40)) {
        Ok(capture) => capture,
        Err(error) => {
            // Dump the pane + the wire log so failures are debuggable.
            let pane = capture_tmux_visible_pane(&session)
                .unwrap_or_else(|_| "<failed to capture pane>".to_string());
            let wire = std::fs::read_to_string(workspace.join("wire.log"))
                .unwrap_or_else(|_| "<no wire log>".to_string());
            panic!(
                "expected final text after tool round trip: {error}\n--- pane ---\n{pane}\n--- wire ---\n{wire}"
            );
        }
    };

    assert!(
        capture.contains(final_text),
        "tmux pane missing final text:\n{capture}"
    );

    server.join().unwrap();
    let captured = requests.lock().unwrap();
    assert_eq!(
        captured.len(),
        2,
        "mock should have received exactly two Anthropic requests"
    );

    // Turn 2 must carry the tool_result back to the model.
    let body2 = &captured[1];
    assert!(
        body2.contains("tool_result"),
        "second request missing tool_result block: {body2}"
    );
    assert!(
        body2.contains("toolu_mock_1"),
        "second request must reference original tool_use_id: {body2}"
    );
    assert!(
        body2.contains("fixture-contents-here") || body2.contains("fixture-contents"),
        "second request should carry the tool output: {body2}"
    );

    let _ = capture_tmux_visible_pane(&session);
}

#[test]
fn tmux_agent_loop_answers_ask_user_question_with_keyboard_selection() {
    if !tmux_available() {
        return;
    }

    let final_text = "puffer-tmux-ask-user-question-ok";
    let (_tempdir, workspace) = temp_workspace().unwrap();
    link_repo_resources(workspace.as_path());

    let final_text_owned = final_text.to_string();
    let (mock_url, requests, server) = spawn_mock_anthropic(2, move |index| {
        if index == 0 {
            sse_tool_use_response(
                "toolu_question_1",
                "AskUserQuestion",
                r#"{"questions":[{"question":"Pick one","header":"Mode","options":[{"label":"Fast","description":"Prioritize speed"},{"label":"Careful","description":"Prioritize review"}]}]}"#,
            )
        } else {
            sse_text_response(&final_text_owned)
        }
    });

    write_anthropic_override(workspace.as_path(), &mock_url);
    write_config(workspace.as_path());

    let binary = env!("CARGO_BIN_EXE_puffer");
    let session = start_tmux_command_with_size(
        "sh",
        &[
            "-lc",
            &format!(
                "HOME='{ws}' PUFFER_HTTP_TRACE_PATH='{ws}/wire.log' '{bin}'",
                ws = workspace.display(),
                bin = binary
            ),
        ],
        Some(workspace.as_path()),
        TerminalSize {
            cols: 120,
            rows: 36,
        },
    )
    .unwrap();

    wait_for_tmux_text(&session, "Puffer Code", Duration::from_secs(20)).unwrap();
    send_tmux_keys(&session, &["ask me to choose", "Enter"]).unwrap();
    wait_for_tmux_text(&session, "Prioritize review", Duration::from_secs(30)).unwrap();
    send_tmux_keys(&session, &["Down", "Enter"]).unwrap();
    wait_for_tmux_text(&session, "Pick one: Careful", Duration::from_secs(30))
        .expect("expected selected AskUserQuestion answer to render in tmux");

    let capture = wait_for_tmux_text(&session, final_text, Duration::from_secs(40))
        .expect("expected final text after answering AskUserQuestion in tmux");
    assert!(
        capture.contains(final_text),
        "tmux pane missing final text:\n{capture}"
    );

    server.join().unwrap();
    let captured = requests.lock().unwrap();
    assert_eq!(
        captured.len(),
        2,
        "mock should have received AskUserQuestion round trip requests"
    );
    let body2 = &captured[1];
    assert!(
        body2.contains("tool_result"),
        "second request missing tool_result block: {body2}"
    );
    assert!(
        body2.contains("Pick one") && body2.contains("Careful"),
        "second request should carry the selected answer: {body2}"
    );
}
