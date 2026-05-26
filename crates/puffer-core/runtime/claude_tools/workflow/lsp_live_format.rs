use super::super::lsp_live_diagnostics::StoredDiagnostic;
use super::{LocationSummary, LspExecutionResult, LspSession};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use url::Url;

pub(super) fn format_hover_result(result: Value) -> Result<LspExecutionResult> {
    if result.is_null() {
        return Ok(LspExecutionResult {
            result: "No hover information available.".to_string(),
            result_count: Some(0),
            file_count: Some(0),
        });
    }
    let text = extract_hover_text(&result);
    Ok(LspExecutionResult {
        result: if text.trim().is_empty() {
            "No hover information available.".to_string()
        } else {
            text
        },
        result_count: Some(usize::from(!result.is_null())),
        file_count: Some(usize::from(!result.is_null())),
    })
}

pub(super) fn format_location_result(
    kind: &str,
    result: Value,
    cwd: &Path,
) -> Result<LspExecutionResult> {
    let locations = extract_locations(&result, cwd)?;
    if locations.is_empty() {
        return Ok(LspExecutionResult {
            result: format!("No {kind} found."),
            result_count: Some(0),
            file_count: Some(0),
        });
    }
    let mut lines = vec![format!(
        "Found {} {}{}:",
        locations.len(),
        kind,
        if locations.len() == 1 { "" } else { "s" }
    )];
    for location in &locations {
        lines.push(format!(
            "- {}:{}:{}",
            location.file_path, location.line, location.character
        ));
    }
    Ok(LspExecutionResult {
        result: lines.join("\n"),
        result_count: Some(locations.len()),
        file_count: Some(unique_file_count(&locations)),
    })
}

pub(super) fn format_references_result(result: Value, cwd: &Path) -> Result<LspExecutionResult> {
    let locations = extract_locations(&result, cwd)?;
    if locations.is_empty() {
        return Ok(LspExecutionResult {
            result: "No references found.".to_string(),
            result_count: Some(0),
            file_count: Some(0),
        });
    }
    let grouped = group_locations_by_file(&locations);
    let mut lines = vec![format!(
        "Found {} references across {} files:",
        locations.len(),
        grouped.len()
    )];
    for (file, entries) in grouped {
        lines.push(format!("\n{file}:"));
        for location in entries {
            lines.push(format!("  - line {}:{}", location.line, location.character));
        }
    }
    Ok(LspExecutionResult {
        result: lines.join("\n"),
        result_count: Some(locations.len()),
        file_count: Some(unique_file_count(&locations)),
    })
}

pub(super) fn format_document_symbol_result(
    result: Value,
    cwd: &Path,
) -> Result<LspExecutionResult> {
    let symbols = extract_document_symbols(&result, cwd)?;
    if symbols.is_empty() {
        return Ok(LspExecutionResult {
            result: "No document symbols found.".to_string(),
            result_count: Some(0),
            file_count: Some(0),
        });
    }
    let mut lines = vec![format!("Found {} document symbols:", symbols.len())];
    for symbol in &symbols {
        lines.push(format!("- {symbol}"));
    }
    Ok(LspExecutionResult {
        result: lines.join("\n"),
        result_count: Some(symbols.len()),
        file_count: Some(1),
    })
}

pub(super) fn format_diagnostics_result(diagnostics: &[StoredDiagnostic]) -> LspExecutionResult {
    if diagnostics.is_empty() {
        return LspExecutionResult {
            result: "No diagnostics found.".to_string(),
            result_count: Some(0),
            file_count: Some(0),
        };
    }
    let mut lines = vec![format!("Found {} diagnostics:", diagnostics.len())];
    for diagnostic in diagnostics {
        let mut detail = format!(
            "- line {}:{} [{}]",
            diagnostic.line,
            diagnostic.character,
            severity_label(diagnostic.severity)
        );
        if let Some(source) = diagnostic.source.as_deref() {
            detail.push_str(&format!(" source={source}"));
        }
        if let Some(code) = diagnostic.code.as_deref() {
            detail.push_str(&format!(" code={code}"));
        }
        detail.push_str(&format!(" {}", diagnostic.message));
        lines.push(detail);
    }
    LspExecutionResult {
        result: lines.join("\n"),
        result_count: Some(diagnostics.len()),
        file_count: Some(1),
    }
}

