//! Agent snapshot and screenshot helpers for managed browser tabs.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

use super::params::optional_u32;
use super::{BrowserRegistry, BrowserSession};

const SNAPSHOT_INSTRUCTION: &str =
    "Refs are fresh for this snapshot. Re-snapshot after navigation or dynamic page changes.";
const ANNOTATED_SCREENSHOT_INSTRUCTION: &str =
    "Refs are fresh for this annotated screenshot. Re-annotate or re-snapshot after navigation or dynamic page changes.";
const SCREENSHOT_OVERLAY_ID: &str = "__puffer_screenshot_overlay__";

/// Element reference captured from the last agent browser snapshot.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserElementRef {
    #[serde(rename = "ref")]
    pub(crate) ref_id: String,
    pub(crate) role: String,
    pub(crate) name: String,
    pub(crate) tag: String,
    #[serde(default)]
    pub(crate) href: Option<String>,
    pub(crate) x: f64,
    pub(crate) y: f64,
    // Internal routing metadata for a field living inside a cross-origin payment
    // iframe (#656). Never serialized: the agent sees a plain ref, and these
    // never round-trip through the top-frame snapshot JSON. The runtime fills the
    // field by CDP node identity (`DOM.resolveNode` resolves `backend_node_id`
    // into its owning frame's context) instead of a top-document handle.
    #[serde(skip)]
    pub(crate) in_frame: bool,
    #[serde(skip)]
    pub(crate) backend_node_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserSnapshot {
    url: String,
    title: String,
    text: String,
    elements: Vec<BrowserElementRef>,
}

/// A fillable/actionable field discovered inside a cross-origin payment iframe
/// by piercing the frame tree with `DOM.getDocument`. The top document cannot
/// see into the OOPIF, so the runtime addresses these fields by CDP node
/// identity (`backend_node_id`, which `DOM.resolveNode` resolves into the
/// owning frame) instead of a live JS handle.
#[derive(Clone, Debug)]
pub(crate) struct InFrameFieldNode {
    pub(crate) backend_node_id: i64,
    pub(crate) role: String,
    pub(crate) name: String,
    pub(crate) tag: String,
}

/// Parses a `DOM.getDocument` node's flat `[k, v, k, v, ...]` attribute array.
fn dom_attributes(node: &Value) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(arr) = node.get("attributes").and_then(Value::as_array) {
        let mut i = 0;
        while i + 1 < arr.len() {
            if let (Some(key), Some(value)) = (arr[i].as_str(), arr[i + 1].as_str()) {
                map.insert(key.to_ascii_lowercase(), value.to_string());
            }
            i += 2;
        }
    }
    map
}

