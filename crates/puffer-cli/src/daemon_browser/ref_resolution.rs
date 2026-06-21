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
  const selector = 'a,button,input,textarea,select,summary,iframe,[role],[contenteditable="true"],[tabindex],label';
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
    // Exact handles stashed by the last snapshot beat signature matching:
    // signature lookup depends on snapshot-time viewport coordinates, which
    // every scrollIntoView invalidates, and indistinguishable elements (four
    // anonymous hosted-payment-field containers) then resolve to the wrong
    // node or to nothing (#633).
    const storedRefs = window.__puffer_agent_refs__;
    const stored = storedRefs && storedRefs.byRef ? storedRefs.byRef[target.ref] : null;
    if (stored && stored.isConnected) {
      const style = getComputedStyle(stored);
      if (style.display !== 'none' && style.visibility !== 'hidden') return stored;
    }
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
  refElement.scrollIntoView({{ block: 'center', inline: 'center', behavior: 'instant' }});
  const rect = refElement.getBoundingClientRect();
  const x = Math.min(Math.max(rect.left + rect.width / 2, 0), Math.max(window.innerWidth - 1, 0));
  const y = Math.min(Math.max(rect.top + rect.height / 2, 0), Math.max(window.innerHeight - 1, 0));
  if (!Number.isFinite(x) || !Number.isFinite(y)) throw new Error('Target has no stable viewport point');
  return {{ x, y }};
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

/// Builds a script that fills one resolved ref.
pub(super) fn fill_expression(target: &BrowserElementRef, text: &str) -> Result<String> {
    let text = serde_json::to_string(text)?;
    ref_script(
        target,
        &format!(
            r#"  const targetEl = refElement.closest('input, textarea, [contenteditable="true"]') || refElement;
  // A hosted payment field (e.g. Shopify/Stripe PCI card fields) renders its
  // real <input> inside a cross-origin iframe the top document can't reach.
  // Setting a value on the shell would silently fill the wrong element and
  // report success — that's how #580 placed orders with no card. Hand the
  // fill back to the runtime instead: it focuses the frame with a real mouse
  // click and types through trusted browser input (#633).
  const editable = ('value' in targetEl) || targetEl.isContentEditable;
  const hostedFrame = targetEl.tagName === 'IFRAME'
    ? targetEl
    : (!editable && targetEl.querySelector ? targetEl.querySelector('iframe') : null);
  if (hostedFrame) {{
    hostedFrame.scrollIntoView({{ block: 'center', inline: 'center', behavior: 'instant' }});
    const rect = hostedFrame.getBoundingClientRect();
    if (!(rect.width > 1 && rect.height > 1)) {{
      throw new Error('Target resolves to a hosted field iframe with no clickable area');
    }}
    window.__puffer_hosted_fill__ = {{ frame: hostedFrame }};
    const x = Math.min(Math.max(rect.left + rect.width / 2, 0), Math.max(window.innerWidth - 1, 0));
    const y = Math.min(Math.max(rect.top + rect.height / 2, 0), Math.max(window.innerHeight - 1, 0));
    return {{ hostedFrameFill: true, x, y }};
  }}
  const expected = {text};
  if ('value' in targetEl) {{
    const prototype = targetEl instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
    const descriptor = Object.getOwnPropertyDescriptor(prototype, 'value');
    if (descriptor && descriptor.set) {{
      descriptor.set.call(targetEl, expected);
    }} else {{
      targetEl.value = expected;
    }}
    targetEl.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: expected }}));
    targetEl.dispatchEvent(new Event('change', {{ bubbles: true }}));
    // Read the value back: a framework-controlled or guarded input can ignore
    // the programmatic set and stay empty. Surface that rather than claiming
    // the field was filled when it wasn't (#580). Check for an empty result
    // only — many inputs legitimately reformat on input (card numbers add
    // spaces, phone numbers add separators), so requiring an exact match would
    // turn working fills into false failures.
    if (expected !== '' && targetEl.value === '') {{
      throw new Error('fill failed: value did not stick (the field may be inside a cross-origin iframe or guarded by the page)');
    }}
  }} else if (targetEl.isContentEditable) {{
    targetEl.textContent = expected;
    targetEl.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: expected }}));
    if (expected !== '' && (targetEl.textContent ?? '') === '') {{
      throw new Error('fill failed: value did not stick (the field may be inside a cross-origin iframe or guarded by the page)');
    }}
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

