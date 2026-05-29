use super::*;
use puffer_resources::{LoadedItem, SourceInfo, SourceKind};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn workspace_builtin_tool_resources() -> LoadedResources {
    let tools_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/tools");
    let mut tool_paths = fs::read_dir(&tools_dir)
        .expect("read builtin tool dir")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("yaml"))
        .collect::<Vec<_>>();
    tool_paths.sort();

    let tools = tool_paths
        .into_iter()
        .map(|path| {
            let spec = serde_yaml::from_str::<ToolSpec>(
                &fs::read_to_string(&path).expect("read builtin tool resource"),
            )
            .expect("parse builtin tool resource");
            LoadedItem {
                value: spec,
                source_info: SourceInfo {
                    path,
                    kind: SourceKind::Builtin,
                },
            }
        })
        .collect();

    LoadedResources {
        tools,
        ..LoadedResources::default()
    }
}

fn workspace_builtin_internal_tool_resources() -> LoadedResources {
    let tools_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/internal_tools");
    let mut tool_paths = fs::read_dir(&tools_dir)
        .expect("read builtin internal tool dir")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("yaml"))
        .collect::<Vec<_>>();
    tool_paths.sort();

    let internal_tools = tool_paths
        .into_iter()
        .map(|path| {
            let spec = serde_yaml::from_str::<ToolSpec>(
                &fs::read_to_string(&path).expect("read builtin internal tool resource"),
            )
            .expect("parse builtin internal tool resource");
            LoadedItem {
                value: spec,
                source_info: SourceInfo {
                    path,
                    kind: SourceKind::Builtin,
                },
            }
        })
        .collect();

    LoadedResources {
        internal_tools,
        ..LoadedResources::default()
    }
}

fn browser_tool_spec() -> ToolSpec {
    ToolSpec {
        id: "Browser".to_string(),
        name: "Browser".to_string(),
        description: "Control browser".to_string(),
        handler: "runtime:browser".to_string(),
        aliases: vec!["browser".to_string()],
        handler_args: Vec::new(),
        approval_policy: Some("auto".to_string()),
        sandbox_policy: Some("workspace-write".to_string()),
        shared_lib: None,
        enabled_if: None,
        input_schema: None,
        metadata: Default::default(),
        display: Default::default(),
    }
}

#[test]
fn puffer_no_browser_disables_builtin_browser_tool() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PUFFER_NO_BROWSER", "1");
    let resources = LoadedResources {
        internal_tools: vec![LoadedItem {
            value: browser_tool_spec(),
            source_info: SourceInfo {
                path: PathBuf::from("browser.yaml"),
                kind: SourceKind::Builtin,
            },
        }],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    std::env::remove_var("PUFFER_NO_BROWSER");

    assert!(registry.tool("Browser").is_none());
    assert!(registry.tool("browser").is_none());
    assert!(registry.internal_tool("Browser").is_none());
    assert!(registry.internal_tool("browser").is_none());
    assert!(!registry
        .definitions()
        .any(|definition| definition.id == "Browser"));
}

#[test]
fn puffer_browser_internal_tool_is_not_model_visible() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("PUFFER_NO_BROWSER");
    let resources = LoadedResources {
        internal_tools: vec![LoadedItem {
            value: browser_tool_spec(),
            source_info: SourceInfo {
                path: PathBuf::from("browser.yaml"),
                kind: SourceKind::Builtin,
            },
        }],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);

    assert!(registry.tool("Browser").is_none());
    assert!(registry.tool("browser").is_none());
    assert!(registry.internal_tool("Browser").is_some());
    assert!(registry.internal_tool("browser").is_some());
    assert!(!registry
        .definitions()
        .any(|definition| definition.id == "Browser"));
}

#[test]
fn workspace_builtin_tool_resources_are_registerable() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("PUFFER_NO_BROWSER");
    let resources = workspace_builtin_tool_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let expected = resources
        .tools
        .iter()
        .filter_map(|item| definition_from_spec(&item.value))
        .map(|definition| definition.id)
        .collect::<BTreeSet<_>>();
    let registered = registry
        .definitions()
        .map(|definition| definition.id.clone())
        .collect::<BTreeSet<_>>();
    let missing = expected
        .difference(&registered)
        .cloned()
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "builtin resources produced unsupported tool registrations: {missing:?}"
    );
    assert!(registry.definition("BrowserAction").is_some());
    assert!(registry.definition("HttpRequest").is_some());
    assert!(registry.definition("McpToolCall").is_some());
    assert!(registry.definition("SlackAction").is_some());
    assert!(registry.definition("TaskFlow").is_some());
    assert!(registry.definition("Sleep").is_some());
    assert!(!registered.contains("Browser"));
    assert!(!registered.contains("Email"));
    assert!(!registered.contains("Lark"));
    assert!(!registered.contains("Slack"));
    assert!(!registered.contains("Telegram"));
    assert!(registry.definition("EmailConfigure").is_none());
    assert!(registry.definition("TelegramLoginStart").is_none());
    assert!(registry.definition("TelegramLoginSubmitCode").is_none());
    assert!(registry.definition("TelegramLoginSubmitPassword").is_none());
}

#[test]
fn workspace_builtin_internal_tool_resources_are_registerable() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("PUFFER_NO_BROWSER");
    let resources = workspace_builtin_internal_tool_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let expected = resources
        .internal_tools
        .iter()
        .filter_map(|item| definition_from_spec(&item.value))
        .map(|definition| definition.id)
        .collect::<BTreeSet<_>>();
    let registered = registry
        .internal_tools()
        .map(|tool| tool.definition().id.clone())
        .collect::<BTreeSet<_>>();
    let missing = expected
        .difference(&registered)
        .cloned()
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "builtin resources produced unsupported internal tool registrations: {missing:?}"
    );
    assert!(registry.internal_definition("Browser").is_some());
    assert!(registry.internal_definition("Email").is_some());
    assert!(registry.internal_definition("Lark").is_some());
    assert!(registry.internal_definition("Slack").is_some());
    assert!(registry.internal_definition("Telegram").is_some());
    assert!(registry.definition("Browser").is_none());
    assert!(registry.definition("Email").is_none());
    assert!(registry.definition("Lark").is_none());
    assert!(registry.definition("Slack").is_none());
    assert!(registry.definition("Telegram").is_none());
}
