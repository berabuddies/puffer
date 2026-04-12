//! Multi-turn teammate execution loop.
//!
//! Each teammate runs as a long-lived thread that polls for messages,
//! executes agent turns, and idles until new work arrives.

use crate::{AppState, MessageRole};
use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Message sent to a running teammate via its mpsc channel.
#[derive(Debug, Clone)]
pub enum TeammateMessage {
    /// A regular incoming message from another agent or the leader.
    Incoming(IncomingMessage),
    /// A shutdown signal requesting the teammate to exit.
    Shutdown { request_id: String },
}

/// A simplified incoming message for the channel (avoids store visibility).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    pub from: String,
    pub text: String,
}

/// Handle to a running in-process teammate.
pub struct TeammateHandle {
    pub agent_id: String,
    pub message_tx: mpsc::Sender<TeammateMessage>,
}

/// Thread-safe registry of active in-process teammates.
pub type TeammateRegistry = Arc<Mutex<HashMap<String, mpsc::Sender<TeammateMessage>>>>;

/// Creates a new empty teammate registry.
pub fn new_teammate_registry() -> TeammateRegistry {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Global registry of active in-process teammates.
static REGISTRY: OnceLock<TeammateRegistry> = OnceLock::new();

/// Returns the global teammate registry, initializing it on first access.
pub fn teammate_registry() -> &'static TeammateRegistry {
    REGISTRY.get_or_init(new_teammate_registry)
}

/// Configuration for spawning a teammate loop.
pub struct TeammateLoopConfig {
    pub agent_id: String,
    pub agent_name: String,
    pub team_name: String,
    pub prompt: String,
    pub max_turns: Option<u32>,
    pub state: AppState,
    pub resources: LoadedResources,
    pub providers: ProviderRegistry,
    pub auth_store: AuthStore,
    pub output_file: PathBuf,
}

/// Spawns a teammate in a background thread with an mpsc channel for messages.
pub fn spawn_teammate(config: TeammateLoopConfig, registry: &TeammateRegistry) -> TeammateHandle {
    let (tx, rx) = mpsc::channel();
    let agent_id = config.agent_id.clone();

    registry
        .lock()
        .unwrap()
        .insert(agent_id.clone(), tx.clone());

    let registry_clone = Arc::clone(registry);
    let id_for_cleanup = agent_id.clone();
    std::thread::spawn(move || {
        teammate_loop(config, rx);
        registry_clone.lock().unwrap().remove(&id_for_cleanup);
    });

    TeammateHandle {
        agent_id,
        message_tx: tx,
    }
}

