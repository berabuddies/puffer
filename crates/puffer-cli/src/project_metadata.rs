//! Per-project (folder-keyed) metadata stored alongside session data.
//!
//! Projects aren't first-class entities — they're just the `cwd` folders that
//! sessions live under. But the UI wants tags and a stable place to keep
//! project-level overrides. We persist that as a single JSON map keyed by
//! folder path.

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "project_metadata.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMetadata {
    #[serde(default)]
    pub tags: Vec<String>,
}

pub struct ProjectMetadataStore {
    path: PathBuf,
}

impl ProjectMetadataStore {
    pub fn from_paths(paths: &ConfigPaths) -> Self {
        Self {
            path: paths.user_config_dir.join(FILE_NAME),
        }
    }

    fn load_map(&self) -> Result<BTreeMap<String, ProjectMetadata>> {
        if !self.path.exists() {
            return Ok(BTreeMap::new());
        }
        let bytes =
            fs::read(&self.path).with_context(|| format!("reading {}", self.path.display()))?;
        if bytes.is_empty() {
            return Ok(BTreeMap::new());
        }
        let map: BTreeMap<String, ProjectMetadata> = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing project metadata at {}", self.path.display()))?;
        Ok(map)
    }

    fn save_map(&self, map: &BTreeMap<String, ProjectMetadata>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(map)?;
        fs::write(&self.path, bytes).with_context(|| format!("writing {}", self.path.display()))?;
        Ok(())
    }

    pub fn all(&self) -> Result<BTreeMap<String, ProjectMetadata>> {
        self.load_map()
    }

    pub fn set_tags(&self, folder_path: &Path, tags: Vec<String>) -> Result<ProjectMetadata> {
        let key = folder_path.to_string_lossy().into_owned();
        let mut map = self.load_map()?;
        let mut cleaned: Vec<String> = tags
            .into_iter()
            .map(|tag| tag.trim().to_string())
            .filter(|tag| !tag.is_empty())
            .collect();
        cleaned.sort();
        cleaned.dedup();
        let entry = map.entry(key).or_default();
        entry.tags = cleaned;
        let result = entry.clone();
        self.save_map(&map)?;
        Ok(result)
    }

    pub fn delete(&self, folder_path: &Path) -> Result<()> {
        let key = folder_path.to_string_lossy().into_owned();
        let mut map = self.load_map()?;
        if map.remove(&key).is_some() {
            self.save_map(&map)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(temp: &tempfile::TempDir) -> ConfigPaths {
        ConfigPaths::discover(temp.path())
    }

    #[test]
    fn set_tags_dedupes_and_sorts() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(&temp);
        fs::create_dir_all(&paths.user_config_dir).unwrap();
        let store = ProjectMetadataStore::from_paths(&paths);
        let folder = temp.path().join("proj");
        let meta = store
            .set_tags(
                &folder,
                vec!["b".into(), "a".into(), "b".into(), "  ".into()],
            )
            .unwrap();
        assert_eq!(meta.tags, vec!["a".to_string(), "b".to_string()]);
        let all = store.all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(
            all.values().next().unwrap().tags,
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn delete_removes_only_the_target_entry() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(&temp);
        fs::create_dir_all(&paths.user_config_dir).unwrap();
        let store = ProjectMetadataStore::from_paths(&paths);
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        store.set_tags(&a, vec!["alpha".into()]).unwrap();
        store.set_tags(&b, vec!["beta".into()]).unwrap();
        store.delete(&a).unwrap();
        let all = store.all().unwrap();
        assert!(!all.contains_key(&a.to_string_lossy().into_owned()));
        assert!(all.contains_key(&b.to_string_lossy().into_owned()));
    }
}
