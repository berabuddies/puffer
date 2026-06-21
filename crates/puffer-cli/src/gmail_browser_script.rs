//! Browser-side Gmail inbox polling script.

/// JavaScript evaluated inside the Gmail inbox tab to extract visible rows.
pub(crate) const GMAIL_INBOX_SCRIPT: &str = r#"
(() => {
  const href = location.href;
  const title = document.title || "";
  const bodyText = document.body ? document.body.innerText || "" : "";
  const host = location.hostname || "";
  const signinLike =
    host.includes("accounts.google.com") ||
    /ServiceLogin|signin|identifier/.test(href) ||
    (/sign in/i.test(title) && !/gmail/i.test(title));
  if (signinLike) {
    return { status: "auth_required", href, title, rows: [] };
  }
  const temporaryError =
    /temporary error/i.test(title) ||
    /temporarily unavailable/i.test(bodyText) ||
    /Temporary Error/.test(bodyText);
  if (temporaryError) {
    return { status: "temporary_error", href, title, bodyText: bodyText.slice(0, 200), rows: [] };
  }
  const visible = (node) => {
    if (!node) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const text = (node) => (node && node.textContent ? node.textContent.trim().replace(/\s+/g, " ") : "");
  const rowHasAttachment = (row, aria) => {
    if (aria.includes("attachment")) return true;
    if (row.querySelector(".aZo, .brg")) return true;
    const labeled = Array.from(row.querySelectorAll("[aria-label], [data-tooltip], [title]"));
    return labeled.some((node) => {
      const label = [
        node.getAttribute("aria-label") || "",
        node.getAttribute("data-tooltip") || "",
        node.getAttribute("title") || ""
      ].join(" ");
      return /attachment/i.test(label);
    });
  };
  const candidateRows = Array.from(document.querySelectorAll('tr[role="row"]'));
  const visibleRows = candidateRows.filter(visible);
  const rows = visibleRows
    .slice(0, 75)
    .map((row, index) => {
      const fromEl = row.querySelector('.yW span[email], span[email], .yX.xY .yW span');
      const subjectEl = row.querySelector('.bog, span[data-thread-id], .y6 span[id]');
      const snippetEl = row.querySelector('.y2, span[data-thread-id] + span');
      const idEl = row.querySelector('[data-legacy-thread-id], [data-thread-id], [data-legacy-message-id]');
      const legacyThreadId =
        row.getAttribute("data-legacy-thread-id") ||
        (idEl && idEl.getAttribute("data-legacy-thread-id")) ||
        "";
      const rawThreadId =
        row.getAttribute("data-thread-id") ||
        (idEl && idEl.getAttribute("data-thread-id")) ||
        "";
      const threadId = legacyThreadId || rawThreadId.replace(/^#/, "");
      const messageId =
        row.getAttribute("data-legacy-message-id") ||
        (idEl && idEl.getAttribute("data-legacy-message-id")) ||
        row.getAttribute("data-message-id") ||
        legacyThreadId ||
        threadId ||
        row.getAttribute("data-id") ||
        "";
      const sender =
        (fromEl && (fromEl.getAttribute("name") || fromEl.getAttribute("aria-label"))) ||
        text(fromEl);
      const fromEmail = (fromEl && fromEl.getAttribute("email")) || "";
      const subject = text(subjectEl);
      const snippet = text(snippetEl);
      const aria = (row.getAttribute("aria-label") || "").toLowerCase();
      const unread =
        row.classList.contains("zE") ||
        row.querySelector(".zF") !== null ||
        aria.includes("unread");
      const hasAttachment = rowHasAttachment(row, aria);
      const fallback = [sender, subject, snippet, index].join(":");
      return {
        id: messageId || fallback,
        threadId,
        legacyThreadId,
        gmailThreadId: rawThreadId,
        sender,
        fromEmail,
        subject,
        snippet,
        unread,
        hasAttachment,
        url: href,
        index
      };
    })
    .filter((row) => row.id && (row.sender || row.subject || row.snippet || row.unread));
  const empty =
    /no conversations/i.test(bodyText) ||
    /inbox is empty/i.test(bodyText) ||
    /no mail/i.test(bodyText);
  const status = rows.length > 0 || empty ? "ok" : "loading";
  return {
    status,
    href,
    title,
    bodyText: bodyText.slice(0, 200),
    empty,
    rows,
    candidateRowCount: candidateRows.length,
    visibleRowCount: visibleRows.length,
    filteredRowCount: rows.length,
    selectorVersion: "2026-06-04"
  };
})()
"#;