fn teammate_loop(mut config: TeammateLoopConfig, rx: mpsc::Receiver<TeammateMessage>) {
    let mut turn_count = 0u32;

    write_status(
        &config.output_file,
        &config.agent_id,
        "running",
        turn_count,
        None,
    );

    // First turn: execute the initial prompt.
    turn_count += 1;
    let first_result = crate::runtime::execute_user_prompt(
        &mut config.state,
        &config.resources,
        &config.providers,
        &mut config.auth_store,
        &config.prompt,
    );
    persist_turn_to_transcript(&mut config.state, &config.prompt, &first_result);
    append_turn_result(&config.output_file, turn_count, &first_result);

    if reached_max(config.max_turns, turn_count) {
        write_status(
            &config.output_file,
            &config.agent_id,
            "completed",
            turn_count,
            None,
        );
        return;
    }

    // Enter the poll loop.
    loop {
        // 1. Drain all available messages (non-blocking).
        let mut incoming: Vec<TeammateMessage> = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(msg) => incoming.push(msg),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        // 2. Check for shutdown.
        if let Some(req_id) = find_shutdown(&incoming) {
            write_status(
                &config.output_file,
                &config.agent_id,
                "stopped",
                turn_count,
                Some(&format!("shutdown (request_id: {req_id})")),
            );
            return;
        }

        // 3. Collect text from incoming messages.
        let texts: Vec<String> = incoming
            .iter()
            .filter_map(|m| match m {
                TeammateMessage::Incoming(msg) => {
                    Some(format!("[Message from {}]: {}", msg.from, msg.text))
                }
                _ => None,
            })
            .collect();

        // 4. If no messages, block-wait with timeout.
        if texts.is_empty() {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(TeammateMessage::Shutdown { request_id }) => {
                    write_status(
                        &config.output_file,
                        &config.agent_id,
                        "stopped",
                        turn_count,
                        Some(&format!("shutdown (request_id: {request_id})")),
                    );
                    return;
                }
                Ok(TeammateMessage::Incoming(msg)) => {
                    let prompt = format!("[Message from {}]: {}", msg.from, msg.text);
                    turn_count += 1;
                    let result = crate::runtime::execute_user_prompt(
                        &mut config.state,
                        &config.resources,
                        &config.providers,
                        &mut config.auth_store,
                        &prompt,
                    );
                    persist_turn_to_transcript(&mut config.state, &prompt, &result);
                    append_turn_result(&config.output_file, turn_count, &result);
                    if reached_max(config.max_turns, turn_count) {
                        write_status(
                            &config.output_file,
                            &config.agent_id,
                            "completed",
                            turn_count,
                            None,
                        );
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            continue;
        }

        // 5. Run one agent turn with accumulated messages.
        let combined = texts.join("\n\n");
        turn_count += 1;
        let result = crate::runtime::execute_user_prompt(
            &mut config.state,
            &config.resources,
            &config.providers,
            &mut config.auth_store,
            &combined,
        );
        persist_turn_to_transcript(&mut config.state, &combined, &result);
        append_turn_result(&config.output_file, turn_count, &result);

        if reached_max(config.max_turns, turn_count) {
            write_status(
                &config.output_file,
                &config.agent_id,
                "completed",
                turn_count,
                None,
            );
            return;
        }
    }
}

/// Persists a completed turn's context to `state.transcript` so the next turn's
/// `transcript_to_items()` has full conversation history (user prompt + tool
/// invocations + assistant reply).
fn persist_turn_to_transcript(
    state: &mut AppState,
    prompt: &str,
    result: &Result<crate::TurnExecution>,
) {
    if let Ok(turn) = result {
        state.push_message(MessageRole::User, prompt);
        for inv in &turn.tool_invocations {
            state.push_tool_invocation(
                &inv.call_id,
                &inv.tool_id,
                &inv.input,
                &inv.output,
                inv.success,
            );
        }
        state.push_message(MessageRole::Assistant, &turn.assistant_text);
    }
}

fn find_shutdown(msgs: &[TeammateMessage]) -> Option<String> {
    msgs.iter().find_map(|m| match m {
        TeammateMessage::Shutdown { request_id } => Some(request_id.clone()),
        _ => None,
    })
}

fn reached_max(max: Option<u32>, current: u32) -> bool {
    max.map_or(false, |m| current >= m)
}

fn write_status(output_file: &Path, agent_id: &str, status: &str, turns: u32, note: Option<&str>) {
    let payload = json!({
        "status": status,
        "agentId": agent_id,
        "turns": turns,
        "note": note,
    });
    let _ = std::fs::write(
        output_file,
        serde_json::to_string_pretty(&payload).unwrap_or_default(),
    );
}

fn append_turn_result(output_file: &Path, turn: u32, result: &Result<crate::TurnExecution>) {
    let previous = std::fs::read_to_string(output_file).unwrap_or_default();
    let entry = match result {
        Ok(exec) => format!(
            "\n--- Turn {turn} ---\n{}\nTool calls: {}\n",
            exec.assistant_text.trim(),
            exec.tool_invocations.len()
        ),
        Err(error) => format!("\n--- Turn {turn} ---\nError: {error}\n"),
    };
    let _ = std::fs::write(output_file, format!("{previous}{entry}"));
}
