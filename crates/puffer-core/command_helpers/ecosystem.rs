use super::common::open_text_file_in_editor;
use super::emit_system;
use super::CommandActionEntry;
use crate::AppState;
use anyhow::Result;
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_resources::{load_resources, plugin_mcp_servers, LoadedResources, SourceKind};
use puffer_session_store::SessionStore;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

/// Backward-compatible alias for MCP action picker rows.
pub type McpActionEntry = CommandActionEntry;

/// Lists loaded MCP servers from both resource packs and plugins.
#[allow(dead_code)]
pub(crate) fn list_mcp_servers(
    state: &mut AppState,
    resources: &LoadedResources,
    session_store: &SessionStore,
) -> Result<()> {
    let servers = plugin_mcp_servers(resources);
    if servers.is_empty() && resources.mcp_servers.is_empty() {
        return emit_system(
            state,
            session_store,
            "No MCP servers are configured.".to_string(),
        );
    }
    let mut text = String::from("MCP servers:\n");
    for server in &resources.mcp_servers {
        let _ = writeln!(
            &mut text,
            "{} [{}] -> {}",
            server.value.id, server.value.transport, server.value.endpoint
        );
    }
    for (plugin, server) in servers {
        let target = if server.target.is_empty() {
            server.endpoint.as_str()
        } else {
            server.target.as_str()
        };
        let _ = writeln!(
            &mut text,
            "{}:{} [{}] -> {}",
            plugin.id, server.id, server.transport, target
        );
    }
    emit_system(state, session_store, text)
}

/// Lists loaded IDE integration manifests.
pub(crate) fn list_ides(
    state: &mut AppState,
    resources: &LoadedResources,
    session_store: &SessionStore,
) -> Result<()> {
    emit_system(
        state,
        session_store,
        render_ide_listing(&collect_ide_entries(resources)),
    )
}

/// Shows or materializes the workspace MCP directory.
pub(crate) fn handle_mcp_command(
    state: &mut AppState,
    resources: &LoadedResources,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let paths = ConfigPaths::discover(&state.cwd);
    ensure_workspace_dirs(&paths)?;
    let mcp_dir = paths.workspace_config_dir.join("resources/mcp_servers");
    fs::create_dir_all(&mcp_dir)?;
    let server_path = mcp_dir.join("workspace.yaml");
    if !server_path.exists() {
        fs::write(&server_path, default_mcp_contents())?;
    }
    let state_path = mcp_enablement_path(&paths);
    let mut enablement = load_or_initialize_mcp_enablement(&state_path)?;
    let entries = collect_mcp_entries(resources);
    let trimmed = args.trim();

    if let Some(raw_selector) = trimmed.strip_prefix("enable ") {
        let selector = raw_selector.trim();
        if selector.is_empty() {
            return emit_system(
                state,
                session_store,
                "Usage: /mcp enable <server-name>".to_string(),
            );
        }
        if !entries.iter().any(|entry| entry.selector == selector) {
            return emit_system(
                state,
                session_store,
                format!("Unknown MCP server `{selector}`."),
            );
        }
        if enablement.enable(selector) {
            write_mcp_enablement(&state_path, &enablement)?;
            state.reload_resources_requested = true;
            return emit_system(
                state,
                session_store,
                format!(
                    "Enabled MCP server `{selector}` in {}.",
                    state_path.display()
                ),
            );
        }
        return emit_system(
            state,
            session_store,
            format!("MCP server `{selector}` is already enabled."),
        );
    }

    if let Some(raw_selector) = trimmed.strip_prefix("disable ") {
        let selector = raw_selector.trim();
        if selector.is_empty() {
            return emit_system(
                state,
                session_store,
                "Usage: /mcp disable <server-name>".to_string(),
            );
        }
        if !entries.iter().any(|entry| entry.selector == selector) {
            return emit_system(
                state,
                session_store,
                format!("Unknown MCP server `{selector}`."),
            );
        }
        if enablement.disable(selector) {
            write_mcp_enablement(&state_path, &enablement)?;
            state.reload_resources_requested = true;
            return emit_system(
                state,
                session_store,
                format!(
                    "Disabled MCP server `{selector}` in {}.",
                    state_path.display()
                ),
            );
        }
        return emit_system(
            state,
            session_store,
            format!("MCP server `{selector}` is already disabled."),
        );
    }

    if args.trim() == "path" {
        return emit_system(
            state,
            session_store,
            format!(
                "MCP directory: {}\nMCP enablement file: {}",
                mcp_dir.display(),
                state_path.display()
            ),
        );
    }
    if trimmed == "list" {
        return emit_system(
            state,
            session_store,
            render_mcp_listing(&entries, &enablement),
        );
    }
    if trimmed == "reload" {
        state.reload_resources_requested = true;
        return emit_system(
            state,
            session_store,
            "Reloading MCP changes from disk for this session...".to_string(),
        );
    }
    if matches!(trimmed, "open" | "edit") {
        return open_mcp_manifest(state, session_store, &server_path);
    }
    if let Some(selector) = trimmed
        .strip_prefix("show ")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return describe_mcp_entry(state, session_store, &entries, &enablement, selector);
    }
    if let Some(selector) = trimmed
        .split_once(' ')
        .filter(|(command, _)| matches!(*command, "open" | "edit"))
        .map(|(_, selector)| selector.trim())
        .filter(|value| !value.is_empty())
    {
        return open_named_mcp_manifest(state, session_store, &entries, selector);
    }

    if !trimmed.is_empty() && trimmed != "show" {
        return emit_system(
            state,
            session_store,
            "Usage: /mcp [show|show <server-name>|list|path|open [server-name]|edit [server-name]|enable <server-name>|disable <server-name>|reload]".to_string(),
        );
    }

    emit_system(state, session_store, render_mcp_summary(state, resources)?)
}

