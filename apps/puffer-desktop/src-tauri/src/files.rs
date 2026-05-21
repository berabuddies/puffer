use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

const DEFAULT_MAX_BYTES: usize = 262_144;
const READ_HARD_MAX_BYTES: u64 = 24 * 1024 * 1024;
const WRITE_HARD_MAX_BYTES: u64 = 5 * 1024 * 1024;
const TEXT_SNIFF_BYTES: usize = 8 * 1024;
const NATIVE_TEXT_PREVIEW_MAX_LINES: usize = 200;
const NATIVE_HTML_PREVIEW_MAX_BYTES: usize = 256 * 1024;

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
    if content.len() as u64 > WRITE_HARD_MAX_BYTES {
        bail!(
            "file is too large to write ({} bytes, hard limit {} bytes)",
            content.len(),
            WRITE_HARD_MAX_BYTES
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
    let requested_cap = std::cmp::min(max_bytes, READ_HARD_MAX_BYTES as usize);
    let cap = std::cmp::min(size, requested_cap as u64) as usize;
    let bytes = read_file_prefix(path, cap)?;
    let truncated = (size as usize) > bytes.len();
    let (encoding, content) = if !should_return_base64(path, &bytes) {
        match std::str::from_utf8(&bytes) {
            Ok(text) => ("utf8", text.to_string()),
            Err(_) => ("base64", BASE64_STANDARD.encode(&bytes)),
        }
    } else {
        ("base64", BASE64_STANDARD.encode(&bytes))
    };
    let mut result = json!({
        "path": path.display().to_string(),
        "encoding": encoding,
        "content": content,
        "size": size,
        "truncated": truncated,
    });
    if let Some(lines) = native_text_preview(path) {
        result["textPreview"] = json!(lines);
    }
    if let Some(html) = native_html_preview(path) {
        result["htmlPreview"] = json!(html);
    }
    Ok(result)
}

fn read_file_prefix(path: &Path, cap: usize) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut bytes = Vec::with_capacity(cap);
    file.by_ref()
        .take(cap as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(bytes)
}

fn looks_like_text(bytes: &[u8]) -> bool {
    let sniff = &bytes[..bytes.len().min(TEXT_SNIFF_BYTES)];
    !sniff.contains(&0) && std::str::from_utf8(sniff).is_ok()
}

fn should_return_base64(path: &Path, bytes: &[u8]) -> bool {
    is_document_preview_binary(path) || !looks_like_text(bytes)
}

fn is_document_preview_binary(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "pdf" | "doc" | "dot" | "docx" | "ppt" | "pptx" | "xls" | "xlsx" | "xlsm"
    )
}

fn native_text_preview(path: &Path) -> Option<Vec<String>> {
    if !is_native_preview_candidate(path) {
        return None;
    }
    let output = Command::new("textutil")
        .args([
            "-convert", "txt", "-stdout", "-noload", "-timeout", "5", "--",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let lines = normalize_native_text_preview(&text);
    (!lines.is_empty()).then_some(lines)
}

fn native_html_preview(path: &Path) -> Option<String> {
    if !is_native_preview_candidate(path) {
        return None;
    }
    let output = Command::new("textutil")
        .args([
            "-convert", "html", "-stdout", "-noload", "-timeout", "5", "--",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    let html = String::from_utf8_lossy(&output.stdout);
    let normalized = html.trim();
    if normalized.is_empty() {
        return None;
    }
    Some(normalized.chars().take(NATIVE_HTML_PREVIEW_MAX_BYTES).collect())
}

fn is_native_preview_candidate(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "doc" | "dot" | "rtf"))
        .unwrap_or(false)
}

fn normalize_native_text_preview(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        let trimmed = normalized.trim();
        if trimmed.is_empty() {
            continue;
        }
        lines.push(trimmed.chars().take(600).collect());
        if lines.len() >= NATIVE_TEXT_PREVIEW_MAX_LINES {
            break;
        }
    }
    lines
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
        assert!(should_return_base64(
            Path::new("/tmp/puffer/template.dot"),
            b"plain looking legacy template"
        ));
        assert!(!should_return_base64(
            Path::new("/tmp/puffer/README.md"),
            b"# Notes\n"
        ));
    }

    #[test]
    fn native_preview_targets_legacy_word_files() {
        assert!(is_native_preview_candidate(Path::new(
            "/tmp/puffer/legacy.DOC"
        )));
        assert!(is_native_preview_candidate(Path::new(
            "/tmp/puffer/template.dot"
        )));
        assert!(is_native_preview_candidate(Path::new(
            "/tmp/puffer/rich-text.rtf"
        )));
        assert!(!is_native_preview_candidate(Path::new(
            "/tmp/puffer/report.docx"
        )));
        assert!(!is_native_preview_candidate(Path::new(
            "/tmp/puffer/slides.ppt"
        )));
    }

    #[test]
    fn native_text_preview_normalizes_lines() {
        let lines = normalize_native_text_preview("  first\t line  \n\nsecond   line\n");
        assert_eq!(lines, vec!["first line", "second line"]);
    }

    #[test]
    fn legacy_doc_read_includes_native_text_preview_when_available() {
        if Command::new("textutil").arg("-help").output().is_err() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("legacy.doc");
        std::fs::write(&path, r"{\rtf1\ansi Native legacy doc text}").unwrap();
        let result = read_file_path(&path, DEFAULT_MAX_BYTES).unwrap();
        let preview = result.get("textPreview").and_then(Value::as_array).unwrap();
        assert!(preview
            .iter()
            .any(|line| line.as_str() == Some("Native legacy doc text")));
    }

    #[test]
    fn read_file_caps_large_document_previews_instead_of_rejecting() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.pdf");
        let mut file = std::fs::File::create(&path).unwrap();
        std::io::Write::write_all(&mut file, b"%PDF-1.4\n").unwrap();
        file.set_len(READ_HARD_MAX_BYTES + 1).unwrap();
        let result = read_file_path(&path, 32).unwrap();
        assert_eq!(result.get("encoding").and_then(Value::as_str), Some("base64"));
        assert_eq!(result.get("size").and_then(Value::as_u64), Some(READ_HARD_MAX_BYTES + 1));
        assert_eq!(result.get("truncated").and_then(Value::as_bool), Some(true));
    }

    #[test]
    fn legacy_doc_read_includes_native_html_preview_when_available() {
        if Command::new("textutil").arg("-help").output().is_err() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("legacy.doc");
        std::fs::write(&path, r"{\rtf1\ansi Native \b styled\b0 legacy doc text}").unwrap();
        let result = read_file_path(&path, DEFAULT_MAX_BYTES).unwrap();
        let html = result.get("htmlPreview").and_then(Value::as_str).unwrap();
        assert!(html.contains("styled"));
        assert!(html.contains("<html"));
    }
}