pub(super) fn format_workspace_symbol_result(
    result: Value,
    cwd: &Path,
) -> Result<LspExecutionResult> {
    let symbols = extract_workspace_symbols(&result, cwd)?;
    if symbols.is_empty() {
        return Ok(LspExecutionResult {
            result: "No workspace symbols found.".to_string(),
            result_count: Some(0),
            file_count: Some(0),
        });
    }
    let mut lines = vec![format!("Found {} workspace symbols:", symbols.len())];
    for symbol in &symbols {
        lines.push(format!("- {symbol}"));
    }
    Ok(LspExecutionResult {
        result: lines.join("\n"),
        result_count: Some(symbols.len()),
        file_count: Some(
            symbols
                .iter()
                .filter_map(|entry| entry.split_once(" - ").map(|(_, right)| right.to_string()))
                .collect::<BTreeSet<_>>()
                .len(),
        ),
    })
}

pub(super) fn format_prepare_call_hierarchy_result(
    result: Value,
    cwd: &Path,
) -> Result<LspExecutionResult> {
    let items = extract_call_hierarchy_items(&result, cwd)?;
    if items.is_empty() {
        return Ok(LspExecutionResult {
            result: "No call hierarchy item found at this position.".to_string(),
            result_count: Some(0),
            file_count: Some(0),
        });
    }
    let mut lines = vec![format!("Prepared {} call hierarchy items:", items.len())];
    for item in &items {
        lines.push(format!("- {item}"));
    }
    Ok(LspExecutionResult {
        result: lines.join("\n"),
        result_count: Some(items.len()),
        file_count: Some(items.len()),
    })
}

pub(super) fn format_call_hierarchy_result(
    session: &mut LspSession,
    method: &str,
    label: &str,
    prepared: Value,
    cwd: &Path,
) -> Result<LspExecutionResult> {
    let items = prepared.as_array().cloned().unwrap_or_default();
    let Some(item) = items.first() else {
        return Ok(LspExecutionResult {
            result: "No call hierarchy item found at this position.".to_string(),
            result_count: Some(0),
            file_count: Some(0),
        });
    };
    let result = session.request(method, json!({ "item": item }))?;
    let calls = extract_call_hierarchy_calls(&result, label, cwd)?;
    if calls.is_empty() {
        return Ok(LspExecutionResult {
            result: format!("No {label} found."),
            result_count: Some(0),
            file_count: Some(0),
        });
    }
    let mut lines = vec![format!("Found {} {}:", calls.len(), label)];
    for call in &calls {
        lines.push(format!("- {call}"));
    }
    Ok(LspExecutionResult {
        result: lines.join("\n"),
        result_count: Some(calls.len()),
        file_count: Some(calls.len()),
    })
}

fn extract_hover_text(result: &Value) -> String {
    let Some(contents) = result.get("contents") else {
        return String::new();
    };
    match contents {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(extract_hover_text_from_item)
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Object(_) => extract_hover_text_from_item(contents),
        _ => String::new(),
    }
}

fn extract_hover_text_from_item(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    if let Some(text) = value.get("value").and_then(Value::as_str) {
        return text.to_string();
    }
    String::new()
}

fn severity_label(severity: Option<u64>) -> &'static str {
    match severity {
        Some(1) => "error",
        Some(2) => "warning",
        Some(3) => "information",
        Some(4) => "hint",
        _ => "unknown",
    }
}

fn extract_locations(result: &Value, cwd: &Path) -> Result<Vec<LocationSummary>> {
    if result.is_null() {
        return Ok(Vec::new());
    }
    if let Some(array) = result.as_array() {
        return array
            .iter()
            .map(|entry| location_from_value(entry, cwd))
            .collect::<Result<Vec<_>>>();
    }
    Ok(vec![location_from_value(result, cwd)?])
}

fn location_from_value(value: &Value, cwd: &Path) -> Result<LocationSummary> {
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("LSP location is missing a URI"))?;
    let range = value
        .get("range")
        .or_else(|| value.get("targetSelectionRange"))
        .or_else(|| value.get("targetRange"))
        .ok_or_else(|| anyhow!("LSP location is missing a range"))?;
    let start = range
        .get("start")
        .ok_or_else(|| anyhow!("LSP location range is missing start"))?;
    Ok(LocationSummary {
        file_path: format_uri(uri, cwd),
        line: start.get("line").and_then(Value::as_u64).unwrap_or(0) as usize + 1,
        character: start.get("character").and_then(Value::as_u64).unwrap_or(0) as usize + 1,
    })
}

fn extract_document_symbols(result: &Value, cwd: &Path) -> Result<Vec<String>> {
    let Some(array) = result.as_array() else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    for value in array {
        flatten_document_symbol(value, cwd, &mut output)?;
    }
    Ok(output)
}

