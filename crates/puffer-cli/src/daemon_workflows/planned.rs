use anyhow::{bail, Result};
use puffer_subscriptions::{
    connector_workflow_trigger_supported, suggested_connection_slug, ConnectorTemplate,
    SubscriberManifestRoots,
};

/// Resolves the connector template for a planned workflow binding connection.
pub(super) fn resolve_planned_binding_template(
    roots: &SubscriberManifestRoots,
    connectors: &[ConnectorTemplate],
    connection_slug: &str,
    connector_slug: Option<&str>,
) -> Result<(String, ConnectorTemplate)> {
    if let Some(connector_slug) = connector_slug
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let template = connectors
            .iter()
            .find(|template| template.slug == connector_slug)
            .ok_or_else(|| anyhow::anyhow!("connector `{connector_slug}` not found"))?;
        if !connector_workflow_trigger_supported(roots, template) {
            bail!("connector `{connector_slug}` cannot produce workflow trigger events");
        }
        return Ok((connector_slug.to_string(), template.clone()));
    }

    let matches = connectors
        .iter()
        .filter(|template| {
            suggested_connection_slug(&template.slug) == connection_slug
                && connector_workflow_trigger_supported(roots, template)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [template] => Ok((template.slug.clone(), (*template).clone())),
        [] => bail!(
            "missing connector_slug for new workflow connection `{connection_slug}`; no trigger-ready connector suggests that connection name"
        ),
        _ => bail!(
            "new workflow connection `{connection_slug}` is ambiguous; provide connector_slug"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn planned_binding_infers_suggested_connection_connector() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let mut template = connector_template("email");
        template.command = vec!["puffer-subscriber-email".to_string()];

        let (connector_slug, _) =
            resolve_planned_binding_template(&roots, &[template], "email", None).unwrap();

        assert_eq!(connector_slug, "email");
    }

    #[test]
    fn planned_binding_uses_explicit_connector_for_custom_connection_name() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let mut template = connector_template("email");
        template.command = vec!["puffer-subscriber-email".to_string()];

        let (connector_slug, _) =
            resolve_planned_binding_template(&roots, &[template], "team-mail", Some("email"))
                .unwrap();

        assert_eq!(connector_slug, "email");
    }

    fn connector_template(slug: &str) -> ConnectorTemplate {
        ConnectorTemplate {
            slug: slug.to_string(),
            description: String::new(),
            skill: String::new(),
            binary: String::new(),
            command: Vec::new(),
            requires_auth: true,
            can_subscribe: true,
            can_proxy_agent: false,
            subscriber: None,
            output_schema: json!({}),
            actions: BTreeMap::new(),
        }
    }
}
