use crate::catalog::{builtin_connector_template, builtin_connector_templates, ConnectorTemplate};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

/// Errors returned by [`ConnectorCatalogStore`].
#[derive(Debug, Error)]
pub enum ConnectorCatalogStoreError {
    /// I/O failed while reading or writing connector state.
    #[error("connector catalog io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON failed to parse or encode.
    #[error("connector catalog json error: {0}")]
    Json(#[from] serde_json::Error),
    /// Connector input is invalid.
    #[error("invalid connector: {0}")]
    Invalid(String),
    /// Connector was not found.
    #[error("connector `{0}` not found")]
    NotFound(String),
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct ConnectorCatalogFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    connectors: Vec<ConnectorTemplate>,
}

/// File-backed store for user-defined connector templates.
pub struct ConnectorCatalogStore {
    path: PathBuf,
    inner: Mutex<ConnectorCatalogFile>,
}

impl ConnectorCatalogStore {
    /// Loads a connector catalog store. Missing files are treated as empty.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, ConnectorCatalogStoreError> {
        let path = path.into();
        let inner = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            if raw.trim().is_empty() {
                ConnectorCatalogFile::default()
            } else {
                serde_json::from_str(&raw)?
            }
        } else {
            ConnectorCatalogFile::default()
        };
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    /// Returns all connector templates, with user definitions overriding
    /// built-ins that share the same slug.
    pub fn list_with_builtins(&self) -> Vec<ConnectorTemplate> {
        let mut by_slug = builtin_connector_templates()
            .into_iter()
            .map(|template| (template.slug.clone(), template))
            .collect::<std::collections::BTreeMap<_, _>>();
        for template in self.inner.lock().unwrap().connectors.clone() {
            by_slug.insert(template.slug.clone(), template);
        }
        by_slug.into_values().collect()
    }

    /// Returns user-defined connector templates only.
    pub fn list_user(&self) -> Vec<ConnectorTemplate> {
        let mut list = self.inner.lock().unwrap().connectors.clone();
        list.sort_by(|a, b| a.slug.cmp(&b.slug));
        list
    }

    /// Returns one connector template, checking user definitions before built-ins.
    pub fn get(&self, slug: &str) -> Option<ConnectorTemplate> {
        self.inner
            .lock()
            .unwrap()
            .connectors
            .iter()
            .find(|template| template.slug == slug)
            .cloned()
            .or_else(|| builtin_connector_template(slug))
    }

    /// Registers or replaces one user-defined connector template.
    pub fn upsert(
        &self,
        template: ConnectorTemplate,
    ) -> Result<ConnectorTemplate, ConnectorCatalogStoreError> {
        validate_template(&template)?;
        let mut guard = self.inner.lock().unwrap();
        guard
            .connectors
            .retain(|existing| existing.slug != template.slug);
        guard.connectors.push(template.clone());
        guard.connectors.sort_by(|a, b| a.slug.cmp(&b.slug));
        write_atomic(&self.path, &*guard)?;
        Ok(template)
    }

    /// Deletes one user-defined connector template.
    pub fn delete(&self, slug: &str) -> Result<(), ConnectorCatalogStoreError> {
        let mut guard = self.inner.lock().unwrap();
        let before = guard.connectors.len();
        guard.connectors.retain(|template| template.slug != slug);
        if guard.connectors.len() == before {
            return Err(ConnectorCatalogStoreError::NotFound(slug.to_string()));
        }
        write_atomic(&self.path, &*guard)
    }
}

fn validate_template(template: &ConnectorTemplate) -> Result<(), ConnectorCatalogStoreError> {
    validate_slug("connector slug", &template.slug)?;
    if template.description.trim().is_empty() {
        return Err(ConnectorCatalogStoreError::Invalid(
            "connector description must not be empty".to_string(),
        ));
    }
    if template.skill.trim().is_empty() {
        return Err(ConnectorCatalogStoreError::Invalid(
            "connector skill must not be empty".to_string(),
        ));
    }
    if template.binary.trim().is_empty() && template.command.is_empty() {
        return Err(ConnectorCatalogStoreError::Invalid(
            "connector binary or command must not be empty".to_string(),
        ));
    }
    if template.command.iter().any(|part| part.trim().is_empty()) {
        return Err(ConnectorCatalogStoreError::Invalid(
            "connector command entries must not be empty".to_string(),
        ));
    }
    for (slug, action) in &template.actions {
        validate_action_slug("connector action slug", slug)?;
        if action.slug != *slug {
            return Err(ConnectorCatalogStoreError::Invalid(format!(
                "connector action map key `{slug}` must match action slug `{}`",
                action.slug
            )));
        }
        if action.permission.category.trim().is_empty() {
            return Err(ConnectorCatalogStoreError::Invalid(format!(
                "connector action `{slug}` permission category must not be empty"
            )));
        }
    }
    Ok(())
}

fn validate_action_slug(label: &str, slug: &str) -> Result<(), ConnectorCatalogStoreError> {
    if slug.is_empty()
        || !slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return Err(ConnectorCatalogStoreError::Invalid(format!(
            "{label} must be non-empty ASCII using lowercase letters, digits, '-' or '_'"
        )));
    }
    Ok(())
}

fn validate_slug(label: &str, slug: &str) -> Result<(), ConnectorCatalogStoreError> {
    if slug.is_empty()
        || !slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(ConnectorCatalogStoreError::Invalid(format!(
            "{label} must be non-empty kebab-case ASCII"
        )));
    }
    Ok(())
}

fn write_atomic(
    path: &Path,
    store: &ConnectorCatalogFile,
) -> Result<(), ConnectorCatalogStoreError> {
    let tmp = path.with_extension("tmp");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&tmp, serde_json::to_vec_pretty(store)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_visible_without_user_file() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConnectorCatalogStore::load(temp.path().join("connectors.json")).unwrap();

        assert!(store.get("telegram-login").is_some());
        assert!(store
            .list_with_builtins()
            .iter()
            .any(|template| template.slug == "slack-bot"));
    }

    #[test]
    fn user_template_overrides_builtin() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConnectorCatalogStore::load(temp.path().join("connectors.json")).unwrap();
        let mut template = builtin_connector_template("email").unwrap();
        template.description = "Custom email".to_string();
        store.upsert(template).unwrap();

        let reopened = ConnectorCatalogStore::load(temp.path().join("connectors.json")).unwrap();
        assert_eq!(reopened.get("email").unwrap().description, "Custom email");
    }
}
