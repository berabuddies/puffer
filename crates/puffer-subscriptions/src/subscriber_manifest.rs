//! Subscriber manifest discovery and per-connection instantiation.

use crate::{ConnectionRecord, ConnectorTemplate};
use anyhow::Result;
use puffer_subscriber_runtime::{Manifest, StateSpec};
use std::path::{Path, PathBuf};

/// Directory roots searched for subscriber manifests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriberManifestRoots {
    /// Workspace `.puffer` directory.
    pub workspace_config_dir: PathBuf,
    /// User-level `.puffer` directory.
    pub user_config_dir: PathBuf,
    /// Bundled resources directory.
    pub builtin_resources_dir: PathBuf,
}

impl SubscriberManifestRoots {
    /// Creates a subscriber manifest search root set.
    pub fn new(
        workspace_config_dir: impl Into<PathBuf>,
        user_config_dir: impl Into<PathBuf>,
        builtin_resources_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            workspace_config_dir: workspace_config_dir.into(),
            user_config_dir: user_config_dir.into(),
            builtin_resources_dir: builtin_resources_dir.into(),
        }
    }
}

/// Finds a subscriber manifest directory for `topic`, searching workspace,
/// user, then bundled resources.
pub fn find_subscriber_manifest(roots: &SubscriberManifestRoots, topic: &str) -> Option<PathBuf> {
    [
        roots.workspace_config_dir.join("subscribers").join(topic),
        roots.user_config_dir.join("subscribers").join(topic),
        roots.builtin_resources_dir.join("subscribers").join(topic),
    ]
    .into_iter()
    .find(|dir| dir.join("manifest.toml").exists())
}

/// Loads a direct subscriber manifest for `topic`, if one exists.
pub fn direct_subscriber_manifest(
    roots: &SubscriberManifestRoots,
    topic: &str,
) -> Result<Option<Manifest>> {
    let Some(dir) = find_subscriber_manifest(roots, topic) else {
        return Ok(None);
    };
    Ok(Some(Manifest::load(dir)?))
}

/// Returns whether a connection has any manifest-backed event source.
pub fn connection_subscriber_manifest_exists(
    roots: &SubscriberManifestRoots,
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
) -> bool {
    connection_subscriber_source(roots, connection, template).is_some()
}

/// Returns whether a connector template can start workflow trigger events.
pub fn connector_workflow_trigger_supported(
    roots: &SubscriberManifestRoots,
    template: &ConnectorTemplate,
) -> bool {
    (template.can_subscribe && template.command_argv().is_some())
        || template.subscriber.as_ref().is_some_and(|subscriber| {
            find_subscriber_manifest(roots, &subscriber.manifest_slug).is_some()
        })
        || find_subscriber_manifest(roots, &template.slug).is_some()
}

/// Returns stable runtime/source hint labels for connector selection UIs.
pub fn connector_runtime_hints(
    roots: &SubscriberManifestRoots,
    template: &ConnectorTemplate,
) -> Vec<String> {
    let mut hints = Vec::new();
    if connector_subscriber_manifest_supported(roots, template) {
        hints.push("subscriber");
    }
    if template.command_argv().is_some() {
        hints.push("command");
    }
    if template.binary.starts_with("puffer internal-tool") {
        hints.push("internal-tool");
    }
    if serve_configured_connector(&template.slug) {
        hints.push("serve");
    }
    if hints.is_empty() {
        hints.push("connector");
    }
    hints.into_iter().map(str::to_string).collect()
}

/// Returns whether a concrete connection can start workflow trigger events.
pub fn connection_workflow_trigger_supported(
    roots: &SubscriberManifestRoots,
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
) -> bool {
    (template.can_subscribe && template.command_argv().is_some())
        || connection_subscriber_manifest_exists(roots, connection, template)
}

fn connector_subscriber_manifest_supported(
    roots: &SubscriberManifestRoots,
    template: &ConnectorTemplate,
) -> bool {
    template.subscriber.as_ref().is_some_and(|subscriber| {
        find_subscriber_manifest(roots, &subscriber.manifest_slug).is_some()
    }) || find_subscriber_manifest(roots, &template.slug).is_some()
}

fn serve_configured_connector(slug: &str) -> bool {
    matches!(
        slug,
        "telegram-bot"
            | "discord-bot"
            | "matrix-bot"
            | "alertmanager-webhook"
            | "asana-webhook"
            | "datadog-webhook"
            | "github-webhook"
            | "grafana-webhook"
            | "gitlab-webhook"
            | "jira-webhook"
            | "linear-webhook"
            | "pagerduty-webhook"
            | "sentry-webhook"
            | "shopify-webhook"
            | "stripe-webhook"
            | "trello-webhook"
            | "webhook"
    )
}

/// Loads the subscriber manifest for a connection, instantiating shared
/// connector metadata as a connection-scoped subscriber when needed.
pub fn connection_subscriber_manifest(
    roots: &SubscriberManifestRoots,
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
) -> Result<Option<Manifest>> {
    let Some(source) = connection_subscriber_source(roots, connection, template) else {
        return Ok(None);
    };
    let mut manifest = Manifest::load(&source.dir)?;
    if source.instantiate {
        instantiate_manifest(roots, connection, template, &mut manifest);
    }
    Ok(Some(manifest))
}

