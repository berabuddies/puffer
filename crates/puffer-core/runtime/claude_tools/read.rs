use crate::workspace_paths;
use anyhow::{anyhow, bail, Context, Result};
use puffer_runner_api::FilesystemExecutionPolicy;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_LINES_TO_READ: usize = 2000;
const MAX_PDF_PAGES_PER_READ: u32 = 20;
const MAX_BINARY_PREVIEW_BYTES: usize = 512;

#[derive(Debug, Deserialize)]
struct ClaudeReadInput {
    file_path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    pages: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClaudeReadOutput {
    Text { file: TextFilePayload },
    Image { file: ImageFilePayload },
    Notebook { file: NotebookFilePayload },
    Pdf { file: PdfFilePayload },
    FileUnchanged { file: UnchangedFilePayload },
}

#[derive(Debug, Serialize)]
struct TextFilePayload {
    #[serde(rename = "filePath")]
    file_path: String,
    content: String,
    #[serde(rename = "numLines")]
    num_lines: usize,
    #[serde(rename = "startLine")]
    start_line: usize,
    #[serde(rename = "totalLines")]
    total_lines: usize,
}

#[derive(Debug, Serialize)]
struct ImageFilePayload {
    base64: String,
    #[serde(rename = "type")]
    mime_type: String,
    #[serde(rename = "originalSize")]
    original_size: usize,
}

#[derive(Debug, Serialize)]
struct NotebookFilePayload {
    #[serde(rename = "filePath")]
    file_path: String,
    cells: Vec<Value>,
}

#[derive(Debug, Serialize)]
struct PdfFilePayload {
    #[serde(rename = "filePath")]
    file_path: String,
    base64: String,
    #[serde(rename = "originalSize")]
    original_size: usize,
}

#[derive(Debug, Serialize)]
struct UnchangedFilePayload {
    #[serde(rename = "filePath")]
    file_path: String,
}

/// Executes the Claude-style `Read` tool and returns a JSON-serialized result payload.
pub fn execute_claude_read_tool(
    cwd: &Path,
    working_dirs: &[PathBuf],
    filesystem: &FilesystemExecutionPolicy,
    input: Value,
) -> Result<String> {
    let input: ClaudeReadInput =
        serde_json::from_value(input).context("invalid Read tool input")?;
    let input = ClaudeReadInput {
        pages: normalize_optional_string(input.pages),
        ..input
    };
    let output = execute_claude_read(cwd, working_dirs, filesystem, input)?;
    Ok(serde_json::to_string_pretty(&output)?)
}

/// Returns a serialized Claude-compatible payload indicating the file has not
/// changed since the caller's prior full read.
pub fn execute_claude_file_unchanged(file_path: &str) -> Result<String> {
    Ok(serde_json::to_string_pretty(
        &ClaudeReadOutput::FileUnchanged {
            file: UnchangedFilePayload {
                file_path: file_path.to_string(),
            },
        },
    )?)
}

fn execute_claude_read(
    cwd: &Path,
    working_dirs: &[PathBuf],
    filesystem: &FilesystemExecutionPolicy,
    input: ClaudeReadInput,
) -> Result<ClaudeReadOutput> {
    if let Some(limit) = input.limit {
        if limit == 0 {
            bail!("Read limit must be greater than 0");
        }
    }

    if let Some(pages) = input.pages.as_deref() {
        validate_pdf_pages(pages)?;
    }

    let path = resolve_absolute_read_path(cwd, working_dirs, filesystem, &input.file_path)?;
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if ext == "ipynb" {
        return read_notebook(&path, &input.file_path);
    }
    if ext == "pdf" {
        return read_pdf(&path, &input.file_path, input.pages.as_deref());
    }
    if let Some(mime_type) = image_mime_type(&ext) {
        return read_image(&path, mime_type);
    }
    read_text(&path, &input.file_path, input.offset, input.limit)
}

fn read_text(
    path: &Path,
    original_file_path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<ClaudeReadOutput> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read text file {}", path.display()))?;
    let contents = match std::str::from_utf8(&bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let preview = format_binary_preview(&bytes);
            let selected = format!(
                "<system-reminder>Warning: the file is not valid UTF-8. Returning a binary preview instead. Use Bash or another binary-aware tool if you need the raw bytes.</system-reminder>\n{}",
                preview
            );
            let num_lines = selected.lines().count();
            return Ok(ClaudeReadOutput::Text {
                file: TextFilePayload {
                    file_path: original_file_path.to_string(),
                    content: selected,
                    num_lines,
                    start_line: 1,
                    total_lines: num_lines,
                },
            });
        }
    };
    let total_lines = contents.lines().count();
    // CC convention is 1-indexed offsets ("Line numbers begin at 1").
    // A model that passes `offset: 0` previously got back
    // `start_line: 0` and content rendered with line numbers
    // `0\tfirst_line\n` — a quietly off-by-one rendering that the
    // model isn't told about and can't easily detect. Clamp to 1
    // so the rendering matches the convention.
    let start_line = offset.map(|o| o.max(1)).unwrap_or(1);
    let effective_limit = limit.unwrap_or(MAX_LINES_TO_READ);

