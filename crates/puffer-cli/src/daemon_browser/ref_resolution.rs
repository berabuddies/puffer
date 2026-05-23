//! Browser ref resolution helpers for agent actions.

use anyhow::Result;

use super::screenshot::BrowserElementRef;

const HELPERS: &str = r#"  const normalizeText = (value) => String(value ?? '').replace(/\s+/g, ' ').trim().toLowerCase();
  const isVisible = (el) => {
    const style = getComputedStyle(el);
    if (style.visibility === 'hidden' || style.display === 'none') return false;
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0 && rect.bottom >= 0 && rect.right >= 0 &&
      rect.top <= innerHeight && rect.left <= innerWidth;
  };
  const nameFor = (el) => {
    const aria = el.getAttribute('aria-label') || el.getAttribute('alt') || el.getAttribute('title');
    if (aria) return aria.trim();
    if (el.labels && el.labels.length) return Array.from(el.labels).map((label) => label.innerText).join(' ').trim();
    if (el.placeholder) return el.placeholder.trim();
    if (el.value && el.tagName !== 'OPTION') return String(el.value).trim();
    return (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim();
  };
  const roleFor = (el) => {
    const explicit = el.getAttribute('role');
    if (explicit) return explicit;
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'button') return 'button';
    if (tag === 'input') return el.type || 'textbox';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'select') return 'combobox';
    return tag;
  };
  const selector = 'a,button,input,textarea,select,summary,[role],[contenteditable="true"],[tabindex],label';
  const normalizeHref = (value) => normalizeText(value || '');
  const resolveLabelTarget = (node, editableSelector) => {
    if (!node) return null;
    const direct = node.closest(editableSelector);
    if (direct) return direct;
    const label = node.closest('label');
    if (label) {
      if (label.control) return label.control;
      const nested = label.querySelector(editableSelector);
      if (nested) return nested;
    }
    return node.querySelector?.(editableSelector) || null;
  };
  const resolveCheckableTarget = (node) => {
    if (!node) return null;
    const selector = 'input[type="checkbox"], input[type="radio"], [role="checkbox"], [role="radio"]';
    if (node instanceof HTMLInputElement && (node.type === 'checkbox' || node.type === 'radio')) {
      return node;
    }
    const direct = node.closest(selector);
    if (direct) return direct;
    const label = node.closest('label');
    if (label) {
      if (label.control instanceof HTMLInputElement &&
          (label.control.type === 'checkbox' || label.control.type === 'radio')) {
        return label.control;
      }
      const nested = label.querySelector(selector);
      if (nested) return nested;
    }
    return node.querySelector?.(selector) || null;
  };
  const resolveFileInputTarget = (node) => {
    if (!node) return null;
    if (node instanceof HTMLInputElement && node.type === 'file') return node;
    const direct = node.closest('input[type="file"]');
    if (direct instanceof HTMLInputElement && direct.type === 'file') return direct;
    const label = node.closest('label');
    if (label) {
      if (label.control instanceof HTMLInputElement && label.control.type === 'file') return label.control;
      const nested = label.querySelector('input[type="file"]');
      if (nested instanceof HTMLInputElement && nested.type === 'file') return nested;
    }
    const nested = node.querySelector?.('input[type="file"]');
    if (nested instanceof HTMLInputElement && nested.type === 'file') return nested;
    return null;
  };
  const resolveSelectTarget = (node) => {
    if (!node) return null;
    const direct = node.closest('select');
    if (direct) return direct;
    const label = node.closest('label');
    if (label) {
      if (label.control instanceof HTMLSelectElement) return label.control;
      const nested = label.querySelector('select');
      if (nested) return nested;
    }
    return node.querySelector?.('select') || null;
  };
  const resolveFocusableTarget = (node) => {
    if (!node) return null;
    return node.closest('input, textarea, select, button, a, [tabindex], [contenteditable="true"], [role="button"], [role="link"], [role="checkbox"], [role="radio"], [role="switch"], [role="combobox"], [role="textbox"]') || node;
  };
  const elementMatchesTarget = (el, target) => {
    if (!el) return false;
    if (normalizeText(roleFor(el)) !== normalizeText(target.role)) return false;
    if (normalizeText(el.tagName.toLowerCase()) !== normalizeText(target.tag)) return false;
    const targetName = normalizeText(target.name);
    const currentName = normalizeText(nameFor(el));
    if (targetName && currentName !== targetName) return false;
    const targetHref = normalizeHref(target.href);
    if (targetHref && normalizeHref(el.href) !== targetHref) return false;
    return true;
  };
  const scoreCandidate = (el, target) => {
    let score = 0;
    if (normalizeText(roleFor(el)) === normalizeText(target.role)) score += 4;
    if (normalizeText(el.tagName.toLowerCase()) === normalizeText(target.tag)) score += 4;
    const currentName = normalizeText(nameFor(el));
    const targetName = normalizeText(target.name);
    if (targetName && currentName === targetName) {
      score += 8;
    } else if (targetName && currentName && (currentName.includes(targetName) || targetName.includes(currentName))) {
      score += 4;
    }
    const targetHref = normalizeHref(target.href);
    if (targetHref && normalizeHref(el.href) === targetHref) score += 4;
    const rect = el.getBoundingClientRect();
    const dx = (rect.left + rect.width / 2) - target.x;
    const dy = (rect.top + rect.height / 2) - target.y;
    score -= Math.min(Math.hypot(dx, dy) / 200, 2);
    return score;
  };
  const findTarget = (target) => {
    const hit = document.elementFromPoint(target.x, target.y);
    const hitTarget = hit ? resolveLabelTarget(hit, 'a,button,input,textarea,select,summary,[role],[contenteditable="true"],[tabindex],label') : null;
    if (elementMatchesTarget(hitTarget || hit, target)) {
      return hitTarget || hit;
    }
    const candidates = Array.from(document.querySelectorAll(selector)).filter(isVisible);
    let best = null;
    let bestScore = Number.NEGATIVE_INFINITY;
    for (const candidate of candidates) {
      const score = scoreCandidate(candidate, target);
      if (score > bestScore) {
        best = candidate;
        bestScore = score;
      }
    }
    if (best && bestScore >= 6) {
      return best;
    }
    return null;
  };"#;

