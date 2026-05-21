use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const DEFAULT_MAX_BYTES: usize = 262_144;
const HARD_MAX_BYTES: u64 = 5 * 1024 * 1024;
const TEXT_SNIFF_BYTES: usize = 8 * 1024;

pub(crate) fn list_dir(params: &Value, allowed_roots: &[PathBuf]) -> Result<Value> {
    let raw = params
        .get("path")
        .and_then(Value::as_str)
        .context("missing path")?;
    let path = validate_path(allowed_roots, raw)?;
    let meta = std::fs::metadata(&path)
        .with_context(|| format!("path does not exist: {}", path.display()))?;
    if !meta.is_dir() {
        bail!("path is not a directory: {}", path.display());
    }

    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in
        std::fs::read_dir(&path).with_context(|| format!("reading directory {}", path.display()))?
    {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".DS_Store" || name.starts_with('~') {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let file_type = meta.file_type();
        let kind = if file_type.is_symlink() {
            "symlink"
        } else if file_type.is_dir() {
            "directory"
        } else {
            "file"
        };
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_millis() as u64)
            .unwrap_or(0);
        let value = json!({
            "name": name,
            "kind": kind,
            "size": meta.len(),
            "modifiedMs": modified_ms,
        });
        let target_is_dir = if file_type.is_symlink() {
            std::fs::metadata(entry.path())
                .map(|target| target.is_dir())
                .unwrap_or(false)
        } else {
            file_type.is_dir()
        };
        if target_is_dir {
            dirs.push(value);
        } else {
            files.push(value);
        }
    }
    dirs.sort_by_key(entry_name_lower);
    files.sort_by_key(entry_name_lower);
    dirs.extend(files);
    Ok(json!({ "entries": dirs }))
}

pub(crate) fn read_file(params: &Value, allowed_roots: &[PathBuf]) -> Result<Value> {
    let raw = params
        .get("path")
        .and_then(Value::as_str)
        .context("missing path")?;
    let max_bytes = params
        .get("maxBytes")
        .or_else(|| params.get("max_bytes"))
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_MAX_BYTES);
    let path = validate_path(allowed_roots, raw)?;
    read_file_path(&path, max_bytes)
}

pub(crate) fn write_file(params: &Value, allowed_roots: &[PathBuf]) -> Result<Value> {
    let raw = params
        .get("path")
        .and_then(Value::as_str)
        .context("missing path")?;
    let content = params
        .get("content")
        .and_then(Value::as_str)
        .context("missing content")?;
    if content.len() as u64 > HARD_MAX_BYTES {
        bail!(
            "file is too large to write ({} bytes, hard limit {} bytes)",
            content.len(),
            HARD_MAX_BYTES
        );
    }
    let path = validate_path(allowed_roots, raw)?;
    let meta = std::fs::metadata(&path)
        .with_context(|| format!("path does not exist: {}", path.display()))?;
    if meta.is_dir() {
        bail!("path is a directory, not a file: {}", path.display());
    }
    std::fs::write(&path, content.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    read_file_path(&path, DEFAULT_MAX_BYTES)
}

pub(crate) fn validate_path(allowed_roots: &[PathBuf], raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        bail!("path must be absolute: {raw}");
    }
    let canonical = std::fs::canonicalize(&path)
        .with_context(|| format!("path does not exist: {}", path.display()))?;
    if !path_is_allowed(allowed_roots, &canonical) {
        bail!("path escapes allowed roots: {}", canonical.display());
    }
    Ok(canonical)
}

pub(crate) fn validate_write_path(allowed_roots: &[PathBuf], raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        bail!("path must be absolute: {raw}");
    }
    if path.exists() {
        let canonical = std::fs::canonicalize(&path)
            .with_context(|| format!("path does not exist: {}", path.display()))?;
        if !path_is_allowed(allowed_roots, &canonical) {
            bail!("path escapes allowed roots: {}", canonical.display());
        }
        return Ok(canonical);
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    let canonical_parent = std::fs::canonicalize(parent)
        .with_context(|| format!("parent path does not exist: {}", parent.display()))?;
    if !path_is_allowed(allowed_roots, &canonical_parent) {
        bail!("path escapes allowed roots: {}", path.display());
    }
    let Some(name) = path.file_name() else {
        bail!("path has no file name: {}", path.display());
    };
    Ok(canonical_parent.join(name))
}

pub(crate) fn path_is_allowed(allowed_roots: &[PathBuf], path: &Path) -> bool {
    allowed_roots
        .iter()
        .any(|root| path_starts_with(path, root))
}

fn read_file_path(path: &Path, max_bytes: usize) -> Result<Value> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("path does not exist: {}", path.display()))?;
    if meta.is_dir() {
        bail!("path is a directory, not a file: {}", path.display());
    }
    let size = meta.len();
    if size > HARD_MAX_BYTES {
        bail!(
            "file is too large to preview ({} bytes, hard limit {} bytes)",
            size,
            HARD_MAX_BYTES
        );
    }
    let cap = std::cmp::min(size as usize, max_bytes);
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading {}", path.display()))?
        .into_iter()
        .take(cap)
        .collect::<Vec<_>>();
    let truncated = (size as usize) > bytes.len();
    let (encoding, content) = if looks_like_text(&bytes) {
        match std::str::from_utf8(&bytes) {
            Ok(text) => ("utf8", text.to_string()),
            Err(_) => ("base64", BASE64_STANDARD.encode(&bytes)),
        }
    } else {
        ("base64", BASE64_STANDARD.encode(&bytes))
    };
    Ok(json!({
        "path": path.display().to_string(),
        "encoding": encoding,
        "content": content,
        "size": size,
        "truncated": truncated,
    }))
}

fn looks_like_text(bytes: &[u8]) -> bool {
    let sniff = &bytes[..bytes.len().min(TEXT_SNIFF_BYTES)];
    !sniff.contains(&0) && std::str::from_utf8(sniff).is_ok()
}

fn entry_name_lower(value: &Value) -> String {
    value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase()
}

fn path_starts_with(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}