/// Builds the script that checks whether focus reached the pending hosted
/// field iframe stashed by [`fill_expression`].
pub(super) fn hosted_fill_focus_check_expression() -> &'static str {
    r#"(() => {
  const pending = window.__puffer_hosted_fill__;
  if (!pending || !pending.frame || !pending.frame.isConnected) {
    return { focused: false, reason: 'no pending hosted fill frame' };
  }
  return { focused: document.activeElement === pending.frame };
})()"#
}

/// Builds the script that re-reads a fresh viewport center point for the
/// pending hosted fill frame. Used between click retries: right after a
/// scroll the browser-side hit test can lag the new layout by a frame, so a
/// click at correct coordinates may still land in the parent document.
pub(super) fn hosted_fill_point_expression() -> &'static str {
    r#"(() => {
  const pending = window.__puffer_hosted_fill__;
  if (!pending || !pending.frame || !pending.frame.isConnected) {
    throw new Error('no pending hosted fill frame');
  }
  pending.frame.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
  const rect = pending.frame.getBoundingClientRect();
  const x = Math.min(Math.max(rect.left + rect.width / 2, 0), Math.max(window.innerWidth - 1, 0));
  const y = Math.min(Math.max(rect.top + rect.height / 2, 0), Math.max(window.innerHeight - 1, 0));
  return { x, y };
})()"#
}

/// `Runtime.callFunctionOn` body, run on a node resolved INSIDE a cross-origin
/// payment iframe (#656). Confirms focus actually landed on this field — the
/// only #580 guard available cross-origin — then clears any existing content so
/// the runtime's per-character keystroke typing replaces rather than appends.
/// `this` is the resolved field; returns `{ focused, tag, origin }`.
pub(super) fn in_frame_prepare_fill_fn() -> &'static str {
    r#"function() {
  const focused = document.activeElement === this;
  if (focused && 'value' in this) {
    const proto = this instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    const descriptor = Object.getOwnPropertyDescriptor(proto, 'value');
    if (descriptor && descriptor.set) { descriptor.set.call(this, ''); } else { this.value = ''; }
    this.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'deleteContentBackward' }));
  } else if (focused && this.isContentEditable) {
    this.textContent = '';
    this.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'deleteContentBackward' }));
  }
  return { focused, tag: this.tagName, origin: location.origin };
}"#
}

/// `Runtime.callFunctionOn` body that reads a field's value back after a fill.
/// Unlike a top-document hosted fill, an in-frame field CAN be read back, so a
/// fill that silently did not stick is caught (#580). Falls back to
/// `textContent` for contenteditable fields, which have no `value`. Returns
/// `{ value }`.
pub(super) fn in_frame_readback_fn() -> &'static str {
    r#"function() { return { value: ('value' in this) ? String(this.value ?? '') : String(this.textContent ?? '') }; }"#
}

/// `Runtime.callFunctionOn` body that selects an option on a native `<select>`
/// resolved inside a cross-origin payment iframe (the expiry month/year on the
/// Amazon card form). A native popup can't be driven by coordinate clicks, so
/// the value is set programmatically and `input`/`change` are fired for the
/// widget to observe. Matches by option value or visible label. Returns
/// `{ matched, value | available }`.
pub(super) fn in_frame_select_fn(value: &str) -> Result<String> {
    let value = serde_json::to_string(value)?;
    Ok(format!(
        r#"function() {{
  const normalize = (v) => String(v ?? '').trim();
  const requested = normalize({value});
  const options = Array.from(this.options || []);
  const match = options.find((option) => {{
    const optionValue = normalize(option.value);
    const optionLabel = normalize(option.label || option.textContent || option.value);
    return optionValue === requested || optionLabel === requested;
  }});
  if (!match) {{
    const available = options.slice(0, 16).map((o) => normalize(o.label || o.textContent || o.value)).filter(Boolean).join(', ');
    return {{ matched: false, available }};
  }}
  for (const option of options) option.selected = option === match;
  this.value = match.value;
  this.dispatchEvent(new Event('input', {{ bubbles: true }}));
  this.dispatchEvent(new Event('change', {{ bubbles: true }}));
  return {{ matched: true, value: match.value }};
}}"#
    ))
}

/// `Runtime.callFunctionOn` body that sets the checked state of a checkbox/radio
/// resolved inside a cross-origin payment iframe, firing `input`/`change`.
pub(super) fn in_frame_set_checked_fn(checked: bool) -> String {
    format!(
        r#"function() {{
  if (!('checked' in this)) {{ return {{ ok: false }}; }}
  this.checked = {checked};
  this.dispatchEvent(new Event('input', {{ bubbles: true }}));
  this.dispatchEvent(new Event('change', {{ bubbles: true }}));
  return {{ ok: true }};
}}"#
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
