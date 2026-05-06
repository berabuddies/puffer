use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::BrowserCursor;

/// Builds the JavaScript used to inspect the cursor at a viewport coordinate.
pub(super) fn cursor_eval_expression(x: f64, y: f64) -> String {
    format!(
        r#"
(() => {{
  const rootX = {x};
  const rootY = {y};
  const fallbackCursor = "default";
  const allowed = new Set([
    "alias", "all-scroll", "cell", "col-resize", "context-menu",
    "copy", "crosshair", "default", "e-resize", "ew-resize",
    "grab", "grabbing", "help", "move", "n-resize", "ne-resize",
    "nesw-resize", "no-drop", "none", "not-allowed", "ns-resize",
    "nw-resize", "nwse-resize", "pointer", "progress", "row-resize",
    "s-resize", "se-resize", "sw-resize", "text", "vertical-text",
    "w-resize", "wait", "zoom-in", "zoom-out"
  ]);

  function nearestActionCursor(element) {{
    if (!element || !element.closest) return null;
    const editable = element.closest("textarea,input,[contenteditable=''],[contenteditable='true']");
    if (editable) return "text";
    const actionable = element.closest("a[href],button,select,summary,[role='button'],[role='link'],[onclick]");
    if (actionable) return "pointer";
    return null;
  }}

  function pointInsideRect(x, y, rect) {{
    return x >= rect.left - 1 && x <= rect.right + 1 && y >= rect.top - 1 && y <= rect.bottom + 1;
  }}

  function textNodeAtCaret(doc, x, y) {{
    if (doc.caretPositionFromPoint) {{
      const position = doc.caretPositionFromPoint(x, y);
      if (position) return {{ node: position.offsetNode, offset: position.offset }};
    }}
    if (doc.caretRangeFromPoint) {{
      const range = doc.caretRangeFromPoint(x, y);
      if (range) return {{ node: range.startContainer, offset: range.startOffset }};
    }}
    return null;
  }}

  function textNodeHasHit(doc, node, offset, x, y) {{
    if (!node || node.nodeType !== Node.TEXT_NODE) return false;
    const text = node.nodeValue || "";
    if (!/\S/.test(text)) return false;
    const range = doc.createRange();
    range.selectNodeContents(node);
    for (const rect of range.getClientRects()) {{
      if (pointInsideRect(x, y, rect)) return true;
    }}
    const candidates = [offset - 1, offset, offset + 1];
    for (const candidate of candidates) {{
      const start = Math.max(0, Math.min(text.length - 1, candidate));
      const end = Math.min(text.length, start + 1);
      if (start >= end || !/\S/.test(text.slice(start, end))) continue;
      range.setStart(node, start);
      range.setEnd(node, end);
      for (const rect of range.getClientRects()) {{
        if (pointInsideRect(x, y, rect)) return true;
      }}
    }}
    return false;
  }}

  function selectableTextAtPoint(doc, element, x, y) {{
    if (!element) return false;
    const view = doc.defaultView;
    const computed = view ? view.getComputedStyle(element) : null;
    if (computed && computed.userSelect === "none") return false;
    const caret = textNodeAtCaret(doc, x, y);
    return caret ? textNodeHasHit(doc, caret.node, caret.offset, x, y) : false;
  }}

  function cursorForElement(doc, element, x, y) {{
    if (!element) return fallbackCursor;
    const view = element.ownerDocument && element.ownerDocument.defaultView;
    const computed = view ? view.getComputedStyle(element).cursor : "";
    if (computed && computed !== "auto") return computed;
    const actionCursor = nearestActionCursor(element);
    if (actionCursor) return actionCursor;
    if (selectableTextAtPoint(doc, element, x, y)) return "text";
    return fallbackCursor;
  }}

  function elementAt(doc, x, y) {{
    let element = doc.elementFromPoint(x, y);
    while (element && /^(IFRAME|FRAME)$/.test(element.tagName || "")) {{
      try {{
        const rect = element.getBoundingClientRect();
        const childDoc = element.contentDocument;
        if (!childDoc) break;
        const childX = x - rect.left;
        const childY = y - rect.top;
        const childElement = childDoc.elementFromPoint(childX, childY);
        if (!childElement) break;
        element = childElement;
        doc = childDoc;
        x = childX;
        y = childY;
      }} catch (_) {{
        break;
      }}
    }}
    return {{ doc, element, x, y }};
  }}

  const hit = elementAt(document, rootX, rootY);
  const rawCursor = cursorForElement(hit.doc, hit.element, hit.x, hit.y);
  const cursor = allowed.has(rawCursor) ? rawCursor : fallbackCursor;
  return {{ cursor }};
}})()
"#
    )
}

/// Parses the Chrome Runtime.evaluate response for a cursor query.
pub(super) fn parse_cursor_response(value: &Value) -> Result<BrowserCursor> {
    if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
        bail!("Chrome cursor query failed: {message}");
    }
    if let Some(message) = value
        .pointer("/result/exceptionDetails/text")
        .and_then(Value::as_str)
    {
        bail!("Chrome cursor script failed: {message}");
    }
    let result = value
        .pointer("/result/result/value")
        .context("Chrome cursor response missing value")?;
    Ok(BrowserCursor {
        cursor: result
            .get("cursor")
            .and_then(Value::as_str)
            .unwrap_or("default")
            .to_string(),
    })
}