fn dom_children(node: &Value) -> &[Value] {
    node.get("children")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Splits camelCase / snake_case / kebab-case identifiers into a readable label.
fn humanize_identifier(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for ch in raw.chars() {
        if ch == '_' || ch == '-' || ch == ':' || ch == '.' {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
            prev_lower = false;
            continue;
        }
        if ch.is_ascii_uppercase() && prev_lower {
            out.push(' ');
        }
        out.push(ch);
        prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Derives an agent-facing name for an in-frame field from its attributes.
fn derive_in_frame_name(attrs: &HashMap<String, String>, tag: &str) -> String {
    for key in ["aria-label", "placeholder", "title", "name", "id"] {
        if let Some(value) = attrs.get(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return humanize_identifier(trimmed);
            }
        }
    }
    tag.to_string()
}

/// Classifies one DOM node into an in-frame field (role, fillable). Returns
/// `None` for hidden inputs and non-fillable elements.
fn classify_field_node(node: &Value) -> Option<InFrameFieldNode> {
    let tag = node.get("nodeName").and_then(Value::as_str)?.to_ascii_lowercase();
    let backend_node_id = node.get("backendNodeId").and_then(Value::as_i64)?;
    let attrs = dom_attributes(node);
    let role = match tag.as_str() {
        "input" => {
            let input_type = attrs
                .get("type")
                .map(String::as_str)
                .unwrap_or("text")
                .to_ascii_lowercase();
            match input_type.as_str() {
                // Not text-fillable: skip so the agent never types into them.
                "hidden" | "file" => return None,
                "submit" | "button" | "image" | "reset" => "button",
                "checkbox" => "checkbox",
                "radio" => "radio",
                // text / tel / number / email / password / search / url / date / ...
                _ => "textbox",
            }
        }
        "select" => "combobox",
        "textarea" => "textbox",
        "button" => "button",
        _ => {
            let editable = attrs
                .get("contenteditable")
                .map(|value| value != "false")
                .unwrap_or(false);
            if editable {
                "textbox"
            } else {
                return None;
            }
        }
    };
    Some(InFrameFieldNode {
        backend_node_id,
        role: role.to_string(),
        name: derive_in_frame_name(&attrs, &tag),
        tag,
    })
}

/// Walks a `DOM.getDocument { depth: -1, pierce: true }` tree and collects the
/// fillable fields that live inside one of `payment_frame_ids`. Descends into
/// iframe `contentDocument`s, switching the active frame at each boundary, so a
/// field is only surfaced when its nearest enclosing frame is a gated payment
/// frame. Hidden inputs and fields outside the gated frames are ignored.
pub(crate) fn collect_in_frame_field_nodes(
    doc_root: &Value,
    payment_frame_ids: &HashSet<String>,
) -> Vec<InFrameFieldNode> {
    let mut out = Vec::new();
    walk_dom_for_fields(doc_root, None, payment_frame_ids, &mut out);
    out
}

/// True when an iframe element carries an accessible name (title / aria-label /
/// alt). Such iframes are already surfaced as refs by the top-document snapshot
/// and filled by `hosted_frame_fill` (Stripe/Shopify single-field frames), so
/// the deep pass must leave them alone to avoid duplicate refs.
fn iframe_has_accessible_name(node: &Value) -> bool {
    let attrs = dom_attributes(node);
    ["title", "aria-label", "alt"]
        .iter()
        .any(|key| attrs.get(*key).map(|v| !v.trim().is_empty()).unwrap_or(false))
}

fn walk_dom_for_fields(
    node: &Value,
    current_frame: Option<&str>,
    payment_frame_ids: &HashSet<String>,
    out: &mut Vec<InFrameFieldNode>,
) {
    let node_name = node
        .get("nodeName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if node_name.eq_ignore_ascii_case("iframe") {
        if let Some(content) = node.get("contentDocument") {
            let frame_id = node.get("frameId").and_then(Value::as_str);
            // Only collect fields when this iframe is a gated payment frame that
            // the titled-iframe path can't already handle (i.e. it is untitled).
            let is_payment = frame_id
                .map(|frame| payment_frame_ids.contains(frame))
                .unwrap_or(false);
            let active = if is_payment && !iframe_has_accessible_name(node) {
                frame_id
            } else {
                None
            };
            walk_dom_for_fields(content, active, payment_frame_ids, out);
        }
        // Light-DOM children (rare) stay in the parent frame.
        for child in dom_children(node) {
            walk_dom_for_fields(child, current_frame, payment_frame_ids, out);
        }
        return;
    }
    if let Some(frame) = current_frame {
        if payment_frame_ids.contains(frame) {
            if let Some(field) = classify_field_node(node) {
                out.push(field);
            }
        }
    }
    for child in dom_children(node) {
        walk_dom_for_fields(child, current_frame, payment_frame_ids, out);
    }
}

/// Hosts of payment iframes whose card form is reachable by the same-process
/// deep pass — i.e. the iframe is cross-ORIGIN but same-SITE as the page, so
/// `DOM.getDocument { pierce }` can descend into it. Amazon's APX iframe
/// (apx-security.amazon.com embedded in www.amazon.com) is the canonical case.
/// Matched on a domain boundary (exact host or a real subdomain), never as a
/// raw substring. Cross-SITE processors (Stripe/Shopify/etc.) are intentionally
/// absent: their card frames are true OOPIFs this pass can't reach, and they
/// already work via the titled single-field hosted-fill path.
const PAYMENT_FRAME_HOSTS: &[&str] = &["apx-security.amazon.com", "payments.amazon.com"];

/// Frame `name` substrings (case-insensitive) of secure payment iframes. This is
/// the primary gate (host may vary); complements [`PAYMENT_FRAME_HOSTS`].
const PAYMENT_FRAME_NAME_PATTERNS: &[&str] = &["apxsecureiframe", "securepaymentframe"];

fn host_of(url: &str) -> Option<String> {
    let after_scheme = url.split("://").nth(1)?;
    let authority = after_scheme.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// True when `host` is exactly `pattern` or a subdomain of it — anchored to a
/// label boundary so `notpaypal.com` / `apx-security.amazon.com.evil.com` never
/// match `paypal.com` / `apx-security.amazon.com`.
fn host_matches(host: &str, pattern: &str) -> bool {
    host == pattern || host.ends_with(&format!(".{pattern}"))
}

fn frame_is_payment(url: &str, name: &str) -> bool {
    if let Some(host) = host_of(url) {
        if PAYMENT_FRAME_HOSTS
            .iter()
            .any(|pattern| host_matches(&host, pattern))
        {
            return true;
        }
    }
    let name = name.to_ascii_lowercase();
    PAYMENT_FRAME_NAME_PATTERNS
        .iter()
        .any(|pattern| name.contains(pattern))
}

/// Walks a `Page.getFrameTree` result and returns the ids of child frames that
/// look like third-party payment iframes. The root (main) frame is never
/// included. This is the cheap gate that decides whether the deep pierce runs.
pub(crate) fn payment_frame_ids_from_tree(frame_tree_result: &Value) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(root) = frame_tree_result.get("frameTree") else {
        return out;
    };
    collect_payment_frames(root, true, &mut out);
    out
}

fn collect_payment_frames(tree_node: &Value, is_root: bool, out: &mut HashSet<String>) {
    if let Some(frame) = tree_node.get("frame") {
        let id = frame.get("id").and_then(Value::as_str).unwrap_or_default();
        let url = frame.get("url").and_then(Value::as_str).unwrap_or_default();
        let name = frame.get("name").and_then(Value::as_str).unwrap_or_default();
        if !is_root && !id.is_empty() && frame_is_payment(url, name) {
            out.insert(id.to_string());
        }
    }
    if let Some(children) = tree_node.get("childFrames").and_then(Value::as_array) {
        for child in children {
            collect_payment_frames(child, false, out);
        }
    }
}

/// Promotes collected in-frame fields to agent refs, numbering them after the
/// top-document refs (`start_index`). A field without a resolved top-viewport
/// quad center is dropped: a ref the runtime can't click is worse than none.
pub(crate) fn field_nodes_to_refs(
    nodes: &[InFrameFieldNode],
    quad_centers: &HashMap<i64, (f64, f64)>,
    start_index: usize,
) -> Vec<BrowserElementRef> {
    let mut refs = Vec::new();
    let mut index = start_index;
    for node in nodes {
        let Some(&(x, y)) = quad_centers.get(&node.backend_node_id) else {
            continue;
        };
        index += 1;
        refs.push(BrowserElementRef {
            ref_id: format!("@e{index}"),
            role: node.role.clone(),
            name: node.name.clone(),
            tag: node.tag.clone(),
            href: None,
            x,
            y,
            in_frame: true,
            backend_node_id: Some(node.backend_node_id),
        });
    }
    refs
}

/// Averages a `DOM.getContentQuads` first quad `[x1,y1,...,x4,y4]` into its
/// top-viewport center. Returns `None` for a node with no rendered geometry,
/// which drops it from the snapshot.
fn content_quad_center(session: &BrowserSession, backend_node_id: i64) -> Option<(f64, f64)> {
    let result = session
        .cdp_call(
            "DOM.getContentQuads",
            json!({ "backendNodeId": backend_node_id }),
        )
        .ok()?;
    let quad = result.get("quads")?.as_array()?.first()?.as_array()?;
    if quad.len() < 8 {
        return None;
    }
    let coord = |i: usize| quad.get(i).and_then(Value::as_f64);
    let mut sx = 0.0;
    let mut sy = 0.0;
    for corner in 0..4 {
        sx += coord(corner * 2)?;
        sy += coord(corner * 2 + 1)?;
    }
    Some((sx / 4.0, sy / 4.0))
}

/// Surfaces fillable fields inside cross-origin payment iframes as agent refs
/// (#656). Gated cheaply on the frame tree so the expensive pierce only runs on
/// pages with a third-party payment iframe. Returns refs numbered after
/// `start_index`; the caller appends them to the top-document refs.
fn deep_snapshot_payment_iframes(
    session: &BrowserSession,
    start_index: usize,
) -> Result<Vec<BrowserElementRef>> {
    let tree = session.cdp_call("Page.getFrameTree", json!({}))?;
    let payment_frames = payment_frame_ids_from_tree(&tree);
    if payment_frames.is_empty() {
        return Ok(Vec::new());
    }
    let document = session.cdp_call("DOM.getDocument", json!({ "depth": -1, "pierce": true }))?;
    let root = document.get("root").cloned().unwrap_or(Value::Null);
    let nodes = collect_in_frame_field_nodes(&root, &payment_frames);
    if nodes.is_empty() {
        return Ok(Vec::new());
    }
    let mut quad_centers = HashMap::new();
    for node in &nodes {
        if let Some(center) = content_quad_center(session, node.backend_node_id) {
            quad_centers.insert(node.backend_node_id, center);
        }
    }
    Ok(field_nodes_to_refs(&nodes, &quad_centers, start_index))
}

/// Screenshot format supported by the managed browser worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BrowserScreenshotFormat {
    Png,
    Jpeg,
}

impl BrowserScreenshotFormat {
    fn as_cdp_value(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
        }
    }

    fn as_str(self) -> &'static str {
        self.as_cdp_value()
    }
}

