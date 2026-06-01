use crate::AppState;
use anyhow::{anyhow, Result};
use glob::Pattern;
use puffer_config::{
    ensure_project_memory, resolve_project_memory, ConfigPaths, ResolvedProjectMemory,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use serde::Deserialize;
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const ENTRY_DELIMITER: &str = "\n§\n";
const PROJECT_MEMORY_TOOL_ID: &str = "Memory";
const PROJECT_MEMORY_READ_TOOL_ID: &str = "Read";
const PROJECT_MEMORY_SKILL_TOOL_ID: &str = "Skill";
const PROJECT_MEMORY_SKILL_NAME: &str = "project-memory";
const LOCK_RETRY_MS: u64 = 25;
const LOCK_RETRY_ATTEMPTS: usize = 80;
const MEMORY_REVIEW_PROMPT: &str = "Review the conversation above and decide whether the active project's MEMORY.md should be updated. Save only durable project knowledge: stable structure, confirmed workflow rules, environment quirks relevant to this project, and recurring user corrections that matter for future work in this project. Do not save temporary task progress, one-off debugging state, speculative ideas, or transient file paths. If something is worth saving, use the Memory tool. If nothing is worth saving, stop.";
const MEMORY_FLUSH_PROMPT: &str = "The conversation is about to be compacted. Save any durable project facts worth preserving into the active project's MEMORY.md. Prioritize stable project conventions, recurring workflow rules, and user corrections that apply to this project. Do not save temporary task progress. If nothing is worth saving, stop.";

struct ProjectMemorySideTurn {
    state: AppState,
    resources: LoadedResources,
    filter: Option<crate::runtime::RequestToolFilter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMemoryContext {
    pub project_name: String,
    pub project_root: PathBuf,
    pub memory_file: PathBuf,
    pub char_limit: usize,
}

impl ProjectMemoryContext {
    pub fn load(cwd: &Path, char_limit: usize) -> Result<Option<Self>> {
        let paths = ConfigPaths::discover(cwd);
        let Some(resolved) = resolve_project_memory(&paths, cwd)? else {
            return Ok(None);
        };
        Ok(Some(Self::from_resolved(resolved, char_limit)?))
    }

    pub(crate) fn from_parts(
        project_name: String,
        project_root: PathBuf,
        memory_file: PathBuf,
        char_limit: usize,
    ) -> Result<Self> {
        Ok(Self {
            project_name,
            project_root,
            memory_file,
            char_limit,
        })
    }

    fn from_resolved(resolved: ResolvedProjectMemory, char_limit: usize) -> Result<Self> {
        Self::from_parts(
            resolved.name,
            resolved.root,
            resolved.memory_file,
            char_limit,
        )
    }
}

/// Ensures the current cwd has explicit project memory and refreshes app state.
pub fn activate_project_memory(state: &mut AppState) -> Result<Option<ProjectMemoryContext>> {
    if !state.memory_enabled() {
        state.project_memory = None;
        return Ok(None);
    }
    let paths = ConfigPaths::discover(&state.cwd);
    let resolved = ensure_project_memory(&paths, &state.cwd)?;
    let context = ProjectMemoryContext::from_resolved(resolved, state.config.memory.char_limit)?;
    state.project_memory = Some(context.clone());
    state.refresh_project_memory();
    Ok(state.project_memory.clone())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMemoryMutation {
    pub success: bool,
    pub message: String,
    pub entries: Vec<String>,
    pub usage: String,
    pub entry_count: usize,
}

#[derive(Debug, Deserialize)]
struct MemoryToolInput {
    action: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    old_text: Option<String>,
}

pub fn execute_memory_tool(state: &mut AppState, input: serde_json::Value) -> Result<String> {
    let input: MemoryToolInput = serde_json::from_value(input)?;
    let Some(context) = state.project_memory.as_ref() else {
        return Ok(json!({
            "success": false,
            "error": "No configured project matches the current working directory, so project memory is unavailable.",
        })
        .to_string());
    };

    let mutation = match input.action.as_str() {
        "add" => add_entry(
            &context.memory_file,
            context.char_limit,
            input.content.as_deref(),
        ),
        "replace" => replace_entry(
            &context.memory_file,
            context.char_limit,
            input.old_text.as_deref(),
            input.content.as_deref(),
        ),
        "remove" => remove_entry(
            &context.memory_file,
            context.char_limit,
            input.old_text.as_deref(),
        ),
        other => Err(anyhow!(
            "Unknown action `{other}`. Use add, replace, or remove."
        )),
    }?;

    if mutation.success {
        state.project_memory_review_turns = 0;
    }

    Ok(json!({
        "success": mutation.success,
        "message": mutation.message,
        "entries": mutation.entries,
        "usage": mutation.usage,
        "entry_count": mutation.entry_count,
        "project": context.project_name,
        "path": context.memory_file,
    })
    .to_string())
}

pub fn flush_project_memory(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
) -> Result<()> {
    if !state.memory_enabled() || !state.memory_flush_enabled() || state.project_memory.is_none() {
        return Ok(());
    }
    let Some(mut side_turn) = prepare_project_memory_side_turn(state, resources)? else {
        return Ok(());
    };
    let _ = crate::runtime::execute_user_prompt_with_tool_filter(
        &mut side_turn.state,
        &side_turn.resources,
        providers,
        auth_store,
        MEMORY_FLUSH_PROMPT,
        side_turn.filter.as_ref(),
    )?;
    Ok(())
}

pub fn spawn_project_memory_review(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) {
    if !state.memory_enabled() || !state.memory_review_enabled() || state.project_memory.is_none() {
        return;
    }
    let Ok(Some(mut side_turn)) = prepare_project_memory_side_turn(state, resources) else {
        return;
    };
    let providers = providers.clone();
    let mut auth_store = auth_store.clone();
    thread::spawn(move || {
        let _ = crate::runtime::execute_user_prompt_with_tool_filter(
            &mut side_turn.state,
            &side_turn.resources,
            &providers,
            &mut auth_store,
            MEMORY_REVIEW_PROMPT,
            side_turn.filter.as_ref(),
        );
    });
}

pub fn project_memory_turn_completed(state: &mut AppState) -> bool {
    if !state.memory_enabled() || !state.memory_review_enabled() || state.project_memory.is_none() {
        return false;
    }
    state.project_memory_review_turns += 1;
    if state.project_memory_review_turns >= state.memory_review_nudge_interval() {
        state.project_memory_review_turns = 0;
        true
    } else {
        false
    }
}

pub fn project_memory_path(state: &AppState) -> Option<PathBuf> {
    state
        .project_memory
        .as_ref()
        .map(|context| context.memory_file.clone())
}

pub fn project_memory_status(state: &AppState) -> Option<String> {
    let context = state.project_memory.as_ref()?;
    let entries = read_entries(&context.memory_file).ok()?;
    let usage = usage_string(&entries, context.char_limit);
    Some(format!(
        "project={}\nroot={}\nmemory_file={}\nusage={}\nentries={}",
        context.project_name,
        context.project_root.display(),
        context.memory_file.display(),
        usage,
        entries.len(),
    ))
}

/// Returns a runtime reminder that tells the model how to load project memory
/// through the builtin Skill surface instead of relying on system-prompt
/// injection.
pub(crate) fn project_memory_skill_reminder(state: &AppState) -> Option<String> {
    let context = state.project_memory.as_ref()?;
    Some(format!(
        "# projectMemory\nConfigured project memory is available for project `{}` at `{}`.\nIf you need durable project-specific context, call the Skill tool with `skill: \"{}\"` and `args: \"{}\"` before proceeding.\nAfter loading the skill, follow its instructions and use the Memory tool for durable updates instead of editing MEMORY.md directly.",
        context.project_name,
        context.memory_file.display(),
        PROJECT_MEMORY_SKILL_NAME,
        context.memory_file.display(),
    ))
}

fn prepare_project_memory_side_turn(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Option<ProjectMemorySideTurn>> {
    let Some(context) = state.project_memory.as_ref() else {
        return Ok(None);
    };
    let memory_parent = context
        .memory_file
        .parent()
        .ok_or_else(|| anyhow!("project memory path has no parent directory"))?
        .to_path_buf();
    let read_scope = Pattern::escape(&context.memory_file.to_string_lossy());
    let mut side_state = state.clone();
    if !side_state
        .working_dirs
        .iter()
        .any(|path| path == &memory_parent)
    {
        side_state.working_dirs.push(memory_parent);
    }
    let mut side_resources = resources.clone();
    side_resources.tools.retain(|tool| {
        matches!(
            tool.value.id.as_str(),
            PROJECT_MEMORY_TOOL_ID | PROJECT_MEMORY_READ_TOOL_ID | PROJECT_MEMORY_SKILL_TOOL_ID
        )
    });
    side_resources
        .skills
        .retain(|skill| skill.value.name == PROJECT_MEMORY_SKILL_NAME);
    let filter = crate::runtime::build_request_tool_filter(&[
        PROJECT_MEMORY_SKILL_TOOL_ID.to_string(),
        format!("{PROJECT_MEMORY_READ_TOOL_ID}({read_scope})"),
        PROJECT_MEMORY_TOOL_ID.to_string(),
    ])?;
    Ok(Some(ProjectMemorySideTurn {
        state: side_state,
        resources: side_resources,
        filter,
    }))
}

fn add_entry(
    path: &Path,
    char_limit: usize,
    content: Option<&str>,
) -> Result<ProjectMemoryMutation> {
    let content = content.unwrap_or("").trim();
    if content.is_empty() {
        return Err(anyhow!("content is required for add"));
    }
    validate_content(content)?;
    with_locked_entries(path, |entries| {
        if entries.iter().any(|entry| entry == content) {
            return Ok(success_response(
                entries,
                char_limit,
                "Entry already exists.",
            ));
        }
        let next = projected_usage(entries, Some(content), None, char_limit)?;
        entries.push(content.to_string());
        Ok(success_response_with_usage(
            entries,
            next,
            char_limit,
            "Entry added.",
        ))
    })
}

fn replace_entry(
    path: &Path,
    char_limit: usize,
    old_text: Option<&str>,
    new_content: Option<&str>,
) -> Result<ProjectMemoryMutation> {
    let old_text = old_text.unwrap_or("").trim();
    let new_content = new_content.unwrap_or("").trim();
    if old_text.is_empty() {
        return Err(anyhow!("old_text is required for replace"));
    }
    if new_content.is_empty() {
        return Err(anyhow!("content is required for replace"));
    }
    validate_content(new_content)?;
    with_locked_entries(path, |entries| {
        let index = unique_match(entries, old_text)?;
        let next = projected_usage(entries, Some(new_content), Some(index), char_limit)?;
        entries[index] = new_content.to_string();
        Ok(success_response_with_usage(
            entries,
            next,
            char_limit,
            "Entry replaced.",
        ))
    })
}

fn remove_entry(
    path: &Path,
    char_limit: usize,
    old_text: Option<&str>,
) -> Result<ProjectMemoryMutation> {
    let old_text = old_text.unwrap_or("").trim();
    if old_text.is_empty() {
        return Err(anyhow!("old_text is required for remove"));
    }
    with_locked_entries(path, |entries| {
        let index = unique_match(entries, old_text)?;
        entries.remove(index);
        Ok(success_response(entries, char_limit, "Entry removed."))
    })
}

fn with_locked_entries<T>(
    path: &Path,
    mutator: impl FnOnce(&mut Vec<String>) -> Result<T>,
) -> Result<T> {
    let _guard = FileLockGuard::acquire(path)?;
    let mut entries = read_entries(path)?;
    let result = mutator(&mut entries)?;
    write_entries(path, &entries)?;
    Ok(result)
}

fn read_entries(path: &Path) -> Result<Vec<String>> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in raw.split(ENTRY_DELIMITER) {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !entries.iter().any(|existing| existing == trimmed) {
            entries.push(trimmed.to_string());
        }
    }
    Ok(entries)
}

fn write_entries(path: &Path, entries: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_extension("tmp");
    fs::write(&temp_path, entries.join(ENTRY_DELIMITER))?;
    fs::rename(&temp_path, path)?;
    Ok(())
}

fn success_response(entries: &[String], char_limit: usize, message: &str) -> ProjectMemoryMutation {
    success_response_with_usage(
        entries,
        usage_parts(entries, char_limit),
        char_limit,
        message,
    )
}

fn success_response_with_usage(
    entries: &[String],
    usage: (usize, usize),
    char_limit: usize,
    message: &str,
) -> ProjectMemoryMutation {
    ProjectMemoryMutation {
        success: true,
        message: message.to_string(),
        entries: entries.to_vec(),
        usage: format!("{}% — {}/{} chars", usage.0, usage.1, char_limit),
        entry_count: entries.len(),
    }
}

fn usage_string(entries: &[String], char_limit: usize) -> String {
    let usage = usage_parts(entries, char_limit);
    format!("{}% — {}/{} chars", usage.0, usage.1, char_limit)
}

fn usage_parts(entries: &[String], char_limit: usize) -> (usize, usize) {
    let chars = entries.join(ENTRY_DELIMITER).chars().count();
    let pct = if char_limit == 0 {
        0
    } else {
        (chars.saturating_mul(100) / char_limit).min(100)
    };
    (pct, chars)
}

fn projected_usage(
    entries: &[String],
    replacement: Option<&str>,
    replace_index: Option<usize>,
    char_limit: usize,
) -> Result<(usize, usize)> {
    let mut test_entries = entries.to_vec();
    match replace_index {
        Some(index) => test_entries[index] = replacement.unwrap_or_default().to_string(),
        None => test_entries.push(replacement.unwrap_or_default().to_string()),
    }
    let usage = usage_parts(&test_entries, char_limit);
    if usage.1 > char_limit {
        return Err(anyhow!(
            "Project memory at {}/{} chars. Replace or remove existing entries first.",
            usage.1,
            char_limit
        ));
    }
    Ok(usage)
}

fn unique_match(entries: &[String], old_text: &str) -> Result<usize> {
    let matches = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.contains(old_text).then_some(index))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(anyhow!("No entry matched `{old_text}`.")),
        [index] => Ok(*index),
        _ => Err(anyhow!(
            "Multiple entries matched `{old_text}`. Be more specific."
        )),
    }
}

fn validate_content(content: &str) -> Result<()> {
    let lowered = content.to_ascii_lowercase();
    for needle in [
        "ignore previous instructions",
        "ignore all instructions",
        "system prompt override",
        "authorized_keys",
        "~/.ssh",
        "$api_key",
        "$token",
    ] {
        if lowered.contains(needle) {
            return Err(anyhow!(
                "Blocked: content contains unsafe prompt-injection or exfiltration text."
            ));
        }
    }
    if content.chars().any(|ch| {
        matches!(
            ch,
            '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{feff}'
        )
    }) {
        return Err(anyhow!(
            "Blocked: content contains invisible unicode characters."
        ));
    }
    Ok(())
}

struct FileLockGuard {
    path: PathBuf,
}

impl FileLockGuard {
    fn acquire(target: &Path) -> Result<Self> {
        let path = target.with_extension("lock");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        for _ in 0..LOCK_RETRY_ATTEMPTS {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self { path }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    thread::sleep(Duration::from_millis(LOCK_RETRY_MS));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(anyhow!(
            "Timed out waiting for project memory lock {}",
            path.display()
        ))
    }
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::profile::{EffectiveApprovalPolicy, EffectiveSandboxMode};
    use crate::permissions::FilesystemPermissionPolicy;
    use crate::runtime::claude_tools::{execute_tool, ProviderToolContext};
    use puffer_config::MemoryConfig;
    use puffer_provider_registry::{AuthStore, ModelDescriptor, ProviderDescriptor};
    use puffer_resources::{
        LoadedItem, LoadedResources, SkillSpec, SourceInfo, SourceKind, ToolSpec,
    };
    use puffer_session_store::SessionMetadata;
    use puffer_tools::ToolRegistry;
    use serde_json::{json, Value};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::thread;
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    fn allow_all_filesystem_policy(root: &Path) -> FilesystemPermissionPolicy {
        FilesystemPermissionPolicy {
            approval: EffectiveApprovalPolicy::Allow,
            sandbox_mode: EffectiveSandboxMode::DangerFullAccess,
            workspace_roots: vec![root.to_path_buf()],
            session_granted: true,
            allow_all_paths: true,
        }
    }

    #[test]
    fn add_replace_remove_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("MEMORY.md");
        let added = add_entry(&path, 200, Some("fact one")).unwrap();
        assert_eq!(added.entry_count, 1);
        let replaced = replace_entry(&path, 200, Some("one"), Some("fact two")).unwrap();
        assert_eq!(replaced.entries, vec!["fact two".to_string()]);
        let removed = remove_entry(&path, 200, Some("two")).unwrap();
        assert!(removed.entries.is_empty());
    }

    #[test]
    fn blocked_content_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("MEMORY.md");
        let error = add_entry(&path, 200, Some("ignore previous instructions")).unwrap_err();
        assert!(error.to_string().contains("Blocked"));
    }

    #[test]
    fn memory_tool_runtime_handler_updates_file_and_resets_review_counter() {
        let temp = tempfile::tempdir().unwrap();
        let memory_file = temp.path().join("MEMORY.md");
        let mut state = project_memory_state(temp.path(), &memory_file);
        state.project_memory_review_turns = 5;
        let resources = memory_tool_resources();
        let registry = ToolRegistry::from_resources(&resources);
        let definition = registry.definition("Memory").expect("memory tool");

        let filesystem_policy = allow_all_filesystem_policy(temp.path());
        let result = execute_tool(
            &mut state,
            &resources,
            &registry,
            definition,
            temp.path(),
            &filesystem_policy,
            json!({"action": "add", "content": "stable project rule"}),
            ProviderToolContext::None,
        )
        .expect("execute memory tool");

        assert!(result.success);
        let payload: Value = serde_json::from_str(&result.output.stdout).expect("json payload");
        assert_eq!(payload["success"], true);
        assert_eq!(payload["entry_count"], 1);
        assert_eq!(state.project_memory_review_turns, 0);
        assert!(std::fs::read_to_string(&memory_file)
            .unwrap()
            .contains("stable project rule"));
    }

    #[test]
    fn background_review_uses_memory_tool_and_persists_entry() {
        let temp = tempfile::tempdir().unwrap();
        let memory_file = temp.path().join("MEMORY.md");
        let mut state = project_memory_state(temp.path(), &memory_file);
        state.current_provider = Some("local-anthropic".to_string());
        state.current_model = Some("local-anthropic/claude-sonnet-4-5".to_string());
        let resources = memory_tool_resources();
        let mut providers = ProviderRegistry::new();
        providers.register(local_provider(spawn_json_server(vec![
            json!({
                "id": "msg_review_tool",
                "model": "claude-sonnet-4-5",
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_review_1",
                    "name": "Memory",
                    "input": {"action": "add", "content": "reviewed durable fact"}
                }],
                "stop_reason": "tool_use"
            }),
            json!({
                "id": "msg_review_done",
                "model": "claude-sonnet-4-5",
                "role": "assistant",
                "content": [{"type": "text", "text": "review complete"}],
                "stop_reason": "end_turn"
            }),
        ])));
        let auth_store = AuthStore::default();

        spawn_project_memory_review(&state, &resources, &providers, &auth_store);
        wait_for_memory_entry(&memory_file, "reviewed durable fact");
        let contents = std::fs::read_to_string(&memory_file).unwrap();
        assert!(contents.contains("reviewed durable fact"));
    }

    #[test]
    fn project_memory_side_turn_uses_skill_and_restricts_read_to_memory_file() {
        let temp = tempfile::tempdir().unwrap();
        let memory_dir = temp.path().join("project-memory");
        std::fs::create_dir_all(&memory_dir).unwrap();
        let memory_file = memory_dir.join("MEMORY.md");
        let sibling_file = memory_dir.join("secret.txt");
        std::fs::write(&memory_file, "original durable fact").unwrap();
        std::fs::write(&sibling_file, "do not read this").unwrap();
        let mut state = project_memory_state(temp.path(), &memory_file);
        state.current_provider = Some("local-anthropic".to_string());
        state.current_model = Some("local-anthropic/claude-sonnet-4-5".to_string());
        let resources = memory_tool_resources();
        let mut side_turn = prepare_project_memory_side_turn(&state, &resources)
            .unwrap()
            .expect("project memory side turn");
        assert_eq!(side_turn.resources.skills.len(), 1);
        assert_eq!(side_turn.resources.skills[0].value.name, "project-memory");
        let mut providers = ProviderRegistry::new();
        providers.register(local_provider(spawn_json_server(vec![
            json!({
                "id": "msg_skill",
                "model": "claude-sonnet-4-5",
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_skill_1",
                    "name": "Skill",
                    "input": {"skill": "project-memory", "args": memory_file.display().to_string()}
                }],
                "stop_reason": "tool_use"
            }),
            json!({
                "id": "msg_read_denied",
                "model": "claude-sonnet-4-5",
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_read_1",
                    "name": "Read",
                    "input": {"file_path": sibling_file.display().to_string()}
                }],
                "stop_reason": "tool_use"
            }),
            json!({
                "id": "msg_read_allowed",
                "model": "claude-sonnet-4-5",
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_read_2",
                    "name": "Read",
                    "input": {"file_path": memory_file.display().to_string()}
                }],
                "stop_reason": "tool_use"
            }),
            json!({
                "id": "msg_memory_replace",
                "model": "claude-sonnet-4-5",
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_memory_1",
                    "name": "Memory",
                    "input": {
                        "action": "replace",
                        "old_text": "original durable fact",
                        "content": "updated durable fact"
                    }
                }],
                "stop_reason": "tool_use"
            }),
            json!({
                "id": "msg_review_done",
                "model": "claude-sonnet-4-5",
                "role": "assistant",
                "content": [{"type": "text", "text": "review complete"}],
                "stop_reason": "end_turn"
            }),
        ])));
        let mut auth_store = AuthStore::default();
        let turn = crate::runtime::execute_user_prompt_with_tool_filter(
            &mut side_turn.state,
            &side_turn.resources,
            &providers,
            &mut auth_store,
            MEMORY_REVIEW_PROMPT,
            side_turn.filter.as_ref(),
        )
        .expect("execute side turn");
        assert_eq!(turn.assistant_text, "review complete");
        assert_eq!(turn.tool_invocations.len(), 4);
        assert_eq!(turn.tool_invocations[0].tool_id, "Skill");
        assert!(turn.tool_invocations[0].success);
        assert!(turn.tool_invocations[0]
            .output
            .contains("<command-name>project-memory</command-name>"));
        assert_eq!(turn.tool_invocations[1].tool_id, "Read");
        assert!(!turn.tool_invocations[1].success);
        assert!(turn.tool_invocations[1]
            .output
            .contains("slash command tool scope denied this tool call"));
        assert_eq!(turn.tool_invocations[2].tool_id, "Read");
        assert!(turn.tool_invocations[2].success);
        assert!(turn.tool_invocations[2]
            .output
            .contains("original durable fact"));
        assert_eq!(turn.tool_invocations[3].tool_id, "Memory");
        assert!(turn.tool_invocations[3].success);
        assert_eq!(
            std::fs::read_to_string(&memory_file).unwrap(),
            "updated durable fact"
        );
    }

    #[test]
    fn review_counter_triggers_at_interval() {
        let mut state = AppState::new(
            puffer_config::PufferConfig {
                memory: MemoryConfig {
                    review_nudge_interval: 2,
                    ..MemoryConfig::default()
                },
                ..puffer_config::PufferConfig::default()
            },
            PathBuf::from("/workspace/demo"),
            SessionMetadata {
                id: Uuid::nil(),
                display_name: None,
                generated_title: None,
                cwd: PathBuf::from("/workspace/demo"),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        );
        state.project_memory = Some(ProjectMemoryContext {
            project_name: "demo".to_string(),
            project_root: PathBuf::from("/workspace/demo"),
            memory_file: PathBuf::from("/tmp/MEMORY.md"),
            char_limit: 6_000,
        });
        assert!(!project_memory_turn_completed(&mut state));
        assert!(project_memory_turn_completed(&mut state));
    }

    #[test]
    fn project_memory_skill_reminder_points_to_builtin_skill_and_memory_file() {
        let mut state = AppState::new(
            puffer_config::PufferConfig::default(),
            PathBuf::from("/workspace/demo"),
            SessionMetadata {
                id: Uuid::nil(),
                display_name: None,
                generated_title: None,
                cwd: PathBuf::from("/workspace/demo"),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        );
        state.project_memory = Some(ProjectMemoryContext {
            project_name: "demo".to_string(),
            project_root: PathBuf::from("/workspace/demo"),
            memory_file: PathBuf::from("/tmp/MEMORY.md"),
            char_limit: 6_000,
        });

        let reminder = project_memory_skill_reminder(&state).expect("project memory reminder");
        assert!(reminder.contains("skill: \"project-memory\""));
        assert!(reminder.contains("/tmp/MEMORY.md"));
        assert!(reminder.contains("Memory tool"));
    }

    fn project_memory_state(cwd: &Path, memory_file: &Path) -> AppState {
        let mut state = AppState::new(
            puffer_config::PufferConfig::default(),
            cwd.to_path_buf(),
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
            },
        );
        state.project_memory = Some(ProjectMemoryContext {
            project_name: "demo".to_string(),
            project_root: cwd.to_path_buf(),
            memory_file: memory_file.to_path_buf(),
            char_limit: 6_000,
        });
        state
    }

    fn memory_tool_resources() -> LoadedResources {
        LoadedResources {
            tools: vec![memory_tool_spec(), skill_tool_spec(), read_tool_spec()],
            skills: vec![project_memory_skill_spec()],
            ..LoadedResources::default()
        }
    }

    fn memory_tool_spec() -> LoadedItem<ToolSpec> {
        LoadedItem {
            value: ToolSpec {
                id: "Memory".to_string(),
                name: "Memory".to_string(),
                description: "Project memory tool".to_string(),
                handler: "runtime:project_memory".to_string(),
                approval_policy: Some("auto".to_string()),
                sandbox_policy: Some("workspace-write".to_string()),
                input_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "action": {"type": "string"},
                        "content": {"type": "string"},
                        "old_text": {"type": "string"}
                    },
                    "required": ["action"]
                })),
                ..ToolSpec::default()
            },
            source_info: SourceInfo {
                path: PathBuf::from("memory.yaml"),
                kind: SourceKind::Builtin,
            },
        }
    }

    fn skill_tool_spec() -> LoadedItem<ToolSpec> {
        LoadedItem {
            value: ToolSpec {
                id: "Skill".to_string(),
                name: "Skill".to_string(),
                description: "Skill tool".to_string(),
                handler: "runtime:skill".to_string(),
                approval_policy: Some("auto".to_string()),
                sandbox_policy: Some("read-only".to_string()),
                input_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "skill": {"type": "string"},
                        "args": {"type": "string"}
                    },
                    "required": ["skill"]
                })),
                ..ToolSpec::default()
            },
            source_info: SourceInfo {
                path: PathBuf::from("skill.yaml"),
                kind: SourceKind::Builtin,
            },
        }
    }

    fn read_tool_spec() -> LoadedItem<ToolSpec> {
        LoadedItem {
            value: ToolSpec {
                id: "Read".to_string(),
                name: "Read".to_string(),
                description: "Read tool".to_string(),
                handler: "runtime:claude_read".to_string(),
                approval_policy: Some("auto".to_string()),
                sandbox_policy: Some("read-only".to_string()),
                input_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                        "offset": {"type": "integer"},
                        "limit": {"type": "integer"},
                        "pages": {"type": "string"}
                    },
                    "required": ["file_path"]
                })),
                ..ToolSpec::default()
            },
            source_info: SourceInfo {
                path: PathBuf::from("read.yaml"),
                kind: SourceKind::Builtin,
            },
        }
    }

    fn project_memory_skill_spec() -> LoadedItem<SkillSpec> {
        LoadedItem {
            value: SkillSpec {
                name: "project-memory".to_string(),
                description: "Load durable project memory".to_string(),
                content: "Load the active project's durable memory from `$ARGUMENTS`.\n\n1. Use `Read` on `$ARGUMENTS`.\n2. Treat the file contents as durable project-specific context for the current task.\n3. If the file is missing or empty, continue without inventing memory.\n4. If you learn a durable new fact, use the Memory tool to add, replace, or remove entries. Do not edit `MEMORY.md` directly.".to_string(),
                allowed_tools: vec!["Read".to_string(), "Memory".to_string()],
                argument_hint: Some("<memory-file>".to_string()),
                ..SkillSpec::default()
            },
            source_info: SourceInfo {
                path: PathBuf::from("skills/project-memory/SKILL.md"),
                kind: SourceKind::Builtin,
            },
        }
    }

    fn local_provider(base_url: String) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "local-anthropic".to_string(),
            display_name: "Local Anthropic".to_string(),
            base_url,
            default_api: "anthropic-messages".to_string(),
            auth_modes: Vec::new(),
            headers: Default::default(),
            query_params: Default::default(),
            discovery: None,
            models: vec![ModelDescriptor {
                id: "claude-sonnet-4-5".to_string(),
                display_name: "Claude Sonnet 4.5".to_string(),
                provider: "local-anthropic".to_string(),
                api: "anthropic-messages".to_string(),
                context_window: 200_000,
                max_output_tokens: 8_192,
                supports_reasoning: true,
                input: Vec::new(),
                cost: None,
                compat: None,
            }],
            chat_completions_path: None,
        }
    }

    fn spawn_json_server(responses: Vec<Value>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for response_body in responses {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut buffer = [0_u8; 8192];
                let _ = stream.read(&mut buffer);
                let body = response_body.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(), body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });
        format!("http://{address}")
    }

    fn wait_for_memory_entry(memory_file: &Path, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let contents = std::fs::read_to_string(memory_file).unwrap_or_default();
            if contents.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle} in {}",
                memory_file.display()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}
