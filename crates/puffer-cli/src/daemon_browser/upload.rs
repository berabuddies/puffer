use anyhow::{bail, Context, Result};
use serde_json::Value;

/// Returns the JavaScript used to resolve a live native file input handle.
pub(super) fn upload_input_handle_expression(x: f64, y: f64) -> String {
    format!(
        r#"(() => {{
  const el = document.elementFromPoint({x}, {y});
  if (!el) throw new Error('No element at target ref');
  const resolveFileInput = (node) => {{
    if (!node) return null;
    if (node instanceof HTMLInputElement && node.type === 'file') return node;
    const direct = node.closest('input[type="file"]');
    if (direct instanceof HTMLInputElement && direct.type === 'file') return direct;
    const label = node.closest('label');
    if (label) {{
      if (label.control instanceof HTMLInputElement && label.control.type === 'file') return label.control;
      const nested = label.querySelector('input[type="file"]');
      if (nested instanceof HTMLInputElement && nested.type === 'file') return nested;
    }}
    const nested = node.querySelector?.('input[type="file"]');
    if (nested instanceof HTMLInputElement && nested.type === 'file') return nested;
    return null;
  }};
  const target = resolveFileInput(el);
  if (!(target instanceof HTMLInputElement) || target.type !== 'file') {{
    throw new Error('Target is not a native file input');
  }}
  return target;
}})()"#
    )
}

/// Returns the JavaScript used to validate an upload target and monitor events.
pub(super) fn upload_prepare_function() -> &'static str {
    r#"function(fileCount) {
  if (!(this instanceof HTMLInputElement) || this.type !== 'file') {
    throw new Error('Target is not a native file input');
  }
  if (this.disabled) {
    throw new Error('Target file input is disabled');
  }
  if (fileCount > 1 && !this.multiple) {
    throw new Error('Target file input does not accept multiple files');
  }
  const state = this.__pufferUploadState || { input: 0, change: 0 };
  state.input = 0;
  state.change = 0;
  if (!this.__pufferUploadState) {
    Object.defineProperty(this, '__pufferUploadState', {
      value: state,
      configurable: true
    });
    this.addEventListener('input', () => { state.input += 1; }, true);
    this.addEventListener('change', () => { state.change += 1; }, true);
  }
  return { multiple: !!this.multiple };
}"#
}

/// Returns the JavaScript used to synthesize upload events only when needed.
pub(super) fn upload_finalize_function() -> &'static str {
    r#"function() {
  if (!(this instanceof HTMLInputElement) || this.type !== 'file') {
    throw new Error('Target is not a native file input');
  }
  const state = this.__pufferUploadState || { input: 0, change: 0 };
  const sawInput = state.input > 0;
  const sawChange = state.change > 0;
  if (!sawInput) {
    this.dispatchEvent(new Event('input', { bubbles: true }));
  }
  if (!sawChange) {
    this.dispatchEvent(new Event('change', { bubbles: true }));
  }
  return {
    input: sawInput ? 'native' : 'synthetic',
    change: sawChange ? 'native' : 'synthetic'
  };
}"#
}

/// Parses one upload target lookup response and returns the live remote object id.
pub(super) fn parse_upload_handle_response(value: &Value) -> Result<String> {
    ensure_runtime_success(value, "browser upload target lookup")?;
    value
        .pointer("/result/result/objectId")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .context("browser upload target lookup returned no object id")
}

/// Validates one upload runtime call response.
pub(super) fn parse_upload_runtime_response(value: &Value, context: &str) -> Result<()> {
    ensure_runtime_success(value, context)?;
    value
        .pointer("/result/result")
        .context("browser upload runtime response missing result")?;
    Ok(())
}

/// Validates one `DOM.setFileInputFiles` response.
pub(super) fn parse_upload_set_files_response(value: &Value) -> Result<()> {
    if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
        bail!("browser upload failed: {message}");
    }
    value
        .get("result")
        .context("browser upload response missing result")?;
    Ok(())
}

fn ensure_runtime_success(value: &Value, context: &str) -> Result<()> {
    if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
        bail!("{context} failed: {message}");
    }
    if let Some(details) = value.pointer("/result/exceptionDetails") {
        let description = details
            .pointer("/exception/description")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::trim)
            .map(ToString::to_string);
        let text = details
            .get("text")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::trim)
            .map(ToString::to_string);
        let line_number = details.get("lineNumber").and_then(Value::as_u64);
        let column_number = details.get("columnNumber").and_then(Value::as_u64);
        let message = description
            .or(text)
            .unwrap_or_else(|| "unknown browser exception".to_string());
        if let (Some(line), Some(column)) = (line_number, column_number) {
            bail!(
                "{context} failed at line {}, column {}: {}",
                line + 1,
                column + 1,
                message
            );
        }
        bail!("{context} failed: {message}");
    }
    Ok(())
}
