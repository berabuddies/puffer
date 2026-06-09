//! DOM inspection expression builder for BrowserAction.

use anyhow::{bail, Result};

/// Builds the bounded selector inspection expression for `domInspect`.
pub(super) fn dom_inspect_expression(query: &str) -> Result<String> {
    let query = query.trim();
    if query.is_empty() {
        bail!("domInspect requires a non-empty CSS selector query");
    }
    let query = serde_json::to_string(query)?;
    Ok(format!(
        r#"(() => {{
  const selector = {query};
  const all = Array.from(document.querySelectorAll(selector));
  const nodes = all.slice(0, 25);
  const textOf = (el) => (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 240);
  const nameOf = (el) => (
    el.getAttribute('aria-label') ||
    el.getAttribute('placeholder') ||
    el.getAttribute('alt') ||
    el.getAttribute('title') ||
    textOf(el)
  ).slice(0, 240);
  const attrsOf = (el) => {{
    const attrs = {{}};
    for (const name of ['id', 'name', 'type', 'role', 'href', 'placeholder', 'aria-label']) {{
      const value = el.getAttribute(name);
      if (value) attrs[name] = value.slice(0, 240);
    }}
    return attrs;
  }};
  return {{
    query: selector,
    count: all.length,
    truncated: all.length > nodes.length,
    elements: nodes.map((el, index) => {{
      const rect = el.getBoundingClientRect();
      const style = getComputedStyle(el);
      return {{
        index,
        tag: el.tagName.toLowerCase(),
        role: el.getAttribute('role') || (el instanceof HTMLInputElement ? (el.type || 'text') : ''),
        name: nameOf(el),
        text: textOf(el),
        visible: rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none',
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        attributes: attrsOf(el)
      }};
    }})
  }};
}})()"#
    ))
}
