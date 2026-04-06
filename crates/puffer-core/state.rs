use puffer_config::PufferConfig;
use puffer_session_store::{
    RuntimePlanState, RuntimeTask, RuntimeTaskStatus, SessionMetadata, SessionRecord,
    TranscriptEvent,
};
use serde::Serialize;
use std::path::PathBuf;

/// Describes the role of a rendered transcript message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

/// Represents one rendered transcript message in the interactive UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedMessage {
    pub role: MessageRole,
    pub text: String,
}

/// Describes the completion state of one recorded task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TaskStatus {
    Completed,
    Failed,
}

/// Represents one recorded shell or tool task in the current session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskRecord {
    pub id: u64,
    pub label: String,
    pub detail: String,
    pub status: TaskStatus,
    pub plan_summary: Option<String>,
    pub agent_id: Option<String>,
    pub worktree: String,
}

/// Captures the current per-session plan mode and summary text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanState {
    pub mode_open: bool,
    pub summary: Option<String>,
    pub updated_at_ms: u64,
}

/// Stores the mutable session and UI state for one interactive Puffer run.
#[derive(Debug, Clone)]
pub struct AppState {
    pub config: PufferConfig,
    pub cwd: PathBuf,
    pub working_dirs: Vec<PathBuf>,
    pub session: SessionMetadata,
    pub transcript: Vec<RenderedMessage>,
    pub current_model: Option<String>,
    pub current_provider: Option<String>,
    pub prompt_color: String,
    pub effort_level: String,
    pub fast_mode: bool,
    pub sandbox_mode: String,
    pub remote_name: Option<String>,
    pub remote_environment: Option<String>,
    pub statusline_enabled: bool,
    pub vim_mode: bool,
    pub should_exit: bool,
    plan: PlanState,
    active_agent_id: Option<String>,
    active_worktree: Option<PathBuf>,
    tasks: Vec<TaskRecord>,
    next_task_id: u64,
}

impl AppState {
    /// Creates a new application state for the active session.
    pub fn new(config: PufferConfig, cwd: PathBuf, session: SessionMetadata) -> Self {
        let session_cwd = session.cwd.clone();
        Self {
            current_model: config.default_model.clone(),
            current_provider: config.default_provider.clone(),
            config,
            cwd,
            working_dirs: Vec::new(),
            session,
            transcript: Vec::new(),
            prompt_color: "default".to_string(),
            effort_level: "medium".to_string(),
            fast_mode: false,
            sandbox_mode: "workspace-write".to_string(),
            remote_name: None,
            remote_environment: None,
            statusline_enabled: true,
            vim_mode: false,
            should_exit: false,
            plan: PlanState {
                mode_open: false,
                summary: None,
                updated_at_ms: 0,
            },
            active_agent_id: None,
            active_worktree: Some(session_cwd),
            tasks: Vec::new(),
            next_task_id: 1,
        }
    }

    /// Restores application state from a persisted session record.
    pub fn from_session_record(config: PufferConfig, session: SessionRecord) -> Self {
        let cwd = session.metadata.cwd.clone();
        let mut state = Self::new(config, cwd, session.metadata);
        for event in session.events {
            match event {
                TranscriptEvent::UserMessage { text } => {
                    state.push_message(MessageRole::User, text)
                }
                TranscriptEvent::AssistantMessage { text } => {
                    state.push_message(MessageRole::Assistant, text)
                }
                TranscriptEvent::SystemMessage { text } => {
                    state.push_message(MessageRole::System, text)
                }
                TranscriptEvent::CommandInvoked { name, args } => state.push_message(
                    MessageRole::System,
                    format!("Command: /{} {}", name, args).trim().to_string(),
                ),
                TranscriptEvent::SessionRenamed { name } => {
                    state.session.display_name = Some(name);
                }
                TranscriptEvent::StateSnapshot {
                    current_model,
                    current_provider,
                    theme,
                    prompt_color,
                    effort_level,
                    fast_mode,
                    sandbox_mode,
                    remote_name,
                    remote_environment,
                    statusline_enabled,
                    working_dirs,
                } => {
                    state.current_model = current_model;
                    state.current_provider = current_provider;
                    state.config.theme = theme;
                    state.prompt_color = prompt_color;
                    state.effort_level = effort_level;
                    state.fast_mode = fast_mode;
                    state.sandbox_mode = sandbox_mode;
                    state.remote_name = remote_name;
                    state.remote_environment = remote_environment;
                    state.statusline_enabled = statusline_enabled;
                    state.working_dirs = working_dirs.into_iter().map(Into::into).collect();
                }
                TranscriptEvent::RuntimeState {
                    plan,
                    tasks,
                    next_task_id,
                    active_agent_id,
                    active_worktree,
                } => {
                    state.plan = PlanState {
                        mode_open: plan.mode_open,
                        summary: plan.summary,
                        updated_at_ms: plan.updated_at_ms,
                    };
                    state.tasks = tasks
                        .into_iter()
                        .map(|task| TaskRecord {
                            id: task.id,
                            label: task.label,
                            detail: task.detail,
                            status: match task.status {
                                RuntimeTaskStatus::Completed => TaskStatus::Completed,
                                RuntimeTaskStatus::Failed => TaskStatus::Failed,
                            },
                            plan_summary: task.plan_summary,
                            agent_id: task.agent_id,
                            worktree: task.worktree,
                        })
                        .collect();
                    let inferred_next = state
                        .tasks
                        .iter()
                        .map(|task| task.id)
                        .max()
                        .unwrap_or(0)
                        .saturating_add(1);
                    state.next_task_id = next_task_id.max(inferred_next).max(1);
                    state.active_agent_id = active_agent_id;
                    state.active_worktree = active_worktree.map(Into::into);
                }
            }
        }
        state
    }

