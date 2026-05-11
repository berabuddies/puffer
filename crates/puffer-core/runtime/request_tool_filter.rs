pub(crate) use crate::permissions::build_request_tool_filter;
pub(crate) use crate::permissions::RequestToolFilter;

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_tools::{
        ToolDefinition, ToolDisplayHints, ToolInputSchema, ToolKind, ToolMetadata, ToolPolicyHints,
    };
    use serde_json::json;
    use std::path::Path;

    fn tool(id: &str) -> ToolDefinition {
        ToolDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: id.to_string(),
            handler: "runtime:test".to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: ToolKind::Custom,
            input_schema: ToolInputSchema::default(),
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        }
    }

    #[test]
    fn filter_maps_task_and_ls_aliases() {
        let filter = build_request_tool_filter(&["Task".to_string(), "LS".to_string()])
            .unwrap()
            .unwrap();
        assert!(filter.allows_definition(&tool("Agent")));
        assert!(filter.allows_definition(&tool("Glob")));
        assert!(!filter.allows_definition(&tool("Read")));
    }

    #[test]
    fn filter_maps_provider_specific_and_legacy_aliases() {
        let cwd = Path::new("/tmp/work");
        let filter = build_request_tool_filter(&[
            "read_file(/tmp/work/**)".to_string(),
            "replace_in_file(/tmp/work/.claude/settings.json)".to_string(),
            "Brief".to_string(),
            "AgentOutputTool".to_string(),
            "KillShell".to_string(),
        ])
        .unwrap()
        .unwrap();
        assert!(filter.allows_definition(&tool("Read")));
        assert!(filter.allows_definition(&tool("Edit")));
        assert!(filter.allows_definition(&tool("SendUserMessage")));
        assert!(filter.allows_definition(&tool("TaskOutput")));
        assert!(filter.allows_definition(&tool("TaskStop")));
        assert!(!filter.allows_definition(&tool("Write")));
        assert!(filter
            .allows_call(
                &tool("Read"),
                cwd,
                &json!({"file_path": "/tmp/work/docs/guide.md"})
            )
            .unwrap());
        assert!(filter
            .allows_call(
                &tool("Edit"),
                cwd,
                &json!({"file_path": "/tmp/work/.claude/settings.json"})
            )
            .unwrap());
    }

    #[test]
    fn filter_matches_bash_prefix_tokens() {
        let filter = build_request_tool_filter(&["Bash(git diff:*)".to_string()])
            .unwrap()
            .unwrap();
        assert!(filter
            .allows_call(
                &tool("Bash"),
                Path::new("/tmp/work"),
                &json!({"command": "git diff --name-only origin/HEAD..."})
            )
            .unwrap());
        assert!(!filter
            .allows_call(
                &tool("Bash"),
                Path::new("/tmp/work"),
                &json!({"command": "git status"})
            )
            .unwrap());
    }

    #[test]
    fn filter_matches_path_globs_against_absolute_and_relative_paths() {
        let cwd = Path::new("/tmp/work");
        let filter = build_request_tool_filter(&[
            "Read(/tmp/work/**)".to_string(),
            "Edit(/tmp/work/.claude/settings.json)".to_string(),
        ])
        .unwrap()
        .unwrap();
        assert!(filter
            .allows_call(
                &tool("Read"),
                cwd,
                &json!({"file_path": "/tmp/work/docs/guide.md"})
            )
            .unwrap());
        assert!(filter
            .allows_call(
                &tool("Edit"),
                cwd,
                &json!({"file_path": ".claude/settings.json"})
            )
            .unwrap());
        assert!(!filter
            .allows_call(
                &tool("Edit"),
                cwd,
                &json!({"file_path": "/tmp/work/src/main.rs"})
            )
            .unwrap());
    }
}