/// Shows or materializes the workspace IDE integration directory.
pub(crate) fn handle_ide_command(
    state: &mut AppState,
    resources: &LoadedResources,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let paths = ConfigPaths::discover(&state.cwd);
    ensure_workspace_dirs(&paths)?;
    let ide_dir = paths.workspace_config_dir.join("resources/ides");
    fs::create_dir_all(&ide_dir)?;
    let workspace_manifest = ensure_workspace_ide_manifest(&ide_dir)?;
    let entries = collect_ide_entries(resources);
    let trimmed = args.trim();

    if trimmed == "path" {
        return emit_system(
            state,
            session_store,
            format!(
                "IDE directory: {}\nWorkspace IDE manifest: {}",
                ide_dir.display(),
                workspace_manifest.display()
            ),
        );
    }
    if trimmed == "list" {
        return emit_system(state, session_store, render_ide_listing(&entries));
    }
    if matches!(trimmed, "open" | "edit") {
        return open_ide_manifest(state, session_store, &workspace_manifest);
    }
    if let Some(selector) = trimmed
        .split_once(' ')
        .filter(|(command, _)| matches!(*command, "open" | "edit"))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
    {
        return open_named_ide_manifest(state, session_store, &entries, selector);
    }
    if let Some(selector) = trimmed
        .strip_prefix("show ")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return describe_ide_entry(state, session_store, &entries, selector);
    }
    if entries.iter().any(|entry| entry.selector == trimmed) {
        return describe_ide_entry(state, session_store, &entries, trimmed);
    }
    emit_system(
        state,
        session_store,
        render_ide_summary(&ide_dir, &workspace_manifest, &entries)?,
    )
}

/// Reloads declarative resources from disk and applies workspace MCP enablement.
#[allow(dead_code)]
pub(crate) fn reload_resources_from_disk(state: &AppState) -> Result<LoadedResources> {
    let paths = ConfigPaths::discover(&state.cwd);
    ensure_workspace_dirs(&paths)?;
    let mut resources = load_resources(&paths, state.tool_runner.as_ref())?;
    apply_mcp_enablement_overrides(&paths, &mut resources)?;
    Ok(resources)
}

fn default_mcp_contents() -> &'static str {
    "id: workspace\n\
display_name: Workspace MCP\n\
transport: stdio\n\
endpoint: \"\"\n\
target: workspace\n\
description: Example MCP server\n\
enabled: true\n"
}

fn default_ide_contents() -> &'static str {
    "id: workspace\n\
display_name: Workspace IDE\n\
description: Example IDE integration\n"
}

