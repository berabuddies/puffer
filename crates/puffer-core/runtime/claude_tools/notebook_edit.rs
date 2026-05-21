use crate::workspace_paths;
use anyhow::{anyhow, bail, Context, Result};
use puffer_runner_api::FilesystemExecutionPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Deserialize)]
struct NotebookEditInput {
    notebook_path: String,
    #[serde(default)]
    cell_id: Option<String>,
    new_source: String,
    #[serde(default)]
    cell_type: Option<String>,
    #[serde(default)]
    edit_mode: Option<String>,
}

/// Structured NotebookEdit response payload matching the Claude-oriented shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotebookEditOutput {
    pub new_source: String,
    pub cell_id: Option<String>,
    pub cell_type: String,
    pub language: String,
    pub edit_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub notebook_path: String,
    pub original_file: String,
    pub updated_file: String,
}

/// Executes the Claude-compatible NotebookEdit operation and returns a JSON payload string.
pub fn execute_notebook_edit_tool(
    cwd: &Path,
    working_dirs: &[PathBuf],
    filesystem: &FilesystemExecutionPolicy,
    input: Value,
) -> Result<String> {
    let parsed: NotebookEditInput =
        serde_json::from_value(input).context("invalid NotebookEdit input payload")?;
    let notebook_path =
        resolve_notebook_path(cwd, working_dirs, filesystem, &parsed.notebook_path)?;
    let output = match execute_notebook_edit_inner(&parsed, &notebook_path) {
        Ok(output) => output,
        Err(error) => NotebookEditOutput {
            new_source: parsed.new_source.clone(),
            cell_id: parsed.cell_id.clone(),
            cell_type: parsed
                .cell_type
                .clone()
                .unwrap_or_else(|| "code".to_string()),
            language: "python".to_string(),
            edit_mode: parsed
                .edit_mode
                .clone()
                .unwrap_or_else(|| "replace".to_string()),
            error: Some(error.to_string()),
            notebook_path: notebook_path.display().to_string(),
            original_file: String::new(),
            updated_file: String::new(),
        },
    };
    Ok(serde_json::to_string_pretty(&output)?)
}

fn execute_notebook_edit_inner(
    input: &NotebookEditInput,
    notebook_path: &Path,
) -> Result<NotebookEditOutput> {
    ensure_notebook_path(notebook_path)?;
    ensure_cell_type(input.cell_type.as_deref())?;
    let original_file = fs::read_to_string(notebook_path)
        .with_context(|| format!("failed to read notebook {}", notebook_path.display()))?;
    let mut notebook: Value =
        serde_json::from_str(&original_file).context("Notebook is not valid JSON.")?;
    let language = notebook
        .pointer("/metadata/language_info/name")
        .and_then(Value::as_str)
        .unwrap_or("python")
        .to_string();
    let mode = parse_mode(input.edit_mode.as_deref().unwrap_or("replace"))?;
    let supports_ids = notebook_supports_cell_ids(&notebook);

    let cells = notebook
        .get_mut("cells")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("notebook is missing a cells array"))?;

    if mode == EditMode::Insert && input.cell_type.is_none() {
        bail!("Cell type is required when using edit_mode=insert.");
    }
    if input.cell_id.is_none() && mode != EditMode::Insert {
        bail!("Cell ID must be specified when not inserting a new cell.");
    }

    let mut target_index = resolve_target_index(cells, input.cell_id.as_deref(), mode)?;
    let mut effective_mode = mode;
    let mut effective_cell_type = input
        .cell_type
        .clone()
        .unwrap_or_else(|| "code".to_string());

    if effective_mode == EditMode::Replace && target_index == cells.len() {
        effective_mode = EditMode::Insert;
        if input.cell_type.is_none() {
            effective_cell_type = "code".to_string();
        }
    }

    let output_cell_id = if supports_ids {
        match effective_mode {
            EditMode::Insert => Some(generate_cell_id()),
            EditMode::Replace | EditMode::Delete => input.cell_id.clone(),
        }
    } else {
        None
    };

    match effective_mode {
        EditMode::Delete => {
            if target_index >= cells.len() {
                bail!("target cell index {} is out of range", target_index);
            }
            let removed = cells.remove(target_index);
            if input.cell_type.is_none() {
                effective_cell_type = removed
                    .get("cell_type")
                    .and_then(Value::as_str)
                    .unwrap_or("code")
                    .to_string();
            }
        }
        EditMode::Insert => {
            if target_index > cells.len() {
                target_index = cells.len();
            }
            let cell = new_notebook_cell(
                &effective_cell_type,
                &input.new_source,
                output_cell_id.as_deref(),
            )?;
            cells.insert(target_index, cell);
        }
        EditMode::Replace => {
            if target_index >= cells.len() {
                bail!("target cell index {} is out of range", target_index);
            }
            let cell = cells
                .get_mut(target_index)
                .ok_or_else(|| anyhow!("target cell index {} is out of range", target_index))?;
            replace_cell_contents(cell, &input.new_source, input.cell_type.as_deref())?;
            if input.cell_type.is_none() {
                effective_cell_type = cell
                    .get("cell_type")
                    .and_then(Value::as_str)
                    .unwrap_or("code")
                    .to_string();
            }
        }
    }

    let updated_file = to_notebook_json_with_single_space_indent(&notebook)?;
    fs::write(notebook_path, &updated_file)
        .with_context(|| format!("failed to write notebook {}", notebook_path.display()))?;
    Ok(NotebookEditOutput {
        new_source: input.new_source.clone(),
        cell_id: output_cell_id,
        cell_type: effective_cell_type,
        language,
        edit_mode: effective_mode.as_str().to_string(),
        error: None,
        notebook_path: notebook_path.display().to_string(),
        original_file,
        updated_file,
    })
}

