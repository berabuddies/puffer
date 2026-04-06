use crate::hooks::run_resource_hooks;
use puffer_resources::LoadedResources;
use serde_json::Value;
use std::path::Path;

pub(super) fn run_tool_hooks(
    resources: &LoadedResources,
    cwd: &Path,
    event: &str,
    tool_id: &str,
    input: &Value,
    success: bool,
    stdout: &str,
    stderr: &str,
) {
    run_resource_hooks(
        resources,
        cwd,
        event,
        &[
            ("PUFFER_TOOL_ID", tool_id.to_string()),
            ("PUFFER_TOOL_INPUT", input.to_string()),
            (
                "PUFFER_TOOL_SUCCESS",
                if success { "true" } else { "false" }.to_string(),
            ),
            ("PUFFER_TOOL_STDOUT", stdout.to_string()),
            ("PUFFER_TOOL_STDERR", stderr.to_string()),
        ],
    );
}

pub(super) fn run_turn_hooks(
    resources: &LoadedResources,
    cwd: &Path,
    text: &str,
    tool_count: usize,
) {
    run_resource_hooks(
        resources,
        cwd,
        "turn_end",
        &[
            ("PUFFER_TURN_TEXT", text.to_string()),
            ("PUFFER_TURN_TOOL_COUNT", tool_count.to_string()),
        ],
    );
}
