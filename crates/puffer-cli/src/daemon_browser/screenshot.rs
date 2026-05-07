//! Agent snapshot and screenshot helpers for managed browser tabs.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::params::optional_u32;
use super::{BrowserRegistry, BrowserSession};

const SNAPSHOT_INSTRUCTION: &str =
    "Refs are fresh for this snapshot. Re-snapshot after navigation or dynamic page changes.";
const ANNOTATED_SCREENSHOT_INSTRUCTION: &str =
    "Refs are fresh for this annotated screenshot. Re-annotate or re-snapshot after navigation or dynamic page changes.";
const SCREENSHOT_OVERLAY_ID: &str = "__puffer_screenshot_overlay__";

/// Element reference captured from the last agent browser snapshot.
#[derive(Clone, Debug, Deserialize, Serialize)]
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
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserSnapshot {
    url: String,
    title: String,
    text: String,
    elements: Vec<BrowserElementRef>,
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
        let snapshot_value = self
            .get(backend_session_id)?
            .evaluate(snapshot_expression().to_string())?
            .value;
        let snapshot: BrowserSnapshot =
            serde_json::from_value(snapshot_value).context("decode browser snapshot")?;
        self.agent_refs
            .lock()
            .unwrap()
            .insert(backend_session_id.to_string(), snapshot.elements.clone());
        Ok(json!({
            "url": snapshot.url,
            "title": snapshot.title,
            "text": snapshot.text,
            "elements": snapshot.elements,
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

fn snapshot_expression() -> &'static str {
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
  const selector = 'a,button,input,textarea,select,summary,[role],[contenteditable="true"],[tabindex],label';
  const elements = Array.from(document.querySelectorAll(selector)).filter(isVisible).slice(0, 120).map((el, index) => {
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
  return {
    url: location.href,
    title: document.title,
    text: (document.body ? document.body.innerText : '').replace(/\n{3,}/g, '\n\n').slice(0, 6000),
    elements
  };
})()"#
}