    let start_index = start_line.saturating_sub(1);
    let all_lines = contents.lines().collect::<Vec<_>>();
    let end_index = start_index
        .saturating_add(effective_limit)
        .min(all_lines.len());
    let num_lines = if contents.is_empty() || start_index >= all_lines.len() {
        0
    } else {
        end_index.saturating_sub(start_index)
    };
    let selected = if contents.is_empty() {
        "<system-reminder>Warning: the file exists but the contents are empty.</system-reminder>"
            .to_string()
    } else if start_index >= all_lines.len() {
        format!(
            "<system-reminder>Warning: the file exists but is shorter than the provided offset ({start_line}). The file has {total_lines} lines.</system-reminder>"
        )
    } else {
        let mut text = all_lines[start_index..end_index].join("\n");
        if contents.ends_with('\n') && !text.is_empty() {
            text.push('\n');
        }
        format_with_line_numbers(&text, start_line)
    };

    Ok(ClaudeReadOutput::Text {
        file: TextFilePayload {
            file_path: original_file_path.to_string(),
            num_lines,
            content: selected,
            start_line,
            total_lines,
        },
    })
}

fn format_with_line_numbers(content: &str, start_line: usize) -> String {
    let mut formatted = String::new();
    for (index, line) in content.lines().enumerate() {
        formatted.push_str(&format!("{:>6}\t{line}\n", start_line + index));
    }
    formatted
}

fn format_binary_preview(bytes: &[u8]) -> String {
    let preview_len = bytes.len().min(MAX_BINARY_PREVIEW_BYTES);
    let mut lines = Vec::new();
    for (line_index, chunk) in bytes[..preview_len].chunks(16).enumerate() {
        let offset = line_index * 16;
        let hex = chunk
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        let ascii = chunk
            .iter()
            .map(|byte| {
                if byte.is_ascii_graphic() || *byte == b' ' {
                    *byte as char
                } else {
                    '.'
                }
            })
            .collect::<String>();
        lines.push(format!("{offset:08x}  {hex:<47}  |{ascii}|"));
    }
    if preview_len < bytes.len() {
        lines.push(format!(
            "... truncated binary preview; {} additional bytes omitted ...",
            bytes.len() - preview_len
        ));
    }
    lines.join("\n")
}

fn read_notebook(path: &Path, original_file_path: &str) -> Result<ClaudeReadOutput> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read notebook {}", path.display()))?;
    let notebook: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse notebook {}", path.display()))?;
    let cells = notebook
        .get("cells")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| anyhow!("notebook is missing a cells array"))?;
    Ok(ClaudeReadOutput::Notebook {
        file: NotebookFilePayload {
            file_path: original_file_path.to_string(),
            cells,
        },
    })
}

fn read_pdf(
    path: &Path,
    original_file_path: &str,
    pages: Option<&str>,
) -> Result<ClaudeReadOutput> {
    if let Some(pages) = pages {
        return read_pdf_pages(path, original_file_path, pages);
    }
    let bytes = fs::read(path).with_context(|| format!("failed to read PDF {}", path.display()))?;
    Ok(ClaudeReadOutput::Pdf {
        file: PdfFilePayload {
            file_path: original_file_path.to_string(),
            base64: encode_base64(&bytes),
            original_size: bytes.len(),
        },
    })
}