    /// Appends a rendered message to the in-memory transcript.
    pub fn push_message(&mut self, role: MessageRole, text: impl Into<String>) {
        self.transcript.push(RenderedMessage {
            role,
            text: text.into(),
        });
    }

    /// Records one completed or failed task in the current runtime session state.
    pub fn record_task(
        &mut self,
        label: impl Into<String>,
        detail: impl Into<String>,
        success: bool,
    ) {
        let worktree = self
            .active_worktree
            .as_deref()
            .unwrap_or(&self.cwd)
            .display()
            .to_string();
        let plan_summary = self.plan.summary.clone();
        let agent_id = self.active_agent_id.clone();
        let detail = format!(
            "{}\ncontext: plan={} agent={} worktree={}",
            detail.into().trim(),
            plan_summary.as_deref().unwrap_or("<unset>"),
            agent_id.as_deref().unwrap_or("<unset>"),
            worktree
        )
        .trim()
        .to_string();
        let task = TaskRecord {
            id: self.next_task_id,
            label: label.into(),
            detail,
            status: if success {
                TaskStatus::Completed
            } else {
                TaskStatus::Failed
            },
            plan_summary,
            agent_id,
            worktree,
        };
        self.next_task_id += 1;
        self.tasks.push(task);
    }

    /// Builds a persisted snapshot event for the current mutable session state.
    pub fn snapshot_event(&self) -> TranscriptEvent {
        TranscriptEvent::StateSnapshot {
            current_model: self.current_model.clone(),
            current_provider: self.current_provider.clone(),
            theme: self.config.theme.clone(),
            prompt_color: self.prompt_color.clone(),
            effort_level: self.effort_level.clone(),
            fast_mode: self.fast_mode,
            sandbox_mode: self.sandbox_mode.clone(),
            remote_name: self.remote_name.clone(),
            remote_environment: self.remote_environment.clone(),
            statusline_enabled: self.statusline_enabled,
            working_dirs: self
                .working_dirs
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
        }
    }

    /// Builds a persisted runtime event for task, plan, agent, and worktree state.
    pub fn runtime_event(&self) -> TranscriptEvent {
        TranscriptEvent::RuntimeState {
            plan: RuntimePlanState {
                mode_open: self.plan.mode_open,
                summary: self.plan.summary.clone(),
                updated_at_ms: self.plan.updated_at_ms,
            },
            tasks: self
                .tasks
                .iter()
                .map(|task| RuntimeTask {
                    id: task.id,
                    label: task.label.clone(),
                    detail: task.detail.clone(),
                    status: match task.status {
                        TaskStatus::Completed => RuntimeTaskStatus::Completed,
                        TaskStatus::Failed => RuntimeTaskStatus::Failed,
                    },
                    plan_summary: task.plan_summary.clone(),
                    agent_id: task.agent_id.clone(),
                    worktree: task.worktree.clone(),
                })
                .collect(),
            next_task_id: self.next_task_id,
            active_agent_id: self.active_agent_id.clone(),
            active_worktree: self
                .active_worktree
                .as_ref()
                .map(|path| path.display().to_string()),
        }
    }

    pub(crate) fn clear_active_agent(&mut self) {
        self.active_agent_id = None;
    }

    pub(crate) fn open_plan(&mut self) {
        self.plan.mode_open = true;
        self.plan.updated_at_ms = now_ms();
    }

    pub(crate) fn close_plan(&mut self) {
        self.plan.mode_open = false;
        self.plan.updated_at_ms = now_ms();
    }

    pub(crate) fn set_active_agent(&mut self, agent_id: Option<String>) {
        self.active_agent_id = agent_id;
    }

    pub(crate) fn set_active_worktree(&mut self, worktree: PathBuf) {
        self.active_worktree = Some(worktree);
    }

    pub(crate) fn set_plan_summary(&mut self, summary: Option<String>) {
        self.plan.summary = summary;
        self.plan.updated_at_ms = now_ms();
    }

    pub(crate) fn render_plan_summary(&self) -> String {
        format!(
            "Plan:\nmode={}\nsummary={}\nupdated_at_ms={}",
            if self.plan.mode_open {
                "open"
            } else {
                "closed"
            },
            self.plan.summary.as_deref().unwrap_or("<unset>"),
            self.plan.updated_at_ms
        )
    }

    pub(crate) fn tasks(&self) -> &[TaskRecord] {
        &self.tasks
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_event_roundtrip_restores_runtime_state() {
        let metadata = SessionMetadata {
            id: uuid::Uuid::nil(),
            display_name: None,
            cwd: PathBuf::from("/tmp/puffer"),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        let mut state = AppState::new(
            PufferConfig::default(),
            PathBuf::from("/tmp/puffer"),
            metadata.clone(),
        );
        state.open_plan();
        state.set_plan_summary(Some("ship runtime parity".to_string()));
        state.set_active_agent(Some("reviewer".to_string()));
        state.set_active_worktree(PathBuf::from("/tmp/puffer/.worktree/tool-review"));
        state.record_task("bash", "cargo test -p puffer-core", true);
        let record = SessionRecord {
            metadata,
            events: vec![state.runtime_event()],
        };

        let restored = AppState::from_session_record(PufferConfig::default(), record);
        assert_eq!(restored.tasks().len(), 1);
        assert!(restored.tasks()[0].detail.contains("agent=reviewer"));
        assert!(restored.tasks()[0]
            .detail
            .contains("plan=ship runtime parity"));
        assert_eq!(restored.render_plan_summary().contains("mode=open"), true);
        assert!(restored
            .render_plan_summary()
            .contains("ship runtime parity"));
    }
}