fn resolve_notebook_path(
    cwd: &Path,
    working_dirs: &[PathBuf],
    filesystem: &FilesystemExecutionPolicy,
    notebook_path: &str,
) -> Result<PathBuf> {
    let candidate = PathBuf::from(notebook_path);
    if !candidate.is_absolute()
        && !notebook_path.trim().starts_with("~/")
        && notebook_path.trim() != "~"
    {
        bail!(
            "The notebook_path must be absolute, received `{}`",
            candidate.display()
        );
    }
    workspace_paths::resolve_path_for_filesystem_policy(
        cwd,
        working_dirs,
        filesystem.sandbox_mode,
        Path::new(notebook_path),
    )
}

fn ensure_notebook_path(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!(
            "The notebook_path must be absolute, received `{}`",
            path.display()
        );
    }
    if path.extension().and_then(|ext| ext.to_str()) != Some("ipynb") {
        bail!("File must be a Jupyter notebook (.ipynb file).");
    }
    Ok(())
}

fn ensure_cell_type(cell_type: Option<&str>) -> Result<()> {
    let Some(cell_type) = cell_type else {
        return Ok(());
    };
    if cell_type == "code" || cell_type == "markdown" {
        return Ok(());
    }
    bail!("Cell type must be either `code` or `markdown`.");
}

fn resolve_target_index(cells: &[Value], cell_id: Option<&str>, mode: EditMode) -> Result<usize> {
    let Some(cell_id) = cell_id else {
        return Ok(0);
    };
    let mut index = cells
        .iter()
        .position(|cell| cell.get("id").and_then(Value::as_str) == Some(cell_id))
        .or_else(|| parse_cell_id(cell_id))
        .ok_or_else(|| anyhow!("Cell with ID \"{}\" not found in notebook.", cell_id))?;
    if index > cells.len() {
        bail!("Cell with index {} does not exist in notebook.", index);
    }
    if mode == EditMode::Insert {
        index = index.saturating_add(1);
    }
    Ok(index)
}

fn parse_cell_id(cell_id: &str) -> Option<usize> {
    if let Some(index) = cell_id.strip_prefix("cell-") {
        return index.parse::<usize>().ok();
    }
    if cell_id.chars().all(|ch| ch.is_ascii_digit()) {
        return cell_id.parse::<usize>().ok();
    }
    None
}

