//! Gmail draft JavaScript helpers.

use serde_json::{json, Value};

use super::GmailComposeFields;

/// Returns true when a Gmail list response contains the expected draft fields.
pub(super) fn draft_rows_contain(fields: &GmailComposeFields, result: &Value) -> bool {
    let rows = result
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if rows.is_empty() {
        return false;
    }
    let expected_subject = fields.subject.trim().to_ascii_lowercase();
    let expected_body = fields.body.trim().to_ascii_lowercase();
    let expected_recipients = fields
        .to
        .iter()
        .chain(fields.cc.iter())
        .chain(fields.bcc.iter())
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    rows.iter().any(|row| {
        let haystack = [
            row.get("sender").and_then(Value::as_str).unwrap_or(""),
            row.get("fromEmail").and_then(Value::as_str).unwrap_or(""),
            row.get("subject").and_then(Value::as_str).unwrap_or(""),
            row.get("snippet").and_then(Value::as_str).unwrap_or(""),
        ]
        .join(" ")
        .to_ascii_lowercase();
        (!expected_subject.is_empty() && haystack.contains(&expected_subject))
            || (!expected_body.is_empty()
                && haystack.contains(&expected_body.chars().take(80).collect::<String>()))
            || expected_recipients
                .iter()
                .any(|recipient| haystack.contains(recipient))
    })
}

/// Builds a Gmail compose autosave script for standalone draft windows.
pub(super) fn gmail_save_draft_script(fields: &GmailComposeFields) -> String {
    draft_script(fields, false)
}

/// Builds a Gmail autosave script that opens the reply composer when needed.
pub(super) fn gmail_reply_draft_script(fields: &GmailComposeFields) -> String {
    draft_script(fields, true)
}

/// Verifies that a reply draft is visible in the currently opened Gmail thread.
pub(super) fn gmail_reply_draft_verify_script(fields: &GmailComposeFields) -> String {
    let expected = draft_autosave_probe(fields);
    format!(
        r#"
(() => {{
  const expected = {expected};
  const normalize = (value) => String(value || "").replace(/\s+/g, " ").trim();
  const visible = (node) => {{
    if (!node) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }};
  const label = (node) => [
    node.getAttribute("aria-label") || "",
    node.getAttribute("data-tooltip") || "",
    node.getAttribute("title") || "",
    node.textContent || ""
  ].join(" ").trim();
  const expectedBody = normalize(expected.body || "");
  const expectedProbe = expectedBody.slice(0, 200);
  const editables = Array.from(document.querySelectorAll('[contenteditable="true"], textarea, input'))
    .filter(visible);
  const bodyCandidates = editables.filter((node) => {{
    const nodeLabel = label(node);
    const isSearch = /search mail|search all conversations/i.test(nodeLabel) ||
      node.getAttribute("name") === "q";
    if (isSearch) return false;
    return /message body|body/i.test(nodeLabel) ||
      (node.getAttribute("g_editable") === "true" && node.getAttribute("role") === "textbox") ||
      (node.getAttribute("contenteditable") === "true" &&
        !!node.closest('[role="dialog"], .AD, .nH'));
  }});
  const matchingBody = bodyCandidates.find((node) => {{
    const bodyText = normalize(node.innerText || node.textContent || node.value || "");
    return expectedProbe ? bodyText.includes(expectedProbe) : bodyText.length > 0;
  }});
  if (!matchingBody) {{
    return {{
      ok: false,
      status: "draft_body_not_visible",
      reason: "reply draft body was not visible in the thread composer",
      href: location.href,
      editableCount: editables.length,
      bodyCandidateCount: bodyCandidates.length
    }};
  }}
  const composeRoot =
    matchingBody.closest('[role="dialog"]') ||
    matchingBody.closest('.AD') ||
    matchingBody.closest('.nH') ||
    document.body;
  const bodyText = normalize(matchingBody.innerText || matchingBody.textContent || matchingBody.value || "");
  const rootText = normalize(composeRoot.innerText || "");
  if (/\bSaving\b|Saving draft/i.test(rootText) && !/saved|draft saved/i.test(rootText)) {{
    return {{
      ok: false,
      status: "saving",
      reason: "Gmail is still saving the reply draft",
      href: location.href,
      bodyLength: bodyText.length
    }};
  }}
  return {{
    ok: true,
    status: /saved|draft saved/i.test(rootText) ? "draft_autosaved" : "draft_present",
    href: location.href,
    bodyLength: bodyText.length,
    bodyCandidateCount: bodyCandidates.length
  }};
}})()
"#
    )
}