/// Builds the interactive `/ide` action list used by the TUI picker.
pub(crate) fn render_ide_actions(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Vec<CommandActionEntry>> {
    let paths = ConfigPaths::discover(&state.cwd);
    ensure_workspace_dirs(&paths)?;
    let ide_dir = paths.workspace_config_dir.join("resources/ides");
    fs::create_dir_all(&ide_dir)?;
    let workspace_manifest = ensure_workspace_ide_manifest(&ide_dir)?;
    let entries = collect_ide_entries(resources);
    let mut actions = vec![
        CommandActionEntry {
            command: "/ide open".to_string(),
            description: format!(
                "Open workspace IDE manifest ({})",
                workspace_manifest.display()
            ),
        },
        CommandActionEntry {
            command: "/ide path".to_string(),
            description: "Show IDE resource paths".to_string(),
        },
        CommandActionEntry {
            command: "/ide list".to_string(),
            description: "List loaded IDE integrations".to_string(),
        },
    ];
    for entry in &entries {
        actions.push(CommandActionEntry {
            command: format!("/ide show {}", entry.selector),
            description: format!("{} [{}] {}", entry.label, entry.source, entry.description),
        });
        actions.push(CommandActionEntry {
            command: format!("/ide open {}", entry.selector),
            description: format!("Open manifest {}", entry.path.display()),
        });
    }
    Ok(actions)
}

/// Renders the MCP summary shown by `/mcp` with no arguments.
pub(crate) fn render_mcp_summary(state: &AppState, resources: &LoadedResources) -> Result<String> {
    let paths = ConfigPaths::discover(&state.cwd);
    ensure_workspace_dirs(&paths)?;
    let mcp_dir = paths.workspace_config_dir.join("resources/mcp_servers");
    fs::create_dir_all(&mcp_dir)?;
    let server_path = mcp_dir.join("workspace.yaml");
    if !server_path.exists() {
        fs::write(&server_path, default_mcp_contents())?;
    }
    let state_path = mcp_enablement_path(&paths);
    let enablement = load_or_initialize_mcp_enablement(&state_path)?;
    let entries = collect_mcp_entries(resources);

    let mut summary = String::new();
    let _ = writeln!(
        &mut summary,
        "{}",
        render_mcp_listing(&entries, &enablement)
    );
    let _ = writeln!(&mut summary);
    let _ = writeln!(
        &mut summary,
        "Use `/mcp enable <server-name>`, `/mcp disable <server-name>`, `/mcp open <server-name>`, or `/mcp reload` to manage MCP state."
    );
    Ok(format!(
        "MCP directory: {}\nMCP enablement file: {}\n{}{}",
        mcp_dir.display(),
        state_path.display(),
        summary,
        fs::read_to_string(&server_path)?
    ))
}

/// Builds the interactive `/mcp` action list used by the TUI picker.
pub(crate) fn render_mcp_actions(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Vec<McpActionEntry>> {
    let paths = ConfigPaths::discover(&state.cwd);
    ensure_workspace_dirs(&paths)?;
    let mcp_dir = paths.workspace_config_dir.join("resources/mcp_servers");
    fs::create_dir_all(&mcp_dir)?;
    let workspace_manifest = mcp_dir.join("workspace.yaml");
    if !workspace_manifest.exists() {
        fs::write(&workspace_manifest, default_mcp_contents())?;
    }
    let enablement = load_or_initialize_mcp_enablement(&mcp_enablement_path(&paths))?;
    let entries = collect_mcp_entries(resources);
    let mut actions = vec![
        McpActionEntry {
            command: "/mcp open".to_string(),
            description: format!(
                "Edit workspace MCP manifest ({})",
                workspace_manifest.display()
            ),
        },
        McpActionEntry {
            command: "/mcp reload".to_string(),
            description: "Reload MCP changes from disk for this session".to_string(),
        },
    ];
    for entry in &entries {
        let status = if enablement.is_disabled(&entry.selector) {
            "disabled"
        } else {
            "enabled"
        };
        actions.push(McpActionEntry {
            command: format!(
                "/mcp {} {}",
                if status == "disabled" {
                    "enable"
                } else {
                    "disable"
                },
                entry.selector
            ),
            description: format!(
                "{} [{}] {} -> {} ({})",
                entry.label,
                status,
                entry.transport,
                if entry.target.is_empty() {
                    "<unset>"
                } else {
                    entry.target.as_str()
                },
                entry.source
            ),
        });
        actions.push(McpActionEntry {
            command: format!("/mcp open {}", entry.selector),
            description: format!("Open manifest {}", entry.path.display()),
        });
    }
    Ok(actions)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct McpEnablement {
    #[serde(default)]
    disabled: Vec<String>,
}

impl McpEnablement {
    fn is_disabled(&self, selector: &str) -> bool {
        let normalized = normalize_selector(selector);
        self.disabled.iter().any(|item| item == &normalized)
    }

    fn enable(&mut self, selector: &str) -> bool {
        let normalized = normalize_selector(selector);
        let before = self.disabled.len();
        self.disabled.retain(|item| item != &normalized);
        before != self.disabled.len()
    }

    fn disable(&mut self, selector: &str) -> bool {
        let normalized = normalize_selector(selector);
        if self.disabled.iter().any(|item| item == &normalized) {
            return false;
        }
        self.disabled.push(normalized);
        self.disabled.sort();
        self.disabled.dedup();
        true
    }
}

#[derive(Debug, Clone)]
struct McpEntry {
    selector: String,
    label: String,
    transport: String,
    target: String,
    source: String,
    path: PathBuf,
}

fn collect_mcp_entries(resources: &LoadedResources) -> Vec<McpEntry> {
    let mut entries = Vec::new();
    for server in &resources.mcp_servers {
        entries.push(McpEntry {
            selector: server.value.id.clone(),
            label: if server.value.display_name.is_empty() {
                server.value.id.clone()
            } else {
                server.value.display_name.clone()
            },
            transport: server.value.transport.clone(),
            target: if server.value.target.is_empty() {
                server.value.endpoint.clone()
            } else {
                server.value.target.clone()
            },
            source: format!("resource:{}", source_kind_label(server.source_info.kind)),
            path: server.source_info.path.clone(),
        });
    }
    for (plugin, server) in plugin_mcp_servers(resources) {
        let Some(plugin_item) = resources
            .plugins
            .iter()
            .find(|item| item.value.id == plugin.id)
        else {
            continue;
        };
        entries.push(McpEntry {
            selector: format!("{}:{}", plugin.id, server.id),
            label: if server.display_name.is_empty() {
                format!("{}:{}", plugin.id, server.id)
            } else {
                server.display_name.clone()
            },
            transport: server.transport.clone(),
            target: if server.target.is_empty() {
                server.endpoint.clone()
            } else {
                server.target.clone()
            },
            source: format!("plugin:{}", plugin.id),
            path: plugin_item.source_info.path.clone(),
        });
    }
    entries.sort_by(|left, right| left.selector.cmp(&right.selector));
    entries
}

fn describe_mcp_entry(
    state: &mut AppState,
    session_store: &SessionStore,
    entries: &[McpEntry],
    enablement: &McpEnablement,
    selector: &str,
) -> Result<()> {
    let Some(entry) = entries.iter().find(|entry| entry.selector == selector) else {
        return emit_system(
            state,
            session_store,
            format!("Unknown MCP server `{selector}`."),
        );
    };
    let status = if enablement.is_disabled(&entry.selector) {
        "disabled"
    } else {
        "enabled"
    };
    emit_system(
        state,
        session_store,
        format!(
            "MCP server {}\nname={}\nstatus={}\ntransport={}\ntarget={}\nsource={}\npath={}",
            entry.selector,
            entry.label,
            status,
            entry.transport,
            if entry.target.is_empty() {
                "<unset>"
            } else {
                entry.target.as_str()
            },
            entry.source,
            entry.path.display()
        ),
    )
}

fn open_named_mcp_manifest(
    state: &mut AppState,
    session_store: &SessionStore,
    entries: &[McpEntry],
    selector: &str,
) -> Result<()> {
    let Some(entry) = entries.iter().find(|entry| entry.selector == selector) else {
        return emit_system(
            state,
            session_store,
            format!("Unknown MCP server `{selector}`."),
        );
    };
    open_mcp_manifest(state, session_store, &entry.path)
}

fn open_mcp_manifest(
    state: &mut AppState,
    session_store: &SessionStore,
    path: &PathBuf,
) -> Result<()> {
    match open_text_file_in_editor(path) {
        Ok(status) => emit_system(state, session_store, status),
        Err(error) => emit_system(
            state,
            session_store,
            format!(
                "Could not open MCP manifest in an editor: {error}\nPath: {}",
                path.display()
            ),
        ),
    }
}

fn render_mcp_listing(entries: &[McpEntry], enablement: &McpEnablement) -> String {
    if entries.is_empty() {
        return "No MCP servers are configured.".to_string();
    }
    let mut text = String::from("MCP servers:\n");
    for entry in entries {
        let status = if enablement.is_disabled(&entry.selector) {
            "disabled"
        } else {
            "enabled"
        };
        let target = if entry.target.is_empty() {
            "<unset>"
        } else {
            entry.target.as_str()
        };
        let label = if entry.label != entry.selector {
            format!(" ({})", entry.label)
        } else {
            String::new()
        };
        let _ = writeln!(
            &mut text,
            "- {}{} [{}] {} -> {} ({})",
            entry.selector, label, status, entry.transport, target, entry.source
        );
    }
    text
}

#[derive(Debug, Clone)]
struct IdeEntry {
    selector: String,
    label: String,
    description: String,
    source: String,
    path: PathBuf,
}

fn ensure_workspace_ide_manifest(ide_dir: &PathBuf) -> Result<PathBuf> {
    let workspace_manifest = ide_dir.join("workspace.yaml");
    if !workspace_manifest.exists() {
        fs::write(&workspace_manifest, default_ide_contents())?;
    }
    Ok(workspace_manifest)
}

fn collect_ide_entries(resources: &LoadedResources) -> Vec<IdeEntry> {
    let mut entries = resources
        .ides
        .iter()
        .map(|ide| IdeEntry {
            selector: ide.value.id.clone(),
            label: ide.value.display_name.clone(),
            description: ide.value.description.clone(),
            source: source_kind_label(ide.source_info.kind).to_string(),
            path: ide.source_info.path.clone(),
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.selector.cmp(&right.selector));
    entries
}

fn render_ide_summary(
    ide_dir: &PathBuf,
    workspace_manifest: &PathBuf,
    entries: &[IdeEntry],
) -> Result<String> {
    let listing = if entries.is_empty() {
        "No IDE integrations are configured.".to_string()
    } else {
        render_ide_listing(entries)
    };
    Ok(format!(
        "IDE directory: {}\nWorkspace IDE manifest: {}\n{}\n\n{}",
        ide_dir.display(),
        workspace_manifest.display(),
        listing,
        fs::read_to_string(workspace_manifest)?
    ))
}

fn render_ide_listing(entries: &[IdeEntry]) -> String {
    if entries.is_empty() {
        return "No IDE integrations are configured.".to_string();
    }
    let mut text = String::from("IDE integrations:\n");
    for entry in entries {
        let _ = writeln!(
            &mut text,
            "- {} ({}) [{}] {}",
            entry.selector, entry.label, entry.source, entry.description
        );
    }
    text.trim_end().to_string()
}

fn describe_ide_entry(
    state: &mut AppState,
    session_store: &SessionStore,
    entries: &[IdeEntry],
    selector: &str,
) -> Result<()> {
    let Some(entry) = entries.iter().find(|entry| entry.selector == selector) else {
        return emit_system(
            state,
            session_store,
            format!("Unknown IDE integration `{selector}`."),
        );
    };
    emit_system(
        state,
        session_store,
        format!(
            "IDE integration {}\nname={}\nsource={}\npath={}\ndescription={}",
            entry.selector,
            entry.label,
            entry.source,
            entry.path.display(),
            if entry.description.is_empty() {
                "<none>"
            } else {
                entry.description.as_str()
            }
        ),
    )
}

fn open_named_ide_manifest(
    state: &mut AppState,
    session_store: &SessionStore,
    entries: &[IdeEntry],
    selector: &str,
) -> Result<()> {
    let Some(entry) = entries.iter().find(|entry| entry.selector == selector) else {
        return emit_system(
            state,
            session_store,
            format!("Unknown IDE integration `{selector}`."),
        );
    };
    open_ide_manifest(state, session_store, &entry.path)
}

fn open_ide_manifest(
    state: &mut AppState,
    session_store: &SessionStore,
    path: &PathBuf,
) -> Result<()> {
    match open_text_file_in_editor(path) {
        Ok(status) => emit_system(state, session_store, status),
        Err(error) => emit_system(
            state,
            session_store,
            format!(
                "Could not open IDE manifest in an editor: {error}\nPath: {}",
                path.display()
            ),
        ),
    }
}

fn mcp_enablement_path(paths: &ConfigPaths) -> PathBuf {
    paths.workspace_config_dir.join("mcp_servers.toml")
}

fn load_or_initialize_mcp_enablement(path: &PathBuf) -> Result<McpEnablement> {
    if path.exists() {
        return Ok(toml::from_str(&fs::read_to_string(path)?)?);
    }
    let default = McpEnablement::default();
    write_mcp_enablement(path, &default)?;
    Ok(default)
}

fn write_mcp_enablement(path: &PathBuf, value: &McpEnablement) -> Result<()> {
    fs::write(path, toml::to_string_pretty(value)?)?;
    Ok(())
}

fn normalize_selector(selector: &str) -> String {
    selector.trim().to_ascii_lowercase()
}

fn apply_mcp_enablement_overrides(
    paths: &ConfigPaths,
    resources: &mut LoadedResources,
) -> Result<()> {
    let settings = load_or_initialize_mcp_enablement(&mcp_enablement_path(paths))?;
    if settings.disabled.is_empty() {
        return Ok(());
    }
    resources
        .mcp_servers
        .retain(|server| !settings.is_disabled(&server.value.id));
    for plugin in &mut resources.plugins {
        let plugin_id = plugin.value.id.clone();
        plugin
            .value
            .mcp_servers
            .retain(|server| !settings.is_disabled(&format!("{}:{}", plugin_id, server.id)));
    }
    Ok(())
}

fn source_kind_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Builtin => "builtin",
        SourceKind::User => "user",
        SourceKind::Workspace => "workspace",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::ConfigPaths;
    use puffer_resources::{LoadedItem, McpServerSpec, PluginSpec, SourceInfo};
    use tempfile::tempdir;

    #[test]
    fn mcp_enablement_round_trip_and_filtering_work() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let paths = ConfigPaths::discover(root);
        ensure_workspace_dirs(&paths).unwrap();

        let mut resources = LoadedResources::default();
        resources.mcp_servers.push(LoadedItem {
            value: McpServerSpec {
                id: "docs".to_string(),
                display_name: "Docs".to_string(),
                transport: "stdio".to_string(),
                endpoint: String::new(),
                target: "docs".to_string(),
                description: String::new(),
                env: Default::default(),
                inherit_env: true,
                timeout: None,
                connect_timeout: None,
                headers: Default::default(),
                oauth: None,
            },
            source_info: SourceInfo {
                path: root.join("resources/mcp_servers/docs.yaml"),
                kind: SourceKind::Builtin,
            },
        });
        resources.plugins.push(LoadedItem {
            value: PluginSpec {
                id: "workspace".to_string(),
                display_name: "Workspace".to_string(),
                description: String::new(),
                commands: Vec::new(),
                skills: Vec::new(),
                agents: Vec::new(),
                mcp_servers: vec![McpServerSpec {
                    id: "logs".to_string(),
                    display_name: "Logs".to_string(),
                    transport: "stdio".to_string(),
                    endpoint: String::new(),
                    target: "logs".to_string(),
                    description: String::new(),
                    env: Default::default(),
                    inherit_env: true,
                    timeout: None,
                    connect_timeout: None,
                    headers: Default::default(),
                    oauth: None,
                }],
                lsp_servers: Vec::new(),
            },
            source_info: SourceInfo {
                path: root.join("resources/plugins/workspace.yaml"),
                kind: SourceKind::Workspace,
            },
        });

        let state_path = mcp_enablement_path(&paths);
        let mut enablement = load_or_initialize_mcp_enablement(&state_path).unwrap();
        assert!(!enablement.is_disabled("docs"));
        assert!(enablement.disable("docs"));
        assert!(enablement.disable("workspace:logs"));
        write_mcp_enablement(&state_path, &enablement).unwrap();

        apply_mcp_enablement_overrides(&paths, &mut resources).unwrap();
        assert!(resources.mcp_servers.is_empty());
        assert!(resources.plugins[0].value.mcp_servers.is_empty());
    }
}