fn flatten_document_symbol(value: &Value, cwd: &Path, output: &mut Vec<String>) -> Result<()> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("document symbol missing name"))?;
    let detail = value
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let location = value
        .get("selectionRange")
        .or_else(|| value.get("range"))
        .and_then(|range| range.get("start"))
        .map(|start| {
            format!(
                "{}:{}",
                start.get("line").and_then(Value::as_u64).unwrap_or(0) + 1,
                start.get("character").and_then(Value::as_u64).unwrap_or(0) + 1
            )
        })
        .unwrap_or_else(|| "?:?".to_string());
    let kind = symbol_kind_name(value.get("kind").and_then(Value::as_u64));
    output.push(
        [
            name.to_string(),
            kind.to_string(),
            detail.to_string(),
            location,
        ]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" - "),
    );
    if let Some(children) = value.get("children").and_then(Value::as_array) {
        for child in children {
            flatten_document_symbol(child, cwd, output)?;
        }
    }
    let _ = cwd;
    Ok(())
}

fn extract_workspace_symbols(result: &Value, cwd: &Path) -> Result<Vec<String>> {
    let Some(array) = result.as_array() else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    for value in array {
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("workspace symbol missing name"))?;
        let kind = symbol_kind_name(value.get("kind").and_then(Value::as_u64));
        let location = value
            .get("location")
            .map(|location| location_from_value(location, cwd))
            .transpose()?;
        let suffix = location
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file_path, location.line, location.character
                )
            })
            .unwrap_or_else(|| "<unknown location>".to_string());
        output.push(format!("{name} - {kind} - {suffix}"));
    }
    Ok(output)
}

fn extract_call_hierarchy_items(result: &Value, cwd: &Path) -> Result<Vec<String>> {
    let Some(array) = result.as_array() else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    for value in array {
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("call hierarchy item missing name"))?;
        let uri = value
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("call hierarchy item missing uri"))?;
        let range = value
            .get("selectionRange")
            .or_else(|| value.get("range"))
            .and_then(|range| range.get("start"))
            .ok_or_else(|| anyhow!("call hierarchy item missing selection range"))?;
        output.push(format!(
            "{} - {}:{}:{}",
            name,
            format_uri(uri, cwd),
            range.get("line").and_then(Value::as_u64).unwrap_or(0) + 1,
            range.get("character").and_then(Value::as_u64).unwrap_or(0) + 1
        ));
    }
    Ok(output)
}

fn extract_call_hierarchy_calls(result: &Value, label: &str, cwd: &Path) -> Result<Vec<String>> {
    let Some(array) = result.as_array() else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    for value in array {
        let item = value
            .get(if label == "incoming calls" {
                "from"
            } else {
                "to"
            })
            .ok_or_else(|| anyhow!("call hierarchy entry missing item"))?;
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("call hierarchy target missing name"))?;
        let uri = item
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("call hierarchy target missing uri"))?;
        let range = item
            .get("selectionRange")
            .or_else(|| item.get("range"))
            .and_then(|range| range.get("start"))
            .ok_or_else(|| anyhow!("call hierarchy target missing range"))?;
        output.push(format!(
            "{} - {}:{}:{}",
            name,
            format_uri(uri, cwd),
            range.get("line").and_then(Value::as_u64).unwrap_or(0) + 1,
            range.get("character").and_then(Value::as_u64).unwrap_or(0) + 1
        ));
    }
    Ok(output)
}

fn format_uri(uri: &str, cwd: &Path) -> String {
    let file_path = Url::parse(uri)
        .ok()
        .and_then(|url| url.to_file_path().ok())
        .unwrap_or_else(|| PathBuf::from(uri));
    file_path
        .strip_prefix(cwd)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| file_path.display().to_string())
        .replace('\\', "/")
}

fn group_locations_by_file(
    locations: &[LocationSummary],
) -> BTreeMap<String, Vec<LocationSummary>> {
    let mut grouped = BTreeMap::new();
    for location in locations {
        grouped
            .entry(location.file_path.clone())
            .or_insert_with(Vec::new)
            .push(location.clone());
    }
    grouped
}

fn unique_file_count(locations: &[LocationSummary]) -> usize {
    locations
        .iter()
        .map(|location| location.file_path.clone())
        .collect::<BTreeSet<_>>()
        .len()
}

fn symbol_kind_name(kind: Option<u64>) -> &'static str {
    match kind.unwrap_or_default() {
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        12 => "function",
        13 => "variable",
        14 => "constant",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type-parameter",
        _ => "symbol",
    }
}