/// Capture-only screenshot settings for the browser worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BrowserCaptureScreenshotOptions {
    pub(super) format: BrowserScreenshotFormat,
    pub(super) quality: Option<u8>,
}

/// Agent-facing screenshot options, including temporary page annotations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BrowserAgentScreenshotOptions {
    pub(super) annotate: bool,
    pub(super) capture: BrowserCaptureScreenshotOptions,
}

/// Raw screenshot bytes returned from the worker as base64 plus capture format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct BrowserCapturedScreenshot {
    pub(super) data: String,
    pub(super) format: BrowserScreenshotFormat,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserAgentScreenshot {
    tab_id: String,
    format: String,
    data: String,
    url: String,
    title: String,
    width: u32,
    height: u32,
    annotated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    elements: Vec<BrowserElementRef>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    instruction: String,
}

/// Parses one agent screenshot request payload into validated capture options.
pub(super) fn parse_agent_screenshot_options(
    params: &Value,
) -> Result<BrowserAgentScreenshotOptions> {
    let format = parse_screenshot_format(
        params
            .get("screenshotFormat")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    )?;
    let quality = parse_screenshot_quality(params)?;
    if quality.is_some() && format != BrowserScreenshotFormat::Jpeg {
        bail!("`screenshotQuality` requires `screenshotFormat` `jpeg`");
    }
    Ok(BrowserAgentScreenshotOptions {
        annotate: params
            .get("annotate")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        capture: BrowserCaptureScreenshotOptions { format, quality },
    })
}

