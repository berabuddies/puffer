mod manage;
mod support;

use self::manage::{
    disable_workspace_plugin, enable_workspace_plugin, install_workspace_plugin,
    uninstall_workspace_plugin, update_workspace_plugin,
};
use self::support::{
    default_plugin_contents, format_plugin_counts, is_disabled_placeholder,
    marketplace_management_request, plugin_description, plugin_help_text, plugin_status,
    render_plugin_marketplace, source_kind_label,
};
use super::common::open_text_file_in_editor;
use super::{emit_system, CommandActionEntry};
use crate::AppState;
use anyhow::Result;
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_resources::{
    plugin_lsp_servers, plugin_mcp_servers, LoadedItem, LoadedResources, PluginSpec, SourceInfo,
    SourceKind,
};
use puffer_session_store::SessionStore;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

/// Backward-compatible alias for plugin action picker rows.
pub type PluginActionEntry = CommandActionEntry;

/// Shows or materializes the workspace plugin directory.
pub(crate) fn handle_plugin_command(
    state: &mut AppState,
    resources: &LoadedResources,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let paths = ConfigPaths::discover(&state.cwd);
    ensure_workspace_dirs(&paths)?;
    let plugins_dir = paths.workspace_config_dir.join("resources/plugins");
    fs::create_dir_all(&plugins_dir)?;
    let plugin_path = plugins_dir.join("workspace.yaml");
    if !plugin_path.exists() {
        fs::write(&plugin_path, default_plugin_contents())?;
    }
    let trimmed = args.trim();
    let inventory = plugin_inventory(&paths, resources)?;

    match trimmed {
        "help" | "-h" | "--help" => emit_system(state, session_store, plugin_help_text()),
        "" | "show" | "manage" => emit_system(
            state,
            session_store,
            render_plugin_summary(state, resources)?,
        ),
        "marketplace" | "market" | "marketplace list" | "market list" => {
            emit_system(state, session_store, render_plugin_marketplace(resources))
        }
        "errors" => emit_system(
            state,
            session_store,
            render_plugin_errors(state, resources)?,
        ),
        "path" => emit_system(
            state,
            session_store,
            format!(
                "Plugins directory: {}\nWorkspace plugin manifest: {}",
                plugins_dir.display(),
                plugin_path.display()
            ),
        ),
        "list" => emit_system(state, session_store, render_plugin_listing(&inventory)),
        "validate" => emit_system(
            state,
            session_store,
            render_plugin_validation(&inventory, None),
        ),
        "install" | "i" => emit_system(
            state,
            session_store,
            format!(
                "{}\n\n{}",
                plugin_help_text(),
                render_plugin_marketplace(resources)
            ),
        ),
        "uninstall" | "remove" | "update" => emit_system(state, session_store, plugin_help_text()),
        "reload" => {
            state.reload_resources_requested = true;
            emit_system(
                state,
                session_store,
                "Reloading plugin changes from disk for this session...".to_string(),
            )
        }
        "open" | "edit" => open_plugin_file(state, session_store, &plugin_path),
        _ if trimmed.starts_with("show ") => {
            let plugin_id = trimmed.trim_start_matches("show ").trim();
            describe_plugin(state, session_store, &inventory, plugin_id)
        }
        _ if trimmed.starts_with("install ") => {
            let plugin_ref = trimmed.trim_start_matches("install ").trim();
            install_workspace_plugin(state, resources, session_store, &paths, plugin_ref)
        }
        _ if trimmed.starts_with("i ") => {
            let plugin_ref = trimmed.trim_start_matches("i ").trim();
            install_workspace_plugin(state, resources, session_store, &paths, plugin_ref)
        }
        _ if trimmed.starts_with("uninstall ") || trimmed.starts_with("remove ") => {
            let plugin_id = trimmed
                .split_once(' ')
                .map(|(_, value)| value.trim())
                .unwrap_or_default();
            uninstall_workspace_plugin(state, resources, session_store, &paths, plugin_id)
        }
        _ if trimmed.starts_with("update ") => {
            let plugin_id = trimmed.trim_start_matches("update ").trim();
            update_workspace_plugin(state, resources, session_store, &paths, plugin_id)
        }
        _ if trimmed.starts_with("validate ") => {
            let plugin_id = trimmed.trim_start_matches("validate ").trim();
            emit_system(
                state,
                session_store,
                render_plugin_validation(&inventory, Some(plugin_id)),
            )
        }
        _ if trimmed.starts_with("open ") || trimmed.starts_with("edit ") => {
            let plugin_id = trimmed
                .split_once(' ')
                .map(|(_, value)| value.trim())
                .unwrap_or_default();
            open_named_plugin_file(state, session_store, &inventory, plugin_id)
        }
        _ if trimmed.starts_with("enable ") => {
            let plugin_id = trimmed.trim_start_matches("enable ").trim();
            enable_workspace_plugin(state, resources, session_store, &paths, plugin_id)
        }
        _ if trimmed.starts_with("disable ") => {
            let plugin_id = trimmed.trim_start_matches("disable ").trim();
            disable_workspace_plugin(state, resources, session_store, &paths, plugin_id)
        }
        _ if marketplace_management_request(trimmed) => emit_system(
            state,
            session_store,
            format!(
                "Custom plugin marketplaces are not implemented yet.\n\n{}",
                render_plugin_marketplace(resources)
            ),
        ),
        _ if inventory.iter().any(|plugin| plugin.value.id == trimmed) => {
            describe_plugin(state, session_store, &inventory, trimmed)
        }
        _ => emit_system(state, session_store, plugin_help_text()),
    }
}