fn connection_subscriber_source(
    roots: &SubscriberManifestRoots,
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
) -> Option<SubscriberManifestSource> {
    if let Some(dir) = find_user_subscriber_manifest(roots, &connection.slug) {
        return Some(SubscriberManifestSource {
            dir,
            instantiate: false,
        });
    }
    if let Some(subscriber) = &template.subscriber {
        if let Some(dir) = find_subscriber_manifest(roots, &subscriber.manifest_slug) {
            return Some(SubscriberManifestSource {
                dir,
                instantiate: true,
            });
        }
    }
    if let Some(dir) = find_builtin_subscriber_manifest(roots, &connection.slug) {
        return Some(SubscriberManifestSource {
            dir,
            instantiate: false,
        });
    }
    find_subscriber_manifest(roots, &connection.connector_slug).map(|dir| {
        SubscriberManifestSource {
            dir,
            instantiate: false,
        }
    })
}

fn find_user_subscriber_manifest(roots: &SubscriberManifestRoots, topic: &str) -> Option<PathBuf> {
    [
        roots.workspace_config_dir.join("subscribers").join(topic),
        roots.user_config_dir.join("subscribers").join(topic),
    ]
    .into_iter()
    .find(|dir| dir.join("manifest.toml").exists())
}

fn find_builtin_subscriber_manifest(
    roots: &SubscriberManifestRoots,
    topic: &str,
) -> Option<PathBuf> {
    let dir = roots.builtin_resources_dir.join("subscribers").join(topic);
    dir.join("manifest.toml").exists().then_some(dir)
}

fn instantiate_manifest(
    roots: &SubscriberManifestRoots,
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
    manifest: &mut Manifest,
) {
    manifest.spec.id = connection.slug.clone();
    manifest.spec.topic = Some(connection.slug.clone());
    if let Some(display_name) = template
        .subscriber
        .as_ref()
        .and_then(|subscriber| subscriber.display_name.as_deref())
    {
        manifest.spec.display_name = Some(format!("{display_name} ({})", connection.slug));
    } else if let Some(display_name) = manifest.spec.display_name.clone() {
        manifest.spec.display_name = Some(format!("{display_name} ({})", connection.slug));
    }
    if let Some(state_root) = template
        .subscriber
        .as_ref()
        .and_then(|subscriber| subscriber.state_root.as_deref())
    {
        manifest.spec.state = Some(StateSpec {
            dir: instantiated_state_dir(roots, state_root, &connection.slug)
                .to_string_lossy()
                .to_string(),
        });
    }
}

fn instantiated_state_dir(
    roots: &SubscriberManifestRoots,
    state_root: &str,
    connection_slug: &str,
) -> PathBuf {
    let root = Path::new(state_root);
    if root.is_absolute() {
        return root.join(connection_slug);
    }
    roots.user_config_dir.join(root).join(connection_slug)
}