fn draft_autosave_probe(fields: &GmailComposeFields) -> Value {
    json!({
        "to": fields.to.clone(),
        "cc": fields.cc.clone(),
        "bcc": fields.bcc.clone(),
        "subject": fields.subject.clone(),
        "body": fields.body.clone(),
    })
}

fn draft_script(fields: &GmailComposeFields, reply_mode: bool) -> String {
    let expected = draft_autosave_probe(fields);
    let reply_block = if reply_mode {
        r#"
  const replyButtons = Array.from(document.querySelectorAll('[aria-label], [data-tooltip], [role="button"]'))
    .filter(visible)
    .filter((node) => /^reply\b/i.test(label(node)) && !/reply all/i.test(label(node)));
  if (replyButtons.length > 0) {
    replyButtons[replyButtons.length - 1].click();
    return { ok: false, status: "reply_opening", reason: "reply composer is opening" };
  }
"#
    } else {
        ""
    };
    format!(
        r#"
(() => {{
  const expected = {expected};
  const visible = (node) => {{
    if (!node) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }};
  const label = (node) => [
    node.getAttribute("aria-label") || "",
    node.getAttribute("data-tooltip") || "",
    node.getAttribute("title") || "",
    node.textContent || ""
  ].join(" ").trim();
  const normalize = (value) => String(value || "").replace(/\s+/g, " ").trim();
  const setEditableValue = (node, value) => {{
    node.focus();
    if ("value" in node) {{
      node.value = value;
    }} else {{
      node.textContent = value;
    }}
    node.dispatchEvent(new InputEvent("input", {{
      bubbles: true,
      inputType: "insertText",
      data: value
    }}));
    node.dispatchEvent(new Event("change", {{ bubbles: true }}));
  }};
  const editables = Array.from(document.querySelectorAll('[contenteditable="true"], textarea, input'))
    .filter(visible);
  const bodyNode = editables.find((node) =>
    /message body|body/i.test(label(node)) ||
    (node.getAttribute("g_editable") === "true" && node.getAttribute("role") === "textbox")
  ) || editables.find((node) => node.getAttribute("contenteditable") === "true");
  if (!bodyNode) {{{reply_block}
    return {{ ok: false, status: "compose_loading", reason: "message body not visible" }};
  }}
  const composeRoot =
    bodyNode.closest('[role="dialog"]') ||
    bodyNode.closest('.AD') ||
    bodyNode.closest('.nH') ||
    document.body;
  const bodyText = normalize(bodyNode.innerText || bodyNode.textContent || bodyNode.value || "");
  const expectedBody = normalize(expected.body || "");
  if (expectedBody && !bodyText.includes(expectedBody.slice(0, 200))) {{
    setEditableValue(bodyNode, expected.body);
    window.__pufferDraftStableSince = 0;
    return {{ ok: false, status: "compose_populated", reason: "draft body was populated" }};
  }}
  const subjectNode =
    composeRoot.querySelector('input[name="subjectbox"]') ||
    Array.from(composeRoot.querySelectorAll('input, textarea, [contenteditable="true"]'))
      .find((node) => /subject/i.test(label(node)));
  const subjectText = subjectNode
    ? normalize(subjectNode.value || subjectNode.innerText || subjectNode.textContent || "")
    : "";
  const expectedSubject = normalize(expected.subject || "");
  if (expectedSubject && !subjectText.includes(expectedSubject)) {{
    if (!subjectNode) {{
      return {{ ok: false, status: "compose_loading", reason: "subject field not visible" }};
    }}
    setEditableValue(subjectNode, expected.subject);
    window.__pufferDraftStableSince = 0;
    return {{ ok: false, status: "compose_populated", reason: "draft subject was populated" }};
  }}
  const rootText = normalize(composeRoot.innerText || "");
  if (/\bSaving\b|Saving draft/i.test(rootText)) {{
    window.__pufferDraftStableSince = 0;
    return {{ ok: false, status: "saving", reason: "Gmail is still saving the draft" }};
  }}
  if (/saved|draft saved/i.test(rootText)) {{
    return {{ ok: true, status: "draft_autosaved", bodyLength: bodyText.length, subject: subjectText }};
  }}
  window.__pufferDraftStableSince ||= Date.now();
  if (Date.now() - window.__pufferDraftStableSince < 3000) {{
    return {{ ok: false, status: "autosave_settling", reason: "waiting for Gmail autosave state" }};
  }}
  return {{ ok: true, status: "draft_autosaved", bodyLength: bodyText.length, subject: subjectText }};
}})()
"#
    )
}
