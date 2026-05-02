use anyhow::Result;
use serde_json::{json, Value};
use sha2::Digest;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug, Default)]
pub(super) struct WorkspaceSnapshot {
    files: BTreeMap<String, FileFingerprint>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileFingerprint {
    len: u64,
    modified_ns: u128,
    sha256: String,
}

/// Captures a structural snapshot of workspace file state.
pub(super) fn snapshot_workspace(root: &Path) -> Result<WorkspaceSnapshot> {
    let mut snapshot = WorkspaceSnapshot::default();
    visit_workspace(root, root, &mut snapshot)?;
    Ok(snapshot)
}

/// Returns a structural witness for file changes between two snapshots.
pub(super) fn workspace_change_witness(
    before: &WorkspaceSnapshot,
    after: &WorkspaceSnapshot,
) -> Value {
    let mut created = Vec::new();
    let mut modified = Vec::new();
    let mut removed = Vec::new();
    let mut created_details = Vec::new();
    let mut modified_details = Vec::new();
    let mut removed_details = Vec::new();

    for (path, fingerprint) in &after.files {
        match before.files.get(path) {
            None => {
                created.push(path.clone());
                created_details.push(file_change_detail(path, None, Some(fingerprint)));
            }
            Some(before_fingerprint) if before_fingerprint != fingerprint => {
                modified.push(path.clone());
                modified_details.push(file_change_detail(
                    path,
                    Some(before_fingerprint),
                    Some(fingerprint),
                ));
            }
            Some(_) => {}
        }
    }
    for (path, fingerprint) in &before.files {
        if !after.files.contains_key(path) {
            removed.push(path.clone());
            removed_details.push(file_change_detail(path, Some(fingerprint), None));
        }
    }

    json!({
        "created": created,
        "modified": modified,
        "removed": removed,
        "details": {
            "created": created_details,
            "modified": modified_details,
            "removed": removed_details,
        },
    })
}

/// Returns whether a workspace-change witness contains at least one change.
pub(super) fn witness_has_changes(witness: &Value) -> bool {
    ["created", "modified", "removed"].iter().any(|key| {
        witness
            .get(*key)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    })
}

fn visit_workspace(root: &Path, current: &Path, snapshot: &mut WorkspaceSnapshot) -> Result<()> {
    if skip_path(root, current) {
        return Ok(());
    }
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if skip_path(root, &path) {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            visit_workspace(root, &path, snapshot)?;
        } else if file_type.is_file() {
            if let Some((relative, fingerprint)) = file_fingerprint(root, &path)? {
                snapshot.files.insert(relative, fingerprint);
            }
        }
    }
    Ok(())
}

fn skip_path(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| component.as_os_str() == ".puffer")
}

fn file_fingerprint(root: &Path, path: &Path) -> Result<Option<(String, FileFingerprint)>> {
    let metadata = fs::metadata(path)?;
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let relative = display_path(path.strip_prefix(root).unwrap_or(path));
    Ok(Some((
        relative,
        FileFingerprint {
            len: metadata.len(),
            modified_ns,
            sha256: file_sha256(path)?,
        },
    )))
}

/// Returns a streaming SHA-256 hash for a regular file.
pub(super) fn file_sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        sha2::Digest::update(&mut hasher, &buffer[..read]);
    }
    Ok(format!("{:x}", sha2::Digest::finalize(hasher)))
}

fn file_change_detail(
    path: &str,
    before: Option<&FileFingerprint>,
    after: Option<&FileFingerprint>,
) -> Value {
    json!({
        "path": path,
        "before": before.map(file_fingerprint_value),
        "after": after.map(file_fingerprint_value),
    })
}

fn file_fingerprint_value(fingerprint: &FileFingerprint) -> Value {
    json!({
        "len": fingerprint.len,
        "modified_ns": fingerprint.modified_ns,
        "sha256": fingerprint.sha256,
    })
}

fn display_path(path: &Path) -> String {
    let path = PathBuf::from(path);
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_witness_ignores_runtime_files() {
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("input.txt"), "before").unwrap();
        fs::create_dir_all(workspace.path().join(".puffer/burbot")).unwrap();
        let before = snapshot_workspace(workspace.path()).unwrap();

        fs::write(workspace.path().join("input.txt"), "after").unwrap();
        fs::write(workspace.path().join("artifact.bin"), "new").unwrap();
        fs::write(
            workspace.path().join(".puffer/burbot/trace.jsonl"),
            "runtime",
        )
        .unwrap();
        let after = snapshot_workspace(workspace.path()).unwrap();

        let witness = workspace_change_witness(&before, &after);

        assert!(witness_has_changes(&witness));
        assert_eq!(witness["created"], json!(["artifact.bin"]));
        assert_eq!(witness["modified"], json!(["input.txt"]));
        assert_eq!(witness["removed"], json!([]));
        assert_eq!(
            witness["details"]["created"][0]["path"],
            json!("artifact.bin")
        );
        assert_eq!(
            witness["details"]["modified"][0]["path"],
            json!("input.txt")
        );
        assert!(witness["details"]["created"][0]["after"]["sha256"].is_string());
    }
}