/// Builds the Chrome DevTools parameters for one still screenshot capture.
pub(super) fn capture_screenshot_command_params(options: BrowserCaptureScreenshotOptions) -> Value {
    let mut params = Map::new();
    params.insert(
        "format".to_string(),
        Value::String(options.format.as_cdp_value().to_string()),
    );
    if let Some(quality) = options.quality {
        params.insert("quality".to_string(), Value::from(quality));
    }
    Value::Object(params)
}

/// Parses one `Page.captureScreenshot` response into the worker screenshot shape.
pub(super) fn parse_capture_screenshot_response(
    value: &Value,
    format: BrowserScreenshotFormat,
) -> Result<BrowserCapturedScreenshot> {
    let data = value
        .pointer("/result/data")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .context("browser screenshot returned no image data")?;
    Ok(BrowserCapturedScreenshot { data, format })
}

impl BrowserRegistry {
    /// Captures an agent-readable DOM snapshot and fresh element refs.
    pub(super) fn agent_snapshot(&self, backend_session_id: &str) -> Result<Value> {
        let session = self.get(backend_session_id)?;
        let snapshot_value = session.evaluate(snapshot_expression().to_string())?.value;
        let snapshot: BrowserSnapshot =
            serde_json::from_value(snapshot_value).context("decode browser snapshot")?;
        let mut elements = snapshot.elements;
        // Deep pass: surface fillable fields inside cross-origin payment iframes
        // the top document can't reach (#656). Best-effort — a failure here must
        // never break the ordinary snapshot.
        match deep_snapshot_payment_iframes(&session, elements.len()) {
            Ok(extra) => elements.extend(extra),
            Err(error) => {
                tracing::debug!(target: "puffer::browser", %error, "payment-iframe deep snapshot skipped");
            }
        }
        self.agent_refs
            .lock()
            .unwrap()
            .insert(backend_session_id.to_string(), elements.clone());
        Ok(json!({
            "url": snapshot.url,
            "title": snapshot.title,
            "text": snapshot.text,
            "elements": elements,
            "instruction": SNAPSHOT_INSTRUCTION
        }))
    }

