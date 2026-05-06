//! Drives a single agent turn from the Tauri host.
//!
//! Mirrors the pattern that `puffer-tui/src/flow.rs::handle_prompt_submit`
//! uses: clone state into a worker thread, stream TurnStreamEvents back via
//! an mpsc channel, block the permission-prompt callback on a one-shot
//! response channel, and persist events after the turn finishes.
//!
//! On the Tauri side we surface three commands:
//!   * `run_agent_turn(session_id, message) -> turn_id` — starts a turn and
//!     emits `session:{id}:event` events while it runs.
//!   * `resolve_permission(turn_id, request_id, action)` — answers a paused
//!     permission prompt.
//!   * `cancel_turn(turn_id)` — best-effort interrupt (denies any pending
//!     permission and drops the emitter so future events are ignored).

use anyhow::{Context, Result};
use indexmap::IndexMap;
use puffer_config::{ensure_workspace_dirs, load_config, ConfigPaths};
use puffer_core::{
    execute_user_turn_streaming_with_permissions, with_user_question_prompt_handler, AppState,
    MessageRole, PermissionPromptAction, PermissionPromptRequest, TurnStreamEvent,
    UserQuestionPromptRequest, UserQuestionPromptResponse,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::load_resources;
use puffer_session_store::{SessionStore, TranscriptEvent};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

/// Registry of turns currently in flight. Inserted on spawn, removed when the
/// worker finishes. Permission responders are keyed by `(turn_id, request_id)`.
#[derive(Default)]
pub(crate) struct TurnRegistry {
    inner: Mutex<HashMap<String, TurnEntry>>,
}

struct TurnEntry {
    cancel: Arc<AtomicBool>,
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<PermissionPromptAction>>>>,
    pending_questions: Arc<Mutex<HashMap<String, mpsc::Sender<UserQuestionPromptResponse>>>>,
    // Tracks the next permission-request id so the callback thread can
    // hand the UI a stable label to reply to.
    next_request_id: Arc<AtomicU64>,
}

impl TurnEntry {
    #[allow(dead_code)]
    fn next_request_id(&self) -> &Arc<AtomicU64> {
        &self.next_request_id
    }
}

impl TurnRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn insert(&self, turn_id: String, entry: TurnEntry) {
        self.inner.lock().unwrap().insert(turn_id, entry);
    }

    fn remove(&self, turn_id: &str) -> Option<TurnEntry> {
        self.inner.lock().unwrap().remove(turn_id)
    }

    fn with<R>(&self, turn_id: &str, f: impl FnOnce(&TurnEntry) -> R) -> Option<R> {
        self.inner.lock().unwrap().get(turn_id).map(f)
    }
}

/// Payload emitted on `session:{id}:event`.
#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum EmittedEvent {
    #[serde(rename_all = "camelCase")]
    TurnStart { turn_id: String },
    #[serde(rename_all = "camelCase")]
    TextDelta { turn_id: String, delta: String },
    #[serde(rename_all = "camelCase")]
    ThinkingDelta { turn_id: String, delta: String },
    #[serde(rename_all = "camelCase")]
    ToolCallsRequested {
        turn_id: String,
        requests: serde_json::Value,
    },
    #[serde(rename_all = "camelCase")]
    ToolInvocations {
        turn_id: String,
        invocations: serde_json::Value,
    },
    #[serde(rename_all = "camelCase")]
    Usage {
        turn_id: String,
        report: serde_json::Value,
    },
    #[serde(rename_all = "camelCase")]
    ReflectionCheckpoint { turn_id: String, summary: String },
    #[serde(rename_all = "camelCase")]
    RetryAttempt {
        turn_id: String,
        attempt: usize,
        max_attempts: usize,
        error: String,
    },
    #[serde(rename_all = "camelCase")]
    PermissionRequest {
        turn_id: String,
        request_id: String,
        tool_id: String,
        summary: String,
        reason: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    UserQuestionRequest {
        turn_id: String,
        request_id: String,
        questions: serde_json::Value,
    },
    #[serde(rename_all = "camelCase")]
    TurnComplete {
        turn_id: String,
        assistant_text: String,
    },
    #[serde(rename_all = "camelCase")]
    TurnError { turn_id: String, error: String },
}

