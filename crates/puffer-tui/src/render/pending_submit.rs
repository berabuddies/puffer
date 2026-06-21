use puffer_core::ToolCallRequest;
use std::cell::RefCell;
use std::time::Instant;

/// Snapshot of one in-flight turn used by the renderer.
#[derive(Default)]
pub(crate) struct PendingSubmitRenderState {
    pub(crate) loading_prompt: Option<String>,
    pub(crate) pending_tool_calls: Vec<ToolCallRequest>,
    pub(crate) queued_prompts: Vec<String>,
    pub(crate) started_at: Option<Instant>,
    /// True when the model is actively producing thinking/reasoning tokens.
    pub(crate) thinking_active: bool,
    /// Transient status hint, such as a retry or command progress message.
    pub(crate) status_hint: Option<String>,
}

impl PendingSubmitRenderState {
    /// Returns true when a provider-backed turn is still active.
    pub(crate) fn is_active(&self) -> bool {
        self.started_at.is_some()
            || self.loading_prompt.is_some()
            || self.status_hint.is_some()
            || !self.pending_tool_calls.is_empty()
    }
}

thread_local! {
    static ACTIVE_PENDING_SUBMIT: RefCell<PendingSubmitRenderState> =
        RefCell::new(PendingSubmitRenderState::default());
}

/// Sets the pending submit render state for the current frame.
pub(crate) fn set_pending_submit_state(
    loading_prompt: Option<String>,
    pending_tool_calls: Vec<ToolCallRequest>,
    queued_prompts: Vec<String>,
    started_at: Option<Instant>,
    thinking_active: bool,
    status_hint: Option<String>,
) {
    ACTIVE_PENDING_SUBMIT.with(|value| {
        *value.borrow_mut() = PendingSubmitRenderState {
            loading_prompt,
            pending_tool_calls,
            queued_prompts,
            started_at,
            thinking_active,
            status_hint,
        };
    });
}

/// Returns the pending submit render snapshot for the current frame.
pub(crate) fn pending_submit_state() -> PendingSubmitRenderState {
    ACTIVE_PENDING_SUBMIT.with(|value| PendingSubmitRenderState {
        loading_prompt: value.borrow().loading_prompt.clone(),
        pending_tool_calls: value.borrow().pending_tool_calls.clone(),
        queued_prompts: value.borrow().queued_prompts.clone(),
        started_at: value.borrow().started_at,
        thinking_active: value.borrow().thinking_active,
        status_hint: value.borrow().status_hint.clone(),
    })
}