    /// Captures one still screenshot, optionally with fresh `@eN` annotations.
    pub(super) fn agent_screenshot(
        &self,
        backend_session_id: &str,
        tab_id: &str,
        options: BrowserAgentScreenshotOptions,
    ) -> Result<Value> {
        let session = self.get(backend_session_id)?;
        let state = session.state();
        if !options.annotate {
            let screenshot = session.capture_screenshot(options.capture)?;
            return Ok(serde_json::to_value(BrowserAgentScreenshot {
                tab_id: tab_id.to_string(),
                format: screenshot.format.as_str().to_string(),
                data: screenshot.data,
                url: state.url,
                title: state.title,
                width: state.width,
                height: state.height,
                annotated: false,
                elements: Vec::new(),
                instruction: String::new(),
            })?);
        }

        let snapshot = capture_snapshot(&session)?;
        session.evaluate(screenshot_annotation_expression(&snapshot.elements)?)?;
        let capture_result = session.capture_screenshot(options.capture);
        let cleanup_result =
            session.evaluate(remove_screenshot_annotation_expression().to_string());
        let screenshot = capture_result?;
        cleanup_result.context("remove screenshot annotation overlay")?;
        self.agent_refs
            .lock()
            .unwrap()
            .insert(backend_session_id.to_string(), snapshot.elements.clone());
        Ok(serde_json::to_value(BrowserAgentScreenshot {
            tab_id: tab_id.to_string(),
            format: screenshot.format.as_str().to_string(),
            data: screenshot.data,
            url: snapshot.url,
            title: snapshot.title,
            width: state.width,
            height: state.height,
            annotated: true,
            elements: snapshot.elements,
            instruction: ANNOTATED_SCREENSHOT_INSTRUCTION.to_string(),
        })?)
    }
}

fn capture_snapshot(session: &BrowserSession) -> Result<BrowserSnapshot> {
    let snapshot_value = session.evaluate(snapshot_expression().to_string())?.value;
    serde_json::from_value(snapshot_value).context("decode browser snapshot")
}

fn parse_screenshot_format(raw: Option<&str>) -> Result<BrowserScreenshotFormat> {
    match raw.unwrap_or("png") {
        "png" => Ok(BrowserScreenshotFormat::Png),
        "jpeg" => Ok(BrowserScreenshotFormat::Jpeg),
        other => bail!("unsupported screenshot format `{other}`; use png or jpeg"),
    }
}