/// Summarizes the current plugin registry after a reload request.
pub(crate) fn reload_plugins_summary(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<String> {
    let paths = ConfigPaths::discover(&state.cwd);
    let plugins_dir = paths.workspace_config_dir.join("resources/plugins");
    Ok(format!(
        "Reloaded plugin registry for this session.\nplugins={}\nskills={}\nmcp_servers={}\nlsp_servers={}\nsource_dir={}",
        resources.plugins.len(),
        resources.skills.len(),
        resources.mcp_servers.len() + plugin_mcp_servers(resources).len(),
        plugin_lsp_servers(resources).len(),
        plugins_dir.display()
    ))
}

/// Renders the plugin summary shown by `/plugin` with no arguments.
pub(crate) fn render_plugin_summary(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<String> {
    let paths = ConfigPaths::discover(&state.cwd);
    ensure_workspace_dirs(&paths)?;
    let plugins_dir = paths.workspace_config_dir.join("resources/plugins");
    fs::create_dir_all(&plugins_dir)?;
    let plugin_path = plugins_dir.join("workspace.yaml");
    if !plugin_path.exists() {
        fs::write(&plugin_path, default_plugin_contents())?;
    }
    let inventory = plugin_inventory(&paths, resources)?;
    Ok(format!(
        "Plugins directory: {}\nworkspace_plugin_manifest={}\nloaded_plugins={}\n{}\nUse `/plugin marketplace`, `/plugin install <id|path>`, `/plugin update <id>`, `/plugin uninstall <id>`, `/plugin enable <id>`, `/plugin disable <id>`, `/plugin open <id>`, `/plugin validate [id]`, `/plugin errors`, or `/reload-plugins`.\n\n{}",
        plugins_dir.display(),
        plugin_path.display(),
        inventory.iter().filter(|plugin| !is_disabled_placeholder(&plugin.value)).count(),
        render_plugin_listing(&inventory),
        fs::read_to_string(&plugin_path)?
    ))
}

/// Builds the interactive `/plugin` action list used by the TUI picker.
pub(crate) fn render_plugin_actions(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Vec<PluginActionEntry>> {
    let paths = ConfigPaths::discover(&state.cwd);
    let inventory = plugin_inventory(&paths, resources)?;
    let mut actions = vec![
        PluginActionEntry {
            command: "/plugin marketplace".to_string(),
            description: "Browse builtin plugins that can be installed here".to_string(),
        },
        PluginActionEntry {
            command: "/plugin open".to_string(),
            description: format!(
                "Edit workspace plugin manifest ({})",
                paths
                    .workspace_config_dir
                    .join("resources/plugins/workspace.yaml")
                    .display()
            ),
        },
        PluginActionEntry {
            command: "/reload-plugins".to_string(),
            description: "Reload plugin changes from disk for this session".to_string(),
        },
        PluginActionEntry {
            command: "/plugin errors".to_string(),
            description: "Show plugin-specific resource diagnostics".to_string(),
        },
        PluginActionEntry {
            command: "/plugin validate".to_string(),
            description: "Validate loaded plugin manifests".to_string(),
        },
    ];
    for plugin in &inventory {
        if plugin.value.id == "workspace" {
            actions.push(PluginActionEntry {
                command: format!("/plugin open {}", plugin.value.id),
                description: format!("Open manifest {}", plugin.source_info.path.display()),
            });
            actions.push(PluginActionEntry {
                command: format!("/plugin validate {}", plugin.value.id),
                description: format!("Validate plugin {}", plugin.value.id),
            });
            continue;
        }
        let status = plugin_status(&plugin.value);
        let counts = format_plugin_counts(&plugin.value);
        let label = if plugin.value.display_name == plugin.value.id {
            plugin.value.display_name.clone()
        } else {
            format!("{} ({})", plugin.value.id, plugin.value.display_name)
        };
        actions.push(PluginActionEntry {
            command: format!(
                "/plugin {} {}",
                if is_disabled_placeholder(&plugin.value) {
                    "enable"
                } else {
                    "disable"
                },
                plugin.value.id
            ),
            description: format!(
                "{} [{}] {} • {}",
                label,
                status,
                source_kind_label(plugin.source_info.kind),
                counts
            ),
        });
        actions.push(PluginActionEntry {
            command: format!("/plugin open {}", plugin.value.id),
            description: format!("Open manifest {}", plugin.source_info.path.display()),
        });
        actions.push(PluginActionEntry {
            command: format!("/plugin validate {}", plugin.value.id),
            description: format!("Validate plugin {}", plugin.value.id),
        });
        if plugin.source_info.kind == SourceKind::Workspace {
            actions.push(PluginActionEntry {
                command: format!("/plugin uninstall {}", plugin.value.id),
                description: format!("Remove workspace override for {}", plugin.value.id),
            });
            actions.push(PluginActionEntry {
                command: format!("/plugin update {}", plugin.value.id),
                description: format!("Refresh {} from builtin/user source", plugin.value.id),
            });
        } else if !is_disabled_placeholder(&plugin.value) {
            actions.push(PluginActionEntry {
                command: format!("/plugin install {}", plugin.value.id),
                description: format!("Install an editable workspace copy of {}", plugin.value.id),
            });
        }
    }
    Ok(actions)
}

fn render_plugin_errors(state: &AppState, resources: &LoadedResources) -> Result<String> {
    let paths = ConfigPaths::discover(&state.cwd);
    let plugins_dir = paths.workspace_config_dir.join("resources/plugins");
    let diagnostics = resources
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.contains(" plugin `")
                || diagnostic.contains("/plugins/")
                || diagnostic.contains("\\plugins\\")
        })
        .collect::<Vec<_>>();
    if diagnostics.is_empty() {
        return Ok(format!(
            "Plugin diagnostics\nsource_dir={}\nerrors=0\nNo plugin-specific resource diagnostics are currently recorded.",
            plugins_dir.display()
        ));
    }
    let mut text = format!(
        "Plugin diagnostics\nsource_dir={}\nerrors={}",
        plugins_dir.display(),
        diagnostics.len()
    );
    for diagnostic in diagnostics {
        let _ = writeln!(&mut text, "\n- {diagnostic}");
    }
    Ok(text)
}

fn render_plugin_validation(
    inventory: &[LoadedItem<PluginSpec>],
    plugin_id: Option<&str>,
) -> String {
    let selected = if let Some(plugin_id) = plugin_id {
        let Some(plugin) = inventory.iter().find(|plugin| plugin.value.id == plugin_id) else {
            return format!("Unknown plugin `{plugin_id}`.");
        };
        vec![plugin]
    } else {
        inventory.iter().collect::<Vec<_>>()
    };
    let mut text = String::from("Plugin validation\n");
    for plugin in selected {
        let issues = validate_plugin(plugin);
        let status = if issues.is_empty() { "ok" } else { "issues" };
        let _ = writeln!(
            &mut text,
            "- {} [{}] path={}",
            plugin.value.id,
            status,
            plugin.source_info.path.display()
        );
        if issues.is_empty() {
            let _ = writeln!(
                &mut text,
                "  commands={} skills={} mcp_servers={} lsp_servers={}",
                plugin.value.commands.len(),
                plugin.value.skills.len(),
                plugin.value.mcp_servers.len(),
                plugin.value.lsp_servers.len()
            );
        } else {
            for issue in issues {
                let _ = writeln!(&mut text, "  issue: {issue}");
            }
        }
    }
    text.trim_end().to_string()
}

fn plugin_inventory(
    paths: &ConfigPaths,
    resources: &LoadedResources,
) -> Result<Vec<LoadedItem<PluginSpec>>> {
    ensure_workspace_dirs(paths)?;
    let plugins_dir = paths.workspace_config_dir.join("resources/plugins");
    fs::create_dir_all(&plugins_dir)?;
    let workspace_plugin_path = plugins_dir.join("workspace.yaml");
    if !workspace_plugin_path.exists() {
        fs::write(&workspace_plugin_path, default_plugin_contents())?;
    }

    let mut inventory = resources.plugins.clone();
    if !inventory
        .iter()
        .any(|plugin| plugin.value.id == "workspace")
    {
        inventory.push(LoadedItem {
            value: serde_yaml::from_str(&fs::read_to_string(&workspace_plugin_path)?)?,
            source_info: SourceInfo {
                path: workspace_plugin_path,
                kind: SourceKind::Workspace,
            },
        });
    }
    inventory.sort_by(|left, right| left.value.id.cmp(&right.value.id));
    Ok(inventory)
}

fn describe_plugin(
    state: &mut AppState,
    session_store: &SessionStore,
    inventory: &[LoadedItem<PluginSpec>],
    plugin_id: &str,
) -> Result<()> {
    let Some(plugin) = inventory.iter().find(|plugin| plugin.value.id == plugin_id) else {
        return emit_system(
            state,
            session_store,
            format!("Unknown plugin `{plugin_id}`."),
        );
    };
    let mut text = String::new();
    let _ = writeln!(&mut text, "Plugin {}", plugin.value.id);
    let _ = writeln!(&mut text, "Name: {}", plugin.value.display_name);
    let _ = writeln!(&mut text, "Status: {}", plugin_status(&plugin.value));
    let _ = writeln!(
        &mut text,
        "Source: {} ({})",
        source_kind_label(plugin.source_info.kind),
        plugin.source_info.path.display()
    );
    let description = plugin_description(&plugin.value);
    if !description.is_empty() {
        let _ = writeln!(&mut text, "Description: {description}");
    }
    let _ = writeln!(&mut text, "Counts: {}", format_plugin_counts(&plugin.value));
    if !plugin.value.commands.is_empty() {
        let commands = plugin
            .value
            .commands
            .iter()
            .map(|command| command.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(&mut text, "Commands: {commands}");
    }
    if !plugin.value.skills.is_empty() {
        let _ = writeln!(&mut text, "Skills: {}", plugin.value.skills.join(", "));
    }
    if !plugin.value.agents.is_empty() {
        let ids = plugin
            .value
            .agents
            .iter()
            .map(|agent| agent.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(&mut text, "Agents: {ids}");
    }
    if !plugin.value.mcp_servers.is_empty() {
        let ids = plugin
            .value
            .mcp_servers
            .iter()
            .map(|server| server.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(&mut text, "MCP servers: {ids}");
    }
    if !plugin.value.lsp_servers.is_empty() {
        let ids = plugin
            .value
            .lsp_servers
            .iter()
            .map(|server| server.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(&mut text, "LSP servers: {ids}");
    }
    emit_system(state, session_store, text)
}

fn render_plugin_listing(inventory: &[LoadedItem<PluginSpec>]) -> String {
    if inventory.is_empty() {
        return "Plugins:\n<none>".to_string();
    }
    let mut text = String::from("Plugins:\n");
    for plugin in inventory {
        let description = plugin_description(&plugin.value);
        let details = if description.is_empty() {
            format_plugin_counts(&plugin.value)
        } else {
            format!("{description} • {}", format_plugin_counts(&plugin.value))
        };
        let _ = writeln!(
            &mut text,
            "- {} [{}] source={} path={} • {}",
            plugin.value.id,
            plugin_status(&plugin.value),
            source_kind_label(plugin.source_info.kind),
            plugin.source_info.path.display(),
            details
        );
    }
    text
}

fn validate_plugin(plugin: &LoadedItem<PluginSpec>) -> Vec<String> {
    let mut issues = Vec::new();
    if plugin.value.id.trim().is_empty() {
        issues.push("plugin id must not be empty".to_string());
    }
    if plugin.value.display_name.trim().is_empty() {
        issues.push("display_name must not be empty".to_string());
    }
    collect_duplicates(
        plugin
            .value
            .commands
            .iter()
            .map(|command| command.name.as_str()),
        "command",
        &mut issues,
    );
    collect_duplicates(
        plugin.value.skills.iter().map(|skill| skill.as_str()),
        "skill",
        &mut issues,
    );
    collect_duplicates(
        plugin.value.agents.iter().map(|agent| agent.id.as_str()),
        "agent",
        &mut issues,
    );
    collect_duplicates(
        plugin
            .value
            .mcp_servers
            .iter()
            .map(|server| server.id.as_str()),
        "mcp server",
        &mut issues,
    );
    collect_duplicates(
        plugin
            .value
            .lsp_servers
            .iter()
            .map(|server| server.id.as_str()),
        "lsp server",
        &mut issues,
    );
    if is_disabled_placeholder(&plugin.value)
        && (!plugin.value.commands.is_empty()
            || !plugin.value.skills.is_empty()
            || !plugin.value.agents.is_empty()
            || !plugin.value.mcp_servers.is_empty()
            || !plugin.value.lsp_servers.is_empty())
    {
        issues.push(
            "disabled placeholder should not retain commands, skills, agents, MCP servers, or LSP servers"
                .to_string(),
        );
    }
    issues
}

fn collect_duplicates<'a, I>(values: I, label: &str, issues: &mut Vec<String>)
where
    I: IntoIterator<Item = &'a str>,
{
    let mut seen = std::collections::BTreeSet::new();
    let mut duplicates = std::collections::BTreeSet::new();
    for value in values {
        let normalized = value.trim();
        if normalized.is_empty() {
            duplicates.insert("<empty>".to_string());
            continue;
        }
        if !seen.insert(normalized.to_string()) {
            duplicates.insert(normalized.to_string());
        }
    }
    for duplicate in duplicates {
        issues.push(format!("duplicate {label} `{duplicate}`"));
    }
}

fn open_named_plugin_file(
    state: &mut AppState,
    session_store: &SessionStore,
    inventory: &[LoadedItem<PluginSpec>],
    plugin_id: &str,
) -> Result<()> {
    let Some(plugin) = inventory.iter().find(|plugin| plugin.value.id == plugin_id) else {
        return emit_system(
            state,
            session_store,
            format!("Unknown plugin `{plugin_id}`."),
        );
    };
    open_plugin_file(state, session_store, &plugin.source_info.path)
}

fn open_plugin_file(state: &mut AppState, session_store: &SessionStore, path: &Path) -> Result<()> {
    match open_text_file_in_editor(path) {
        Ok(status) => emit_system(state, session_store, status),
        Err(error) => emit_system(
            state,
            session_store,
            format!(
                "Could not open plugin manifest in an editor: {error}\nPath: {}",
                path.display()
            ),
        ),
    }
}
