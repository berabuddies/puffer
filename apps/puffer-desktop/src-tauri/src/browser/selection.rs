use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::BrowserCopySelection;

const COPY_SELECTION_SCRIPT: &str = r#"
(() => {
  function activeElementSelection(doc) {
    const el = doc.activeElement;
    if (!el) return null;
    const tag = el.tagName ? el.tagName.toLowerCase() : "";
    if ((tag === "textarea" || tag === "input") &&
        typeof el.selectionStart === "number" &&
        typeof el.selectionEnd === "number" &&
        el.selectionStart !== el.selectionEnd) {
      return {
        text: el.value.slice(el.selectionStart, el.selectionEnd),
        copiedFrom: tag
      };
    }
    return null;
  }

  function documentSelection(doc) {
    const inputSelection = activeElementSelection(doc);
    if (inputSelection) return inputSelection;

    const selection = doc.getSelection ? doc.getSelection() : null;
    const text = selection ? String(selection.toString()) : "";
    if (text) {
      return { text, copiedFrom: "document-selection" };
    }
    return null;
  }

  function searchDocument(doc) {
    const current = documentSelection(doc);
    if (current) return current;

    for (const frame of doc.querySelectorAll("iframe, frame")) {
      try {
        const childDoc = frame.contentDocument;
        if (!childDoc) continue;
        const childSelection = searchDocument(childDoc);
        if (childSelection && childSelection.text) {
          return {
            text: childSelection.text,
            copiedFrom: childSelection.copiedFrom === "none"
              ? "same-origin-frame"
              : `same-origin-frame:${childSelection.copiedFrom}`
          };
        }
      } catch (_) {
      }
    }

    return { text: "", copiedFrom: "none" };
  }

  return searchDocument(document);
})()
"#;

/// Returns the JavaScript used to read the active page or input selection.
pub(super) fn selection_eval_expression() -> &'static str {
    COPY_SELECTION_SCRIPT
}

/// Parses the Chrome Runtime.evaluate response for a copy-selection request.
pub(super) fn parse_copy_selection_response(value: &Value) -> Result<BrowserCopySelection> {
    if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
        bail!("Chrome selection copy failed: {message}");
    }
    if let Some(message) = value
        .pointer("/result/exceptionDetails/text")
        .and_then(Value::as_str)
    {
        bail!("Chrome selection script failed: {message}");
    }
    let result = value
        .pointer("/result/result/value")
        .context("Chrome selection response missing value")?;
    Ok(BrowserCopySelection {
        text: result
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        copied_from: result
            .get("copiedFrom")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string(),
    })
}
