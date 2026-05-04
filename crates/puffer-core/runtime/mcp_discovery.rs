//! Per-turn MCP tool discovery.
//!
//! Walks every MCP server known to the runner, fetches its `tools/list`
//! response, and registers each tool in the per-turn `ToolRegistry` under
//! a qualified `mcp__<server>__<tool>` id so the model can call it
//! directly (mirroring codex's `list_all_tools` → `qualify_tools` →
//! `ToolRouter::from_config` pattern). The dispatch side is wired in
//! `local_tools::execute_mcp_call`.
//!
//! Errors from one server (transient transport failures, OAuth not yet
//! pushed, etc.) are logged and the rest of the discovery proceeds, so
//! a single misbehaving server doesn't blank out the whole tool list.

use puffer_resources::LoadedResources;
use puffer_runner_api::ToolRunner;
use puffer_tools::{McpToolEntry, ToolRegistry};

/// Builds a per-turn `ToolRegistry` from `resources` and layers in any
/// MCP-discovered tools advertised by `runner`. Callers should prefer
/// this over `ToolRegistry::from_resources(...)` whenever they hold a
/// `ToolRunner` — otherwise the model never sees `mcp__*` entries.
pub fn registry_with_mcp_tools(
    resources: &LoadedResources,
    runner: &dyn ToolRunner,
) -> ToolRegistry {
    let mut registry = ToolRegistry::from_resources(resources);
    discover_and_register_mcp_tools(&mut registry, runner);
    registry
}

const MAX_DISCOVERED_TOOLS: usize = 100;

/// Discover MCP tools from `runner` and register them on `registry`.
/// Caller-side filter: the synthetic `filesystem` server is skipped because
/// `Read` / `ListDir` / `Glob` already surface its content directly.
pub fn discover_and_register_mcp_tools(registry: &mut ToolRegistry, runner: &dyn ToolRunner) {
    let servers = match runner.list_mcp_servers() {
        Ok(servers) => servers,
        Err(error) => {
            tracing::debug!("mcp_discovery: list_mcp_servers failed: {error}");
            return;
        }
    };

    let mut entries: Vec<McpToolEntry> = Vec::new();
    for server in servers {
        if server.id.eq_ignore_ascii_case("filesystem") {
            continue;
        }
        match runner.list_mcp_tools(&server.id) {
            Ok(tools) => {
                for tool in tools {
                    entries.push(McpToolEntry {
                        server: server.id.clone(),
                        name: tool.name,
                        description: tool.description,
                        input_schema: tool.input_schema,
                    });
                }
            }
            Err(error) => {
                tracing::debug!(
                    "mcp_discovery: list_mcp_tools({}) failed: {error}",
                    server.id
                );
            }
        }
    }

    if entries.len() > MAX_DISCOVERED_TOOLS {
        tracing::warn!(
            "mcp_discovery: {} MCP tools exceeds soft cap {} — keeping all but consider tool_search",
            entries.len(),
            MAX_DISCOVERED_TOOLS
        );
    }

    if let Err(error) = registry.register_mcp_tools(entries) {
        tracing::debug!("mcp_discovery: register_mcp_tools failed: {error}");
    }
}
