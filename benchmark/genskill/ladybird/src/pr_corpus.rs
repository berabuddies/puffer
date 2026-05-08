//! On-disk corpus loader and validator.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One PR's metadata as stored in pr_corpus/<id>/meta.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrMeta {
    /// GitHub pull request number.
    pub pr_number: u64,
    /// Canonical GitHub pull request URL.
    pub url: String,
    /// Pull request title.
    pub title: String,
    /// Merge timestamp recorded by GitHub.
    pub merged_at: String,
    /// Commit SHA to reset the replay checkout to before applying tests.
    pub base_commit: String,
    /// Merge commit SHA for the original accepted fix.
    pub merge_commit: String,
    /// Coarse implementation area used for corpus balancing.
    pub area: String,
    /// Files changed by the original pull request.
    pub files_changed: Vec<String>,
    /// Task prompt shown to the expert or replay agent.
    pub task_prompt: String,
}

/// One loaded corpus entry.
#[derive(Debug)]
pub struct CorpusEntry {
    /// Directory id, matching pr_corpus/<id>.
    pub id: String,
    /// Parsed metadata for this PR.
    pub meta: PrMeta,
    /// Absolute or relative path to the corpus entry directory.
    #[allow(dead_code)]
    pub dir: PathBuf,
}

/// Loads and validates every pr_corpus/pr-* subdirectory.
pub fn load_corpus(corpus_dir: &Path) -> Result<Vec<CorpusEntry>> {
    let mut entries = Vec::new();
    for dent in std::fs::read_dir(corpus_dir)
        .with_context(|| format!("reading {}", corpus_dir.display()))?
    {
        let dent = dent?;
        let path = dent.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.starts_with("pr-") => n.to_string(),
            _ => continue,
        };
        let meta_path = path.join("meta.json");
        let meta_text = std::fs::read_to_string(&meta_path)
            .with_context(|| format!("reading {}", meta_path.display()))?;
        let meta: PrMeta = serde_json::from_str(&meta_text)
            .with_context(|| format!("parsing {}", meta_path.display()))?;
        for required in ["reference_fix.patch", "tests"] {
            if !path.join(required).exists() {
                return Err(anyhow!("{}/{} missing", path.display(), required));
            }
        }
        entries.push(CorpusEntry {
            id: name,
            meta,
            dir: path,
        });
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    if entries.is_empty() {
        return Err(anyhow!("no pr-* entries under {}", corpus_dir.display()));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_pr(root: &Path, id: &str, body: &str) {
        let dir = root.join(id);
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(dir.join("meta.json"), body).unwrap();
        fs::write(dir.join("reference_fix.patch"), "").unwrap();
    }

    #[test]
    fn load_valid_corpus() {
        let tmp = TempDir::new().unwrap();
        write_pr(
            tmp.path(),
            "pr-1",
            r#"{
            "pr_number": 1, "url": "https://example.com/1", "title": "x",
            "merged_at": "2025-01-01T00:00:00Z", "base_commit": "abc",
            "merge_commit": "def", "area": "libweb-css",
            "files_changed": [], "task_prompt": "do x"
        }"#,
        );
        let entries = load_corpus(tmp.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "pr-1");
        assert_eq!(entries[0].meta.pr_number, 1);
    }

    #[test]
    fn load_rejects_missing_reference_fix() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("pr-1");
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(
            dir.join("meta.json"),
            r#"{
            "pr_number":1,"url":"x","title":"x","merged_at":"x",
            "base_commit":"x","merge_commit":"x","area":"x",
            "files_changed":[],"task_prompt":"x"
        }"#,
        )
        .unwrap();
        assert!(load_corpus(tmp.path()).is_err());
    }

    #[test]
    fn load_rejects_empty_dir() {
        let tmp = TempDir::new().unwrap();
        assert!(load_corpus(tmp.path()).is_err());
    }
}