/// `run_agent_turn(session_id, message) -> turn_id`.
///
/// Loads the session's AppState, resources, providers, and auth store; spawns
/// a worker thread to drive the turn; emits events on `session:{id}:event`.
/// Returns the generated `turn_id` so the caller can correlate incoming events
/// and send permission responses.
pub(crate) fn run_agent_turn(
    app: AppHandle,
    registry: Arc<TurnRegistry>,
    session_id: String,
    message: String,
) -> Result<String, String> {
    let turn_id = Uuid::new_v4().to_string();
    let event_channel = format!("session:{session_id}:event");

    // Load everything the runtime needs up-front so we can fail early instead
    // of inside the worker thread.
    let (config, resources, providers, auth_store, session_store, state) =
        load_context(&session_id).map_err(|err| err.to_string())?;

    let cancel = Arc::new(AtomicBool::new(false));
    let pending: Arc<Mutex<HashMap<String, mpsc::Sender<PermissionPromptAction>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_questions: Arc<Mutex<HashMap<String, mpsc::Sender<UserQuestionPromptResponse>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let next_request_id = Arc::new(AtomicU64::new(0));

    registry.insert(
        turn_id.clone(),
        TurnEntry {
            cancel: cancel.clone(),
            pending: pending.clone(),
            pending_questions: pending_questions.clone(),
            next_request_id: next_request_id.clone(),
        },
    );

    // Announce start before we spawn so JS can subscribe deterministically.
    let _ = app.emit(
        &event_channel,
        EmittedEvent::TurnStart {
            turn_id: turn_id.clone(),
        },
    );

    let spawn_turn_id = turn_id.clone();
    let spawn_session_id = session_id.clone();
    thread::spawn(move || {
        let result = drive_turn(
            app.clone(),
            event_channel.clone(),
            spawn_turn_id.clone(),
            spawn_session_id.clone(),
            message,
            config,
            resources,
            providers,
            auth_store,
            session_store,
            state,
            cancel,
            pending,
            pending_questions,
            next_request_id,
        );

        match result {
            Ok(assistant_text) => {
                let _ = app.emit(
                    &event_channel,
                    EmittedEvent::TurnComplete {
                        turn_id: spawn_turn_id.clone(),
                        assistant_text,
                    },
                );
            }
            Err(err) => {
                let _ = app.emit(
                    &event_channel,
                    EmittedEvent::TurnError {
                        turn_id: spawn_turn_id.clone(),
                        error: err.to_string(),
                    },
                );
            }
        }

        let _ = registry.remove(&spawn_turn_id);
    });

    Ok(turn_id)
}

/// Pending permission resolution — `action` is one of
/// `"allow_once" | "allow_session" | "allow_all_session" | "deny"`.
pub(crate) fn resolve_permission(
    registry: Arc<TurnRegistry>,
    turn_id: String,
    request_id: String,
    action: String,
) -> Result<(), String> {
    let decoded = match action.as_str() {
        "allow_once" => PermissionPromptAction::AllowOnce,
        "allow_session" => PermissionPromptAction::AllowSession,
        "allow_all_session" => PermissionPromptAction::AllowAllSession,
        "deny" => PermissionPromptAction::Deny,
        other => return Err(format!("unknown permission action `{other}`")),
    };
    let responder = registry
        .with(&turn_id, |entry| {
            entry.pending.lock().unwrap().remove(&request_id)
        })
        .flatten()
        .ok_or_else(|| {
            format!("no pending permission request `{request_id}` on turn `{turn_id}`")
        })?;
    responder
        .send(decoded)
        .map_err(|_| "agent worker already released the permission channel".to_string())
}

/// Resolves a pending `AskUserQuestion` prompt for an in-flight turn.
pub(crate) fn resolve_user_question(
    registry: Arc<TurnRegistry>,
    turn_id: String,
    request_id: String,
    answers: serde_json::Map<String, serde_json::Value>,
    annotations: serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let responder = registry
        .with(&turn_id, |entry| {
            entry.pending_questions.lock().unwrap().remove(&request_id)
        })
        .flatten()
        .ok_or_else(|| {
            format!("no pending user question request `{request_id}` on turn `{turn_id}`")
        })?;
    responder
        .send(UserQuestionPromptResponse {
            answers,
            annotations,
        })
        .map_err(|_| "agent worker already released the user question channel".to_string())
}

/// Best-effort cancel: flips the cancel flag and denies any outstanding
/// permission request. The running turn will finish its current provider
/// stream then exit.
pub(crate) fn cancel_turn(registry: Arc<TurnRegistry>, turn_id: String) -> Result<(), String> {
    registry
        .with(&turn_id, |entry| {
            entry.cancel.store(true, Ordering::SeqCst);
            let mut pending = entry.pending.lock().unwrap();
            for (_, tx) in pending.drain() {
                let _ = tx.send(PermissionPromptAction::Deny);
            }
            let mut pending_questions = entry.pending_questions.lock().unwrap();
            for (_, tx) in pending_questions.drain() {
                let _ = tx.send(UserQuestionPromptResponse {
                    answers: serde_json::Map::new(),
                    annotations: serde_json::Map::new(),
                });
            }
        })
        .ok_or_else(|| format!("no active turn `{turn_id}`"))
}