/// Builds the shared ref-resolution helper block for browser agent scripts.
pub(super) fn browser_target_resolution_helpers() -> &'static str {
    HELPERS
}

/// Builds a script that resolves one ref to its current viewport center point.
pub(super) fn target_point_expression(target: &BrowserElementRef) -> Result<String> {
    let target = serde_json::to_string(target)?;
    Ok(format!(
        r#"(() => {{
{helpers}
  const refTarget = {target};
  const refElement = findTarget(refTarget);
  if (!refElement) throw new Error(`No element matched browser ref ${{refTarget.ref}}`);
  const rect = refElement.getBoundingClientRect();
  return {{ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 }};
}})()"#,
        helpers = HELPERS
    ))
}

/// Builds a script that focuses one resolved ref.
pub(super) fn focus_expression(target: &BrowserElementRef) -> Result<String> {
    ref_script(
        target,
        r#"  const targetEl = refElement;
  if (typeof targetEl.focus !== 'function') throw new Error('Target is not focusable');
  targetEl.focus({ preventScroll: false });
  return true;
"#,
    )
}

/// Builds a script that clicks one resolved ref.
pub(super) fn click_expression(target: &BrowserElementRef) -> Result<String> {
    ref_script(
        target,
        r#"  refElement.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
  refElement.click();
  return true;
"#,
    )
}

/// Builds a script that double-clicks one resolved ref.
pub(super) fn double_click_expression(target: &BrowserElementRef) -> Result<String> {
    ref_script(
        target,
        r#"  refElement.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
  refElement.click();
  refElement.click();
  refElement.dispatchEvent(new MouseEvent('dblclick', { bubbles: true, detail: 2 }));
  return true;
"#,
    )
}

/// Builds a script that hovers one resolved ref.
pub(super) fn hover_expression(target: &BrowserElementRef) -> Result<String> {
    ref_script(
        target,
        r#"  const rect = refElement.getBoundingClientRect();
  refElement.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
  refElement.dispatchEvent(new MouseEvent('mousemove', {
    bubbles: true,
    clientX: rect.left + rect.width / 2,
    clientY: rect.top + rect.height / 2,
    buttons: 0
  }));
  return true;
"#,
    )
}

/// Builds a script that fills one resolved ref.
pub(super) fn fill_expression(target: &BrowserElementRef, text: &str) -> Result<String> {
    let text = serde_json::to_string(text)?;
    ref_script(
        target,
        &format!(
            r#"  const targetEl = refElement.closest('input, textarea, [contenteditable="true"]') || refElement;
  if ('value' in targetEl) {{
    const prototype = targetEl instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
    const descriptor = Object.getOwnPropertyDescriptor(prototype, 'value');
    if (descriptor && descriptor.set) {{
      descriptor.set.call(targetEl, {text});
    }} else {{
      targetEl.value = {text};
    }}
    targetEl.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: {text} }}));
    targetEl.dispatchEvent(new Event('change', {{ bubbles: true }}));
  }} else if (targetEl.isContentEditable) {{
    targetEl.textContent = {text};
    targetEl.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: {text} }}));
  }} else {{
    throw new Error('Target is not editable');
  }}
  return true;