fn read_pdf_pages(path: &Path, original_file_path: &str, pages: &str) -> Result<ClaudeReadOutput> {
    let (first_page, last_page) = parse_pdf_range(pages)?;
    let output = Command::new("pdftotext")
        .args([
            "-f",
            &first_page.to_string(),
            "-l",
            &last_page.to_string(),
            path.to_string_lossy().as_ref(),
            "-",
        ])
        .output()
        .with_context(|| format!("failed to execute pdftotext for {}", path.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            "pdftotext exited unsuccessfully".to_string()
        } else {
            stderr
        };
        bail!(
            "failed to extract PDF pages `{pages}` from {}: {detail}",
            path.display()
        );
    }

    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let trimmed = raw.trim_end_matches('\u{c}').trim_end().to_string();
    let num_lines = trimmed.lines().count();
    Ok(ClaudeReadOutput::Text {
        file: TextFilePayload {
            file_path: original_file_path.to_string(),
            content: if trimmed.is_empty() {
                "<system-reminder>Warning: extracted PDF page contents are empty.</system-reminder>"
                    .to_string()
            } else {
                format_with_line_numbers(&trimmed, 1)
            },
            num_lines,
            start_line: 1,
            total_lines: num_lines,
        },
    })
}

fn read_image(path: &Path, mime_type: &str) -> Result<ClaudeReadOutput> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read image {}", path.display()))?;
    Ok(ClaudeReadOutput::Image {
        file: ImageFilePayload {
            base64: encode_base64(&bytes),
            mime_type: mime_type.to_string(),
            original_size: bytes.len(),
        },
    })
}

fn validate_pdf_pages(value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        bail!("invalid pages parameter: value is empty");
    }
    let (first_page, last_page) = if let Some((left, right)) = value.split_once('-') {
        let first = parse_page_number(left)?;
        let last = parse_page_number(right)?;
        if last < first {
            bail!("invalid pages parameter `{value}`: range end must be >= start");
        }
        (first, last)
    } else {
        let page = parse_page_number(value)?;
        (page, page)
    };
    let span = last_page - first_page + 1;
    if span > MAX_PDF_PAGES_PER_READ {
        bail!("page range `{value}` exceeds maximum of {MAX_PDF_PAGES_PER_READ} pages per request");
    }
    Ok(())
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn parse_page_number(value: &str) -> Result<u32> {
    let page = value
        .trim()
        .parse::<u32>()
        .with_context(|| format!("invalid page number `{value}`"))?;
    if page == 0 {
        bail!("page numbers are 1-indexed and must be >= 1");
    }
    Ok(page)
}

fn parse_pdf_range(value: &str) -> Result<(u32, u32)> {
    if let Some((left, right)) = value.trim().split_once('-') {
        Ok((parse_page_number(left)?, parse_page_number(right)?))
    } else {
        let page = parse_page_number(value)?;
        Ok((page, page))
    }
}

fn resolve_absolute_read_path(
    cwd: &Path,
    working_dirs: &[PathBuf],
    filesystem: &FilesystemExecutionPolicy,
    raw_path: &str,
) -> Result<PathBuf> {
    let provided = PathBuf::from(raw_path);
    if !provided.is_absolute() && !raw_path.trim().starts_with("~/") && raw_path.trim() != "~" {
        bail!(
            "Read requires an absolute file_path; received `{}`",
            provided.display()
        );
    }
    let resolved = workspace_paths::resolve_path_for_filesystem_policy(
        cwd,
        working_dirs,
        filesystem.sandbox_mode,
        Path::new(raw_path),
    )?;
    if resolved.is_dir() {
        bail!(
            "Read can only read files, not directories (`{}`)",
            resolved.display()
        );
    }
    if !resolved.exists() {
        bail!("file does not exist: {}", resolved.display());
    }
    let metadata = fs::metadata(&resolved)
        .with_context(|| format!("failed to stat file {}", resolved.display()))?;
    if !metadata.is_file() {
        bail!(
            "Read can only read regular files (`{}`)",
            resolved.display()
        );
    }
    Ok(resolved)
}