fn replace_cell_contents(
    cell: &mut Value,
    new_source: &str,
    requested_type: Option<&str>,
) -> Result<()> {
    let object = cell
        .as_object_mut()
        .ok_or_else(|| anyhow!("notebook cell is not an object"))?;
    object.insert("source".to_string(), Value::String(new_source.to_string()));

    let existing_type = object
        .get("cell_type")
        .and_then(Value::as_str)
        .unwrap_or("code")
        .to_string();
    let target_type = requested_type.unwrap_or(&existing_type);
    ensure_cell_type(Some(target_type))?;

    object.insert(
        "cell_type".to_string(),
        Value::String(target_type.to_string()),
    );
    if target_type == "code" {
        object.insert("execution_count".to_string(), Value::Null);
        object.insert("outputs".to_string(), Value::Array(Vec::new()));
    } else {
        object.remove("execution_count");
        object.remove("outputs");
    }
    Ok(())
}

fn new_notebook_cell(cell_type: &str, new_source: &str, cell_id: Option<&str>) -> Result<Value> {
    ensure_cell_type(Some(cell_type))?;
    let mut cell = json!({
        "cell_type": cell_type,
        "metadata": {},
        "source": new_source,
    });
    if let Some(cell_id) = cell_id {
        cell["id"] = Value::String(cell_id.to_string());
    }
    if cell_type == "code" {
        cell["execution_count"] = Value::Null;
        cell["outputs"] = Value::Array(Vec::new());
    }
    Ok(cell)
}

fn notebook_supports_cell_ids(notebook: &Value) -> bool {
    let nbformat = notebook
        .get("nbformat")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let nbformat_minor = notebook
        .get("nbformat_minor")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    nbformat > 4 || (nbformat == 4 && nbformat_minor >= 5)
}

fn generate_cell_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}")
}

fn to_notebook_json_with_single_space_indent(notebook: &Value) -> Result<String> {
    let mut output = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut output, formatter);
    notebook.serialize(&mut serializer)?;
    String::from_utf8(output).context("notebook serialization produced non-UTF8 data")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditMode {
    Replace,
    Insert,
    Delete,
}

impl EditMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Insert => "insert",
            Self::Delete => "delete",
        }
    }
}