"#
        ),
    )
}

/// Builds a script that selects one resolved native `<select>`.
pub(super) fn select_expression(target: &BrowserElementRef, value: &str) -> Result<String> {
    let value = serde_json::to_string(value)?;
    ref_script(
        target,
        &format!(
            r#"  const targetEl = refElement.closest('select') || refElement;
  const normalize = (value) => String(value ?? '').trim();
  const requested = normalize({value});
  const options = Array.from(targetEl.options || []);
  const match = options.find((option) => {{
    const optionValue = normalize(option.value);
    const optionLabel = normalize(option.label || option.textContent || option.value);
    return optionValue === requested || optionLabel === requested;
  }});
  if (!match) {{
    const available = options.slice(0, 12).map((option) => normalize(option.label || option.textContent || option.value)).filter(Boolean).join(', ');
    throw new Error(available ? `No option matched "${{requested}}". Available: ${{available}}` : `No option matched "${{requested}}"`);
  }}
  for (const option of options) option.selected = option === match;
  targetEl.value = match.value;
  targetEl.dispatchEvent(new Event('input', {{ bubbles: true }}));
  targetEl.dispatchEvent(new Event('change', {{ bubbles: true }}));
  return true;
"#
        ),
    )
}

/// Builds a script that returns the resolved file input handle.
pub(super) fn upload_input_handle_expression(target: &BrowserElementRef) -> Result<String> {
    ref_script(
        target,
        r#"  const targetEl = resolveFileInputTarget(refElement);
  if (!(targetEl instanceof HTMLInputElement) || targetEl.type !== 'file') {
    throw new Error('Target is not a native file input');
  }
  return targetEl;
"#,
    )
}

/// Builds a script that resolves a checkable control and reports its state.
pub(super) fn checkable_state_expression(target: &BrowserElementRef) -> Result<String> {
    ref_script(
        target,
        r#"  const targetEl = resolveCheckableTarget(refElement);
  if (!targetEl) throw new Error('Target is not a checkbox or radio control');
  if (targetEl instanceof HTMLInputElement) {
    return {
      kind: targetEl.type === 'radio' ? 'radio' : 'checkbox',
      checked: !!targetEl.checked
    };
  }
  return {
    kind: targetEl.getAttribute('role') === 'radio' ? 'radio' : 'checkbox',
    checked: targetEl.getAttribute('aria-checked') === 'true'
  };
"#,
    )
}

/// Builds a script that toggles one resolved checkable control.
pub(super) fn set_checkable_state_expression(
    target: &BrowserElementRef,
    checked: bool,
) -> Result<String> {
    let checked = if checked { "true" } else { "false" };
    ref_script(
        target,
        &format!(
            r#"  const targetEl = resolveCheckableTarget(refElement);
  if (!targetEl) throw new Error('Target is not a checkbox or radio control');
  const desired = {checked};
  if (targetEl instanceof HTMLInputElement) {{
    if (targetEl.type === 'radio' && !desired) {{
      throw new Error('radio buttons cannot be unchecked directly with the current browser ref model');
    }}
    if (Boolean(targetEl.checked) !== desired) {{
      targetEl.click();
    }}
    if (Boolean(targetEl.checked) !== desired) {{
      throw new Error(`target did not become ${{desired ? 'checked' : 'unchecked'}}`);
    }}
    return true;
  }}
  const current = targetEl.getAttribute('aria-checked') === 'true';
  if (current !== desired) {{
    targetEl.click();
  }}
  const updated = targetEl.getAttribute('aria-checked') === 'true';
  if (updated !== desired) {{
    throw new Error(`target did not become ${{desired ? 'checked' : 'unchecked'}}`);
  }}
  return true;
"#
        ),
    )
}

/// Builds a script that scrolls one resolved ref into view.
pub(super) fn scroll_into_view_expression(target: &BrowserElementRef) -> Result<String> {
    ref_script(
        target,
        r#"  refElement.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
  return true;
"#,
    )
}

fn ref_script(target: &BrowserElementRef, body: &str) -> Result<String> {
    let target = serde_json::to_string(target)?;
    Ok(format!(
        r#"(() => {{
{helpers}
  const refTarget = {target};
  const refElement = findTarget(refTarget);
  if (!refElement) throw new Error(`No element matched browser ref ${{refTarget.ref}}`);
{body}
}})()"#,
        helpers = HELPERS,
        body = body
    ))
}