struct SubscriberManifestSource {
    dir: PathBuf,
    instantiate: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConnectionRecord, ConnectorSubscriberTemplate};
    use serde_json::Value;
    use std::collections::BTreeMap;

    #[test]
    fn finds_workspace_manifest_before_builtin_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let workspace = roots.workspace_config_dir.join("subscribers/demo");
        let builtin = roots.builtin_resources_dir.join("subscribers/demo");
        write_manifest(&workspace, "demo", "demo", None);
        write_manifest(&builtin, "demo", "demo", None);

        assert_eq!(find_subscriber_manifest(&roots, "demo").unwrap(), workspace);
    }

    #[test]
    fn instantiates_configured_manifest_for_connection() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let manifest_dir = roots.builtin_resources_dir.join("subscribers/shared");
        write_manifest(&manifest_dir, "shared", "shared", Some("Shared"));
        let connection = ConnectionRecord::authenticated("personal", "connector", "Personal");
        let mut template = template("connector");
        template.subscriber = Some(ConnectorSubscriberTemplate {
            manifest_slug: "shared".to_string(),
            state_root: Some("accounts".to_string()),
            display_name: Some("Connector".to_string()),
        });

        let manifest = connection_subscriber_manifest(&roots, &connection, &template)
            .unwrap()
            .unwrap();

        assert_eq!(manifest.spec.id, "personal");
        assert_eq!(manifest.topic(), "personal");
        assert_eq!(
            manifest.spec.display_name.as_deref(),
            Some("Connector (personal)")
        );
        assert_eq!(
            manifest.spec.state.unwrap().dir,
            roots
                .user_config_dir
                .join("accounts/personal")
                .to_string_lossy()
        );
    }

    #[test]
    fn instantiates_shared_manifest_when_connection_slug_matches_builtin_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let manifest_dir = roots
            .builtin_resources_dir
            .join("subscribers/telegram-user");
        write_manifest(
            &manifest_dir,
            "telegram-user",
            "telegram-user",
            Some("Telegram"),
        );
        let connection =
            ConnectionRecord::authenticated("telegram-user", "telegram-login", "Telegram");
        let mut template = template("telegram-login");
        template.subscriber = Some(ConnectorSubscriberTemplate {
            manifest_slug: "telegram-user".to_string(),
            state_root: Some("telegram-accounts".to_string()),
            display_name: Some("Telegram".to_string()),
        });

        let manifest = connection_subscriber_manifest(&roots, &connection, &template)
            .unwrap()
            .unwrap();

        assert_eq!(manifest.spec.id, "telegram-user");
        assert_eq!(manifest.topic(), "telegram-user");
        assert_eq!(
            manifest.spec.state.unwrap().dir,
            roots
                .user_config_dir
                .join("telegram-accounts/telegram-user")
                .to_string_lossy()
        );
    }

    #[test]
    fn preserves_legacy_connector_manifest_identity() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let manifest_dir = roots.builtin_resources_dir.join("subscribers/email");
        write_manifest(&manifest_dir, "email", "email", Some("Email"));
        let connection = ConnectionRecord::authenticated("work-email", "email", "Work email");
        let template = template("email");

        let manifest = connection_subscriber_manifest(&roots, &connection, &template)
            .unwrap()
            .unwrap();

        assert_eq!(manifest.spec.id, "email");
        assert_eq!(manifest.topic(), "email");
    }

    #[test]
    fn connector_workflow_trigger_support_accepts_command_or_manifest_sources() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let mut command_template = template("custom-feed");
        command_template.command = vec!["custom-feed".to_string()];
        let manifest_template = template("email");
        write_manifest(
            &roots.builtin_resources_dir.join("subscribers/email"),
            "email",
            "email",
            Some("Email"),
        );

        assert!(connector_workflow_trigger_supported(
            &roots,
            &command_template
        ));
        assert!(connector_workflow_trigger_supported(
            &roots,
            &manifest_template
        ));
    }

    #[test]
    fn connector_workflow_trigger_support_rejects_commandless_placeholders() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let template = template("telegram-bot");

        assert!(!connector_workflow_trigger_supported(&roots, &template));
    }

    #[test]
    fn connection_workflow_trigger_support_accepts_shared_subscriber() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        write_manifest(
            &roots.builtin_resources_dir.join("subscribers/shared-login"),
            "shared-login",
            "shared-login",
            Some("Shared"),
        );
        let connection = ConnectionRecord::authenticated("personal", "shared", "Personal");
        let mut template = template("shared");
        template.subscriber = Some(ConnectorSubscriberTemplate {
            manifest_slug: "shared-login".to_string(),
            state_root: Some("shared-accounts".to_string()),
            display_name: Some("Shared".to_string()),
        });

        assert!(connection_workflow_trigger_supported(
            &roots,
            &connection,
            &template
        ));
    }

    #[test]
    fn connector_runtime_hints_report_subscriber_and_internal_sources() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        write_manifest(
            &roots
                .builtin_resources_dir
                .join("subscribers/telegram-user"),
            "telegram-user",
            "telegram-user",
            Some("Telegram"),
        );
        let mut template = template("telegram-login");
        template.binary = "puffer internal-tool telegram".to_string();
        template.subscriber = Some(ConnectorSubscriberTemplate {
            manifest_slug: "telegram-user".to_string(),
            state_root: Some("telegram-accounts".to_string()),
            display_name: Some("Telegram".to_string()),
        });

        assert_eq!(
            connector_runtime_hints(&roots, &template),
            vec!["subscriber", "internal-tool"]
        );
    }

    #[test]
    fn connector_runtime_hints_report_serve_and_command_sources() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let serve_template = template("discord-bot");
        let mut command_template = template("custom-feed");
        command_template.command = vec!["custom-feed".to_string(), "subscribe".to_string()];

        assert_eq!(
            connector_runtime_hints(&roots, &serve_template),
            vec!["serve"]
        );
        assert_eq!(
            connector_runtime_hints(&roots, &command_template),
            vec!["command"]
        );
    }

    fn roots(root: &Path) -> SubscriberManifestRoots {
        SubscriberManifestRoots::new(
            root.join("workspace/.puffer"),
            root.join("home/.puffer"),
            root.join("resources"),
        )
    }

    fn write_manifest(dir: &Path, id: &str, topic: &str, display_name: Option<&str>) {
        std::fs::create_dir_all(dir).unwrap();
        let display_name = display_name
            .map(|name| format!("display_name = \"{name}\"\n"))
            .unwrap_or_default();
        std::fs::write(
            dir.join("manifest.toml"),
            format!(
                "manifest_version = 1\nid = \"{id}\"\nkind = \"subscriber\"\ntopic = \"{topic}\"\n{display_name}[run]\ncmd = [\"true\"]\n"
            ),
        )
        .unwrap();
    }

    fn template(slug: &str) -> ConnectorTemplate {
        ConnectorTemplate {
            slug: slug.to_string(),
            description: String::new(),
            skill: String::new(),
            binary: String::new(),
            command: Vec::new(),
            requires_auth: false,
            can_subscribe: true,
            can_proxy_agent: false,
            subscriber: None,
            output_schema: Value::Null,
            actions: BTreeMap::new(),
        }
    }
}