fn image_mime_type(ext: &str) -> Option<&'static str> {
    match ext {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut index = 0usize;
    while index + 3 <= bytes.len() {
        let chunk = ((bytes[index] as u32) << 16)
            | ((bytes[index + 1] as u32) << 8)
            | (bytes[index + 2] as u32);
        encoded.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
        encoded.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
        encoded.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
        encoded.push(TABLE[(chunk & 0x3f) as usize] as char);
        index += 3;
    }
    match bytes.len() - index {
        1 => {
            let chunk = (bytes[index] as u32) << 16;
            encoded.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            encoded.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
            encoded.push('=');
            encoded.push('=');
        }
        2 => {
            let chunk = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8);
            encoded.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            encoded.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
            encoded.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
            encoded.push('=');
        }
        _ => {}
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_runner_api::{FilesystemExecutionPolicy, FilesystemSandboxMode};
    use std::sync::{Mutex, OnceLock};

    fn workspace_write_policy() -> FilesystemExecutionPolicy {
        FilesystemExecutionPolicy {
            sandbox_mode: FilesystemSandboxMode::WorkspaceWrite,
        }
    }

    #[test]
    fn text_read_uses_one_indexed_offset() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file.txt");
        fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
        let payload = serde_json::json!({
            "file_path": path.display().to_string(),
            "offset": 2,
            "limit": 2,
        });
        let output =
            execute_claude_read_tool(temp.path(), &[], &workspace_write_policy(), payload).unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["type"], "text");
        assert_eq!(parsed["file"]["content"], "     2\ttwo\n     3\tthree\n");
        assert_eq!(parsed["file"]["startLine"], 2);
        assert_eq!(parsed["file"]["numLines"], 2);
        assert_eq!(parsed["file"]["totalLines"], 4);
    }

    #[test]
    fn text_read_defaults_to_2000_lines() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("long.txt");
        let content = (0..2500)
            .map(|value| format!("line-{value}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, content).unwrap();
        let payload = serde_json::json!({
            "file_path": path.display().to_string(),
        });
        let output =
            execute_claude_read_tool(temp.path(), &[], &workspace_write_policy(), payload).unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["type"], "text");
        assert_eq!(parsed["file"]["startLine"], 1);
        assert_eq!(parsed["file"]["numLines"], 2000);
        assert_eq!(parsed["file"]["totalLines"], 2500);
        assert!(parsed["file"]["content"]
            .as_str()
            .is_some_and(|content| content.starts_with("     1\tline-0\n")));
    }

    #[test]
    fn relative_paths_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let payload = serde_json::json!({
            "file_path": "relative.txt",
        });
        let error = execute_claude_read_tool(temp.path(), &[], &workspace_write_policy(), payload)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("Read requires an absolute file_path"));
    }

    #[test]
    fn notebook_read_returns_cells_array() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("notebook.ipynb");
        fs::write(
            &path,
            serde_json::json!({
                "cells": [
                    {"cell_type": "markdown", "source": ["hello"]},
                    {"cell_type": "code", "source": ["print(1)"]},
                ]
            })
            .to_string(),
        )
        .unwrap();
        let payload = serde_json::json!({
            "file_path": path.display().to_string(),
        });
        let output =
            execute_claude_read_tool(temp.path(), &[], &workspace_write_policy(), payload).unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["type"], "notebook");
        assert_eq!(parsed["file"]["cells"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn image_read_returns_mime_and_base64() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("image.png");
        fs::write(&path, [0u8, 1u8, 2u8, 3u8]).unwrap();
        let payload = serde_json::json!({
            "file_path": path.display().to_string(),
        });
        let output =
            execute_claude_read_tool(temp.path(), &[], &workspace_write_policy(), payload).unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["type"], "image");
        assert_eq!(parsed["file"]["type"], "image/png");
        assert_eq!(parsed["file"]["base64"], "AAECAw==");
    }

    #[test]
    fn binary_file_read_returns_preview_instead_of_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("blob.bin");
        fs::write(&path, [0xff, 0x00, 0x41, 0x42, 0x7f]).unwrap();
        let payload = serde_json::json!({
            "file_path": path.display().to_string(),
        });

        let output =
            execute_claude_read_tool(temp.path(), &[], &workspace_write_policy(), payload).unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();

        assert_eq!(parsed["type"], "text");
        assert!(parsed["file"]["content"]
            .as_str()
            .is_some_and(|content| content.contains("not valid UTF-8")));
        assert!(parsed["file"]["content"]
            .as_str()
            .is_some_and(|content| content.contains("00000000  ff 00 41 42 7f")));
    }

    #[test]
    fn pages_validation_enforces_reference_limit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("doc.pdf");
        fs::write(&path, "pdf").unwrap();
        let payload = serde_json::json!({
            "file_path": path.display().to_string(),
            "pages": "1-30",
        });
        let error = execute_claude_read_tool(temp.path(), &[], &workspace_write_policy(), payload)
            .unwrap_err();
        assert!(error.to_string().contains("exceeds maximum of 20 pages"));
    }

    #[test]
    fn blank_pages_value_is_treated_as_absent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("notes.txt");
        fs::write(&path, "hello\n").unwrap();
        let payload = serde_json::json!({
            "file_path": path.display().to_string(),
            "pages": "   ",
        });

        let output =
            execute_claude_read_tool(temp.path(), &[], &workspace_write_policy(), payload).unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();

        assert_eq!(parsed["type"], "text");
        assert!(parsed["file"]["content"]
            .as_str()
            .is_some_and(|content| content.contains("hello")));
    }

    #[test]
    fn empty_file_read_returns_warning_stub() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("empty.txt");
        fs::write(&path, "").unwrap();
        let output = execute_claude_read_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            serde_json::json!({ "file_path": path.display().to_string() }),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert!(parsed["file"]["content"]
            .as_str()
            .is_some_and(|text| text.contains("contents are empty")));
    }

    /// Regression: a model that passes `offset: 0` used to get back
    /// `start_line: 0` and content rendered with line numbers
    /// `0\tfirst_line\n` — quietly off-by-one vs the
    /// CC-convention 1-indexing the rest of Read uses. Now clamped
    /// to 1.
    #[test]
    fn offset_zero_is_clamped_to_one() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file.txt");
        fs::write(&path, "alpha\nbeta\ngamma\n").unwrap();
        let output = execute_claude_read_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            serde_json::json!({
                "file_path": path.display().to_string(),
                "offset": 0,
                "limit": 2,
            }),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["file"]["startLine"], 1, "offset=0 must clamp to 1");
        // Line numbering should start at 1, not 0.
        let content = parsed["file"]["content"].as_str().unwrap();
        assert!(
            content.starts_with("     1\talpha"),
            "first line should be numbered 1; got: {content:?}"
        );
        assert!(content.contains("     2\tbeta"));
        assert!(!content.contains("     0\t"));
    }

    #[test]
    fn offset_beyond_end_returns_warning_stub() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("short.txt");
        fs::write(&path, "one\ntwo\n").unwrap();
        let output = execute_claude_read_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            serde_json::json!({
                "file_path": path.display().to_string(),
                "offset": 5,
            }),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert!(parsed["file"]["content"]
            .as_str()
            .is_some_and(|text| text.contains("shorter than the provided offset (5)")));
    }

    #[test]
    fn file_unchanged_output_uses_claude_variant_shape() {
        let output = execute_claude_file_unchanged("/tmp/demo.txt").unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["type"], "file_unchanged");
        assert_eq!(parsed["file"]["filePath"], "/tmp/demo.txt");
    }

    #[test]
    fn pdf_pages_are_extracted_with_pdftotext_when_available() {
        let _guard = env_lock().lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let bin_dir = temp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let script = bin_dir.join("pdftotext");
        fs::write(&script, "#!/bin/sh\nprintf 'Page one\\nPage two\\n'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script, permissions).unwrap();
        }

        let old_path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{old_path}", bin_dir.display()));

        let pdf_path = temp.path().join("sample.pdf");
        fs::write(&pdf_path, "pdf").unwrap();
        let output = execute_claude_read_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            serde_json::json!({
                "file_path": pdf_path.display().to_string(),
                "pages": "1-2",
            }),
        )
        .unwrap();

        std::env::set_var("PATH", old_path);

        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["type"], "text");
        assert!(parsed["file"]["content"]
            .as_str()
            .is_some_and(|text| text.contains("Page one")));
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn read_allows_files_in_added_working_directories() {
        let temp = tempfile::tempdir().unwrap();
        let extra = temp.path().join("extra");
        let path = extra.join("note.txt");
        fs::create_dir_all(&extra).unwrap();
        fs::write(&path, "hello\n").unwrap();

        let output = execute_claude_read_tool(
            temp.path(),
            &[extra],
            &workspace_write_policy(),
            serde_json::json!({ "file_path": path.display().to_string() }),
        )
        .unwrap();

        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["type"], "text");
        assert!(parsed["file"]["content"]
            .as_str()
            .is_some_and(|text| text.contains("hello")));
    }

    #[test]
    fn read_rejects_files_outside_default_writable_roots() {
        // Path outside cwd, /tmp, $TMPDIR, /add-dir. Codex-style default
        // writable set; this exercises the gate's reject branch.
        let temp = tempfile::tempdir().unwrap();
        let path = std::path::PathBuf::from("/__puffer_test_outside_writable_set__/secret.txt");

        let error = execute_claude_read_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            serde_json::json!({ "file_path": path.display().to_string() }),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("working director"));
    }
}