fn parse_mode(value: &str) -> Result<EditMode> {
    match value {
        "replace" => Ok(EditMode::Replace),
        "insert" => Ok(EditMode::Insert),
        "delete" => Ok(EditMode::Delete),
        _ => bail!("Edit mode must be replace, insert, or delete."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_runner_api::{FilesystemExecutionPolicy, FilesystemSandboxMode};

    fn workspace_write_policy() -> FilesystemExecutionPolicy {
        FilesystemExecutionPolicy {
            sandbox_mode: FilesystemSandboxMode::WorkspaceWrite,
        }
    }

    fn sample_notebook() -> Value {
        json!({
            "nbformat": 4,
            "nbformat_minor": 5,
            "metadata": {
                "language_info": { "name": "python" }
            },
            "cells": [
                {
                    "id": "alpha",
                    "cell_type": "code",
                    "source": "print('a')",
                    "metadata": {},
                    "execution_count": 1,
                    "outputs": [{"output_type": "stream", "text": "a"}]
                },
                {
                    "id": "beta",
                    "cell_type": "markdown",
                    "source": "hello",
                    "metadata": {}
                }
            ]
        })
    }

    fn write_notebook(path: &Path, notebook: &Value) {
        fs::write(path, serde_json::to_string_pretty(notebook).unwrap()).unwrap();
    }

    #[test]
    fn replace_by_cell_id_updates_code_cell_and_clears_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let notebook_path = temp.path().join("demo.ipynb");
        write_notebook(&notebook_path, &sample_notebook());

        let output = execute_notebook_edit_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            json!({
                "notebook_path": notebook_path.display().to_string(),
                "cell_id": "alpha",
                "new_source": "print('updated')",
                "edit_mode": "replace"
            }),
        )
        .unwrap();
        let payload: NotebookEditOutput = serde_json::from_str(&output).unwrap();
        assert_eq!(payload.error, None);
        assert_eq!(payload.cell_id.as_deref(), Some("alpha"));
        assert_eq!(payload.edit_mode, "replace");

        let notebook: Value =
            serde_json::from_str(&fs::read_to_string(&notebook_path).unwrap()).unwrap();
        assert_eq!(notebook["cells"][0]["source"], "print('updated')");
        assert_eq!(notebook["cells"][0]["execution_count"], Value::Null);
        assert_eq!(notebook["cells"][0]["outputs"], Value::Array(Vec::new()));
    }

    #[test]
    fn insert_without_cell_id_inserts_at_beginning() {
        let temp = tempfile::tempdir().unwrap();
        let notebook_path = temp.path().join("demo.ipynb");
        write_notebook(&notebook_path, &sample_notebook());

        let output = execute_notebook_edit_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            json!({
                "notebook_path": notebook_path.display().to_string(),
                "new_source": "first",
                "cell_type": "markdown",
                "edit_mode": "insert"
            }),
        )
        .unwrap();
        let payload: NotebookEditOutput = serde_json::from_str(&output).unwrap();
        assert_eq!(payload.error, None);
        assert_eq!(payload.edit_mode, "insert");

        let notebook: Value =
            serde_json::from_str(&fs::read_to_string(&notebook_path).unwrap()).unwrap();
        assert_eq!(notebook["cells"][0]["source"], "first");
        assert_eq!(notebook["cells"][0]["cell_type"], "markdown");
    }

    #[test]
    fn replace_without_cell_id_returns_error_payload() {
        let temp = tempfile::tempdir().unwrap();
        let notebook_path = temp.path().join("demo.ipynb");
        write_notebook(&notebook_path, &sample_notebook());

        let output = execute_notebook_edit_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            json!({
                "notebook_path": notebook_path.display().to_string(),
                "new_source": "x",
                "edit_mode": "replace"
            }),
        )
        .unwrap();
        let payload: NotebookEditOutput = serde_json::from_str(&output).unwrap();
        assert!(payload
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Cell ID must be specified")));
    }

    #[test]
    fn cell_dash_index_fallback_updates_target_cell() {
        let temp = tempfile::tempdir().unwrap();
        let notebook_path = temp.path().join("demo.ipynb");
        write_notebook(&notebook_path, &sample_notebook());

        let output = execute_notebook_edit_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            json!({
                "notebook_path": notebook_path.display().to_string(),
                "cell_id": "cell-1",
                "new_source": "second-updated",
                "edit_mode": "replace"
            }),
        )
        .unwrap();
        let payload: NotebookEditOutput = serde_json::from_str(&output).unwrap();
        assert_eq!(payload.error, None);

        let notebook: Value =
            serde_json::from_str(&fs::read_to_string(&notebook_path).unwrap()).unwrap();
        assert_eq!(notebook["cells"][1]["source"], "second-updated");
    }

    #[test]
    fn insert_requires_cell_type() {
        let temp = tempfile::tempdir().unwrap();
        let notebook_path = temp.path().join("demo.ipynb");
        write_notebook(&notebook_path, &sample_notebook());

        let output = execute_notebook_edit_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            json!({
                "notebook_path": notebook_path.display().to_string(),
                "new_source": "x",
                "edit_mode": "insert"
            }),
        )
        .unwrap();
        let payload: NotebookEditOutput = serde_json::from_str(&output).unwrap();
        assert!(payload
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Cell type is required")));
    }

    #[test]
    fn notebook_edit_rejects_paths_outside_default_writable_roots() {
        // Path outside cwd, /tmp, $TMPDIR, /add-dir. Codex-style default
        // writable set; this exercises the gate's reject branch.
        let temp = tempfile::tempdir().unwrap();
        let notebook_path =
            std::path::PathBuf::from("/__puffer_test_outside_writable_set__/demo.ipynb");

        let error = execute_notebook_edit_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            json!({
                "notebook_path": notebook_path.display().to_string(),
                "cell_id": "alpha",
                "new_source": "print('updated')",
                "edit_mode": "replace"
            }),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("outside the current working directories"));
    }
}