// ---------------------------------------------------------------------------
// Worker-thread driver
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn drive_turn(
    app: AppHandle,
    event_channel: String,
    turn_id: String,
    session_id: String,
    message: String,
    _config: puffer_config::PufferConfig,
    resources: puffer_resources::LoadedResources,
    providers: ProviderRegistry,
    auth_store: AuthStore,
    session_store: SessionStore,
    mut state: AppState,
    _cancel: Arc<AtomicBool>,
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<PermissionPromptAction>>>>,
    pending_questions: Arc<Mutex<HashMap<String, mpsc::Sender<UserQuestionPromptResponse>>>>,
    next_request_id: Arc<AtomicU64>,
) -> Result<String> {
    // Persist the user message to the session transcript before the turn
    // starts so the UI can reload and still see it even if the turn crashes.
    state.push_message(MessageRole::User, message.clone());
    let session_uuid = state.session.id;
    session_store.append_event(
        session_uuid,
        TranscriptEvent::UserMessage {
            text: message.clone(),
        },
    )?;

    let mut auth_store = auth_store;

    let on_event_app = app.clone();
    let on_event_channel = event_channel.clone();
    let on_event_turn_id = turn_id.clone();
    let on_event = move |event: TurnStreamEvent| {
        let payload = match event {
            TurnStreamEvent::ThinkingDelta(delta) => EmittedEvent::ThinkingDelta {
                turn_id: on_event_turn_id.clone(),
                delta,
            },
            TurnStreamEvent::TextDelta(delta) => EmittedEvent::TextDelta {
                turn_id: on_event_turn_id.clone(),
                delta,
            },
            TurnStreamEvent::ToolCallsRequested(requests) => EmittedEvent::ToolCallsRequested {
                turn_id: on_event_turn_id.clone(),
                requests: serde_json::Value::Array(
                    requests
                        .into_iter()
                        .map(|r| {
                            json!({
                                "callId": r.call_id,
                                "toolId": r.tool_id,
                                "input": r.input,
                            })
                        })
                        .collect(),
                ),
            },
            TurnStreamEvent::ToolInvocations(invocations) => EmittedEvent::ToolInvocations {
                turn_id: on_event_turn_id.clone(),
                invocations: serde_json::Value::Array(
                    invocations
                        .into_iter()
                        .map(|i| {
                            json!({
                                "callId": i.call_id,
                                "toolId": i.tool_id,
                                "input": i.input,
                                "output": i.output,
                                "success": i.success,
                            })
                        })
                        .collect(),
                ),
            },
            TurnStreamEvent::Usage(report) => EmittedEvent::Usage {
                turn_id: on_event_turn_id.clone(),
                report: json!({
                    "inputTokens": report.input_tokens,
                    "outputTokens": report.output_tokens,
                    "cacheReadTokens": report.cache_read_tokens,
                    "cacheCreationTokens": report.cache_creation_tokens,
                }),
            },
            TurnStreamEvent::ReflectionCheckpoint(summary) => EmittedEvent::ReflectionCheckpoint {
                turn_id: on_event_turn_id.clone(),
                summary,
            },
            TurnStreamEvent::ReflectionTrace(_) => return,
            TurnStreamEvent::RetryAttempt {
                attempt,
                max_attempts,
                error,
            } => EmittedEvent::RetryAttempt {
                turn_id: on_event_turn_id.clone(),
                attempt,
                max_attempts,
                error,
            },
        };
        let _ = on_event_app.emit(&on_event_channel, payload);
    };

    let on_perm_app = app.clone();
    let on_perm_channel = event_channel.clone();
    let on_perm_turn_id = turn_id.clone();
    let on_perm_pending = pending.clone();
    let on_perm_next_id = next_request_id.clone();
    let on_permission = move |request: PermissionPromptRequest| -> PermissionPromptAction {
        let request_id = on_perm_next_id.fetch_add(1, Ordering::SeqCst).to_string();
        let (tx, rx) = mpsc::channel::<PermissionPromptAction>();
        on_perm_pending
            .lock()
            .unwrap()
            .insert(request_id.clone(), tx);

        let _ = on_perm_app.emit(
            &on_perm_channel,
            EmittedEvent::PermissionRequest {
                turn_id: on_perm_turn_id.clone(),
                request_id,
                tool_id: request.tool_id,
                summary: request.summary,
                reason: request.reason,
            },
        );

        // Block until the JS side resolves the prompt (or cancel denies it).
        rx.recv().unwrap_or(PermissionPromptAction::Deny)
    };

    let on_question_app = app.clone();
    let on_question_channel = event_channel.clone();
    let on_question_turn_id = turn_id.clone();
    let on_question_pending = pending_questions.clone();
    let on_question_next_id = next_request_id.clone();
    let on_user_question = move |request: UserQuestionPromptRequest| -> UserQuestionPromptResponse {
        let request_id = on_question_next_id
            .fetch_add(1, Ordering::SeqCst)
            .to_string();
        let (tx, rx) = mpsc::channel::<UserQuestionPromptResponse>();
        on_question_pending
            .lock()
            .unwrap()
            .insert(request_id.clone(), tx);

        let _ = on_question_app.emit(
            &on_question_channel,
            EmittedEvent::UserQuestionRequest {
                turn_id: on_question_turn_id.clone(),
                request_id,
                questions: request.questions,
            },
        );

        rx.recv().unwrap_or(UserQuestionPromptResponse {
            answers: serde_json::Map::new(),
            annotations: serde_json::Map::new(),
        })
    };

    let outcome = with_user_question_prompt_handler(on_user_question, || {
        execute_user_turn_streaming_with_permissions(
            &mut state,
            &resources,
            &providers,
            &mut auth_store,
            &message,
            None,
            on_event,
            on_permission,
        )
    })
    .with_context(|| format!("run_agent_turn: session={session_id}"))?;

    // Persist tool invocations before the assistant text so reloads read in
    // the same order as the work happened.
    for invocation in &outcome.tool_invocations {
        session_store.append_event(
            session_uuid,
            TranscriptEvent::ToolInvocation {
                call_id: invocation.call_id.clone(),
                tool_id: invocation.tool_id.clone(),
                input: invocation.input.clone(),
                output: invocation.output.clone(),
                success: invocation.success,
            },
        )?;
    }
    if !outcome.assistant_text.is_empty() {
        state.push_message(MessageRole::Assistant, outcome.assistant_text.clone());
        session_store.append_event(
            session_uuid,
            TranscriptEvent::AssistantMessage {
                text: outcome.assistant_text.clone(),
            },
        )?;
    }
    for trace in &outcome.reflection_traces {
        session_store.append_trace_event(
            session_uuid,
            puffer_session_store::TRACE_RUNTIME,
            trace,
        )?;
    }

    Ok(outcome.assistant_text)
}

