//! Filesystem RPCs for the Files tab in the desktop app.
//!
//! Methods:
//!   - `list_dir(path)` → directory listing, dirs first then files
//!   - `read_file(path, {maxBytes?})` → UTF-8 or base64 content
//!   - `write_file(path, content)` → overwrite an existing text file
//!
//! Path safety: every incoming path must be absolute AND canonicalize
//! underneath one of the daemon's allowed roots — the session cwd, the
//! workspace root, or the user's $HOME. This is defense in depth; the
//! daemon already requires an auth token, but we still refuse to hand
//! out arbitrary filesystem contents via a missed validation.

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::daemon::DaemonState;

const DEFAULT_MAX_BYTES: usize = 262_144; // 256 KiB
const READ_HARD_MAX_BYTES: u64 = 24 * 1024 * 1024; // 24 MiB preview refusal threshold
const WRITE_HARD_MAX_BYTES: u64 = 5 * 1024 * 1024;
const TEXT_SNIFF_BYTES: usize = 8 * 1024;

pub(crate) fn handle_list_dir(state: &DaemonState, params: &Value) -> Result<Value> {
    let raw = params
        .get("path")
        .and_then(|v| v.as_str())
        .context("missing path")?;
    let path = validate_path(state, raw)?;

    let meta = std::fs::metadata(&path)
        .with_context(|| format!("path does not exist: {}", path.display()))?;
    if !meta.is_dir() {
        bail!("path is not a directory: {}", path.display());
    }

    let iter = std::fs::read_dir(&path)
        .with_context(|| format!("reading directory {}", path.display()))?;

    let mut dirs: Vec<Value> = Vec::new();
    let mut files: Vec<Value> = Vec::new();
    for entry in iter {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        // Skip macOS cruft + editor backup files per spec.
        if name == ".DS_Store" || name.starts_with('~') {
            continue;
        }

        // symlink_metadata — classify the entry itself first so we can
        // report "symlink"; then follow to determine fallback kind if the
        // target is still resolvable.
        let lmeta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let ftype = lmeta.file_type();
        let kind = if ftype.is_symlink() {
            "symlink"
        } else if ftype.is_dir() {
            "directory"
        } else {
            "file"
        };

        let size = lmeta.len();
        let modified_ms = lmeta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let value = json!({
            "name": name,
            "kind": kind,
            "size": size,
            "modifiedMs": modified_ms,
        });

        // For sort-bucketing, symlinks follow their target: if the target
        // is a directory, group with dirs; otherwise with files. This
        // matches typical file-manager UX and keeps the tree intuitive.
        let target_is_dir = if ftype.is_symlink() {
            std::fs::metadata(entry.path())
                .map(|m| m.is_dir())
                .unwrap_or(false)
        } else {
            ftype.is_dir()
        };

        if target_is_dir {
            dirs.push(value);
        } else {
            files.push(value);
        }
    }

    fn name_of(v: &Value) -> &str {
        v.get("name").and_then(|n| n.as_str()).unwrap_or("")
    }
    dirs.sort_by(|a, b| name_of(a).to_lowercase().cmp(&name_of(b).to_lowercase()));
    files.sort_by(|a, b| name_of(a).to_lowercase().cmp(&name_of(b).to_lowercase()));

    let mut entries = dirs;
    entries.extend(files);
    Ok(json!({ "entries": entries }))
}