fn parse_screenshot_quality(params: &Value) -> Result<Option<u8>> {
    let Some(quality) = optional_u32(params, "screenshotQuality") else {
        return Ok(None);
    };
    if quality > 100 {
        bail!("`screenshotQuality` must be between 0 and 100");
    }
    Ok(Some(quality as u8))
}

fn screenshot_annotation_expression(elements: &[BrowserElementRef]) -> Result<String> {
    let refs = serde_json::to_string(elements)?;
    Ok(format!(
        r#"(() => {{
  const overlayId = "{SCREENSHOT_OVERLAY_ID}";
  const existing = document.getElementById(overlayId);
  if (existing) existing.remove();
  const overlay = document.createElement('div');
  overlay.id = overlayId;
  Object.assign(overlay.style, {{
    position: 'fixed',
    inset: '0',
    pointerEvents: 'none',
    zIndex: '2147483647',
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace'
  }});
  const refs = {refs};
  for (const ref of refs) {{
    const dot = document.createElement('div');
    Object.assign(dot.style, {{
      position: 'fixed',
      left: `${{Math.max(0, ref.x)}}px`,
      top: `${{Math.max(0, ref.y)}}px`,
      transform: 'translate(-50%, -50%)',
      width: '12px',
      height: '12px',
      borderRadius: '999px',
      background: '#d92d20',
      border: '2px solid #ffffff',
      boxShadow: '0 2px 8px rgba(0, 0, 0, 0.35)'
    }});
    const label = document.createElement('div');
    label.textContent = ref.ref;
    Object.assign(label.style, {{
      position: 'fixed',
      left: `${{Math.max(0, ref.x + 10)}}px`,
      top: `${{Math.max(0, ref.y - 10)}}px`,
      transform: 'translateY(-100%)',
      padding: '2px 6px',
      borderRadius: '999px',
      background: '#111827',
      color: '#ffffff',
      fontSize: '12px',
      fontWeight: '700',
      lineHeight: '1.2',
      whiteSpace: 'nowrap',
      boxShadow: '0 2px 8px rgba(0, 0, 0, 0.25)'
    }});
    overlay.append(dot, label);
  }}
  document.documentElement.appendChild(overlay);
  return true;
}})()"#
    ))
}

fn remove_screenshot_annotation_expression() -> &'static str {
    r#"(() => {
  const existing = document.getElementById("__puffer_screenshot_overlay__");
  if (existing) existing.remove();
  return true;
})()"#
}

pub(super) fn snapshot_expression() -> &'static str {
    r#"(() => {
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
  // Named iframes are kept so hosted payment fields (Shopify/Stripe PCI card
  // inputs render inside titled cross-origin iframes) surface as addressable,
  // meaningfully-named refs; anonymous tracking/ad frames stay out.
  const nodes = Array.from(document.querySelectorAll(selector))
    .filter(isVisible)
    .filter((el) => el.tagName !== 'IFRAME' || nameFor(el) !== '')
    .slice(0, 120);
  const elements = nodes.map((el, index) => {
    const rect = el.getBoundingClientRect();
    return {
      ref: `@e${index + 1}`,
      role: roleFor(el),
      name: nameFor(el).slice(0, 160),
      tag: el.tagName.toLowerCase(),
      href: el.href || null,
      x: rect.left + rect.width / 2,
      y: rect.top + rect.height / 2
    };
  });
  // Stash exact handles so later ref actions resolve this element directly
  // instead of re-deriving it from now-stale viewport coordinates.
  try {
    const byRef = {};
    nodes.forEach((el, index) => { byRef[`@e${index + 1}`] = el; });
    window.__puffer_agent_refs__ = { byRef };
  } catch (error) {}
  return {
    url: location.href,
    title: document.title,
    text: (document.body ? document.body.innerText : '').replace(/\n{3,}/g, '\n\n').slice(0, 6000),
    elements
  };
})()"#
}