/// Loads everything the runtime needs, mirroring the setup path in
/// `puffer-cli/src/main.rs`.
fn load_context(
    session_id: &str,
) -> Result<(
    puffer_config::PufferConfig,
    puffer_resources::LoadedResources,
    ProviderRegistry,
    AuthStore,
    SessionStore,
    AppState,
)> {
    let workspace_root = std::env::current_dir()?;
    let paths = ConfigPaths::discover(&workspace_root);
    ensure_workspace_dirs(&paths)?;
    let config = load_config(&paths)?;
    let auth_path = paths.user_config_dir.join("auth.json");
    let auth_store = AuthStore::load(&auth_path)?;
    let resources = load_resources(&paths)?;

    let mut providers = ProviderRegistry::new();
    for provider in &resources.providers {
        providers.register_with_source(
            provider.value.clone().into_descriptor(),
            provider.source_info.as_provider_source(),
        );
    }
    providers.apply_openai_base_url_override(config.openai_base_url.as_deref());
    if !config.openai_headers.is_empty() {
        providers.set_openai_headers(
            config
                .openai_headers
                .clone()
                .into_iter()
                .collect::<IndexMap<_, _>>(),
        );
    }
    if !config.openai_query_params.is_empty() {
        providers.set_openai_query_params(
            config
                .openai_query_params
                .clone()
                .into_iter()
                .collect::<IndexMap<_, _>>(),
        );
    }
    let _ = providers.discover_and_merge_all(&auth_store);

    let session_store = SessionStore::from_paths(&paths)?;
    let session_uuid = Uuid::parse_str(session_id).context("invalid session id")?;
    let record = session_store.load_session(session_uuid)?;
    let state = AppState::from_session_record(config.clone(), record);
    Ok((
        config,
        resources,
        providers,
        auth_store,
        session_store,
        state,
    ))
}

// Anchor the `json!` import so clippy doesn't trim it while we iterate.
#[allow(dead_code)]
fn _unused_anchor() -> serde_json::Value {
    json!({})
}
