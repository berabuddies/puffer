//! End-to-end coverage for the MCP tool advertising bridge (Pass 1.5g).
//!
//! Walks the same path the OpenAI/Anthropic agent loops take when building
//! the per-turn `ToolRegistry`: `discover_and_register_mcp_tools` calls
//! `runner.list_mcp_servers()` + `list_mcp_tools(server)` and registers
//! each entry under its qualified `mcp__<server>__<tool>` id. Asserts:
//!
//! * the qualified definition exists with `runtime:mcp_call` handler,
//! * `handler_args = [server, tool]` so the runtime executor recovers the
//!   raw names without having to undo sanitization,
//! * the advertised JSON Schema round-trips (model sees the real shape),
//! * the synthetic `filesystem` server is excluded (its content is
//!   surfaced via Read/ListDir/Glob).

use puffer_core::runner_adapter::LocalToolRunner;
use puffer_core::mcp_discovery::registry_with_mcp_tools;
use puffer_resources::{LoadedResources, McpServerSpec};
use puffer_runner_api::ToolRunner;
use puffer_tools::ToolRegistry;
use std::sync::Arc;

const STUB_BIN: &str = env!("CARGO_BIN_EXE_puffer-mcp-stub-server");

fn stub_manifest(marker: &str) -> Vec<McpServerSpec> {
    vec![
        McpServerSpec {
            id: "filesystem".into(),
            display_name: "Filesystem".into(),
            transport: "stdio".into(),
            endpoint: String::new(),
            target: "builtin:filesystem".into(),
            description: "Workspace filesystem stub".into(),
            headers: Default::default(),
            oauth: None,
        },
        McpServerSpec {
            id: "stub".into(),
            display_name: "Stub".into(),
            transport: "stdio".into(),
            endpoint: String::new(),
            target: format!("'{}' --marker {}", STUB_BIN, marker),
            description: "Integration-test stub MCP server".into(),
            headers: Default::default(),
            oauth: None,
        },
    ]
}

fn registry_for(runner: &dyn ToolRunner) -> ToolRegistry {
    let resources = LoadedResources::default();
    registry_with_mcp_tools(&resources, runner)
}

#[test]
fn discover_registers_qualified_mcp_tools_from_local_runner() {
    let runner =
        LocalToolRunner::new().with_mcp_servers(stub_manifest("puffer-mcp-bridge-local"));
    let registry = registry_for(&runner);

    let echo = registry
        .definition("mcp__stub__echo")
        .expect("echo is advertised under qualified id");
    assert_eq!(echo.handler, "runtime:mcp_call");
    assert_eq!(
        echo.handler_args,
        vec!["stub".to_string(), "echo".to_string()]
    );

    // The schema the stub server returned must round-trip into the
    // model-visible definition; without it the model can't call the tool.
    let schema = echo.input_schema.as_json_schema();
    assert_eq!(schema["type"].as_str(), Some("object"));
    assert!(
        schema["properties"]["text"].is_object(),
        "echo input schema preserves `text` property: {schema}"
    );

    // Filesystem stub is intentionally suppressed — Read/ListDir/Glob
    // already cover that surface.
    for definition in registry.definitions() {
        assert!(
            !definition.id.starts_with("mcp__filesystem__"),
            "filesystem mcp tools should not be re-advertised: {}",
            definition.id
        );
    }
}

#[test]
fn discover_is_a_noop_when_runner_has_no_servers() {
    let runner = LocalToolRunner::new();
    let registry = registry_for(&runner);
    let qualified: Vec<_> = registry
        .definitions()
        .filter(|definition| definition.id.starts_with("mcp__"))
        .map(|definition| definition.id.clone())
        .collect();
    assert!(qualified.is_empty(), "expected no mcp__ tools, got {qualified:?}");
}

#[test]
fn discover_works_through_dyn_tool_runner() {
    // Mirrors the production path where the runtime holds an Arc<dyn ToolRunner>.
    let runner: Arc<dyn ToolRunner> = Arc::new(
        LocalToolRunner::new().with_mcp_servers(stub_manifest("puffer-mcp-bridge-arc")),
    );
    let registry = registry_for(runner.as_ref());
    assert!(registry.definition("mcp__stub__echo").is_some());
    assert!(registry.definition("mcp__stub__slow_echo").is_some());
}