pub(crate) fn handle_read_file(state: &DaemonState, params: &Value) -> Result<Value> {
    let raw = params
        .get("path")
        .and_then(|v| v.as_str())
        .context("missing path")?;
    let max_bytes = params
        .get("maxBytes")
        .or_else(|| params.get("max_bytes"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_MAX_BYTES);

    let path = validate_path(state, raw)?;
    let meta = std::fs::metadata(&path)
        .with_context(|| format!("path does not exist: {}", path.display()))?;
    if meta.is_dir() {
        bail!("path is a directory, not a file: {}", path.display());
    }
    let size = meta.len();
    if size > READ_HARD_MAX_BYTES {
        bail!(
            "file is too large to preview ({} bytes, hard limit {} bytes)",
            size,
            READ_HARD_MAX_BYTES
        );
    }

    let cap = std::cmp::min(size as usize, max_bytes);
    let mut buf = vec![0u8; cap];
    use std::io::Read;
    let mut f =
        std::fs::File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let mut filled = 0usize;
    while filled < cap {
        let n = f
            .read(&mut buf[filled..])
            .with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    buf.truncate(filled);

    let truncated = (size as usize) > filled;

    let (encoding, content) = if !should_return_base64(&path, &buf) {
        // looks_like_text already established the first 8 KiB is valid
        // UTF-8 with no NULs; decode the whole buffer and, if that fails
        // (mixed encoding past the sniff window), fall back to base64.
        match std::str::from_utf8(&buf) {
            Ok(s) => ("utf8", s.to_string()),
            Err(_) => ("base64", BASE64_STANDARD.encode(&buf)),
        }
    } else {
        ("base64", BASE64_STANDARD.encode(&buf))
    };

    Ok(json!({
        "path": path.display().to_string(),
        "encoding": encoding,
        "content": content,
        "size": size,
        "truncated": truncated,
    }))
}

/// Overwrite one existing text file and return the updated read-file result.
pub(crate) fn handle_write_file(state: &DaemonState, params: &Value) -> Result<Value> {
    let raw = params
        .get("path")
        .and_then(|v| v.as_str())
        .context("missing path")?;
    let content = params
        .get("content")
        .and_then(|v| v.as_str())
        .context("missing content")?;

    let path = validate_path(state, raw)?;
    let meta = std::fs::metadata(&path)
        .with_context(|| format!("path does not exist: {}", path.display()))?;
    if meta.is_dir() {
        bail!("path is a directory, not a file: {}", path.display());
    }
    if content.len() as u64 > WRITE_HARD_MAX_BYTES {
        bail!(
            "file is too large to write ({} bytes, hard limit {} bytes)",
            content.len(),
            WRITE_HARD_MAX_BYTES
        );
    }

    std::fs::write(&path, content.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    handle_read_file(state, &json!({ "path": path.display().to_string() }))
}

/// Validate that `raw` is absolute and that its canonical form lives
/// under one of the allowed roots. Returns the canonicalized path so
/// callers use it for subsequent syscalls (avoids TOCTOU symlink games
/// between validation and read).
pub(crate) fn validate_path(state: &DaemonState, raw: &str) -> Result<PathBuf> {
    let p = PathBuf::from(raw);
    if !p.is_absolute() {
        bail!("path must be absolute: {}", raw);
    }
    let canonical = std::fs::canonicalize(&p)
        .with_context(|| format!("path does not exist: {}", p.display()))?;

    if !path_is_allowed(state, &canonical) {
        bail!("path escapes allowed roots: {}", canonical.display());
    }
    Ok(canonical)
}

/// Shared allowlist check used by `list_dir` / `read_file` AND by the
/// filesystem watcher. `path` is expected to be canonical (or at least
/// absolute). Returns true iff it lives under the session cwd, the
/// workspace root, or the user's $HOME.
pub(crate) fn path_is_allowed(state: &DaemonState, path: &Path) -> bool {
    for root in allowed_roots(state) {
        if path_starts_with(path, &root) {
            return true;
        }
    }
    false
}

pub(crate) fn allowed_roots(state: &DaemonState) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(c) = std::fs::canonicalize(state.cwd_path()) {
        roots.push(c);
    } else {
        roots.push(state.cwd_path().to_path_buf());
    }
    let ws = state.workspace_root();
    if let Ok(c) = std::fs::canonicalize(&ws) {
        roots.push(c);
    } else {
        roots.push(ws);
    }
    if let Some(home) = home_dir() {
        if let Ok(c) = std::fs::canonicalize(&home) {
            roots.push(c);
        } else {
            roots.push(home);
        }
    }
    roots
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn path_starts_with(path: &Path, root: &Path) -> bool {
    let p: Vec<_> = path.components().collect();
    let r: Vec<_> = root.components().collect();
    if p.len() < r.len() {
        return false;
    }
    p[..r.len()] == r[..]
}

/// Simple text-vs-binary sniff: the first 8 KiB has no NUL bytes and is
/// valid UTF-8. Matches git's heuristic well enough for a preview pane.
fn looks_like_text(buf: &[u8]) -> bool {
    let window = &buf[..buf.len().min(TEXT_SNIFF_BYTES)];
    if window.contains(&0u8) {
        return false;
    }
    std::str::from_utf8(window).is_ok()
}

fn should_return_base64(path: &Path, buf: &[u8]) -> bool {
    is_document_preview_binary(path) || !looks_like_text(buf)
}

fn is_document_preview_binary(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "pdf" | "doc" | "docx" | "ppt" | "pptx" | "xls" | "xlsx" | "xlsm"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_previews_return_base64_even_when_the_header_is_ascii() {
        let ascii_pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n";
        assert!(should_return_base64(
            Path::new("/tmp/puffer/sample.pdf"),
            ascii_pdf
        ));
        assert!(should_return_base64(
            Path::new("/tmp/puffer/legacy.doc"),
            b"plain looking legacy document"
        ));
        assert!(!should_return_base64(
            Path::new("/tmp/puffer/README.md"),
            b"# Notes\n"
        ));
    }
}
