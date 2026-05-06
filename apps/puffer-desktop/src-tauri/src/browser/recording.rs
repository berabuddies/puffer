//! In-memory screen recordings for managed browser tabs.

use serde::Serialize;
use std::collections::{hash_map::DefaultHasher, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{tabs::parse_backend_session_id, BrowserState};

const MAX_FRAMES_PER_ROOT: usize = 240;

/// A unique screencast frame retained for the History pane.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserRecordedFrame {
    pub(crate) frame_id: String,
    pub(crate) backend_session_id: String,
    pub(crate) root_session_id: String,
    pub(crate) tab_id: String,
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) mime_type: String,
    pub(crate) encoding: String,
    pub(crate) data: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) recorded_at_ms: u64,
}

/// Stores deduplicated browser screencast frames per Puffer session.
#[derive(Default)]
pub(crate) struct BrowserRecordingRegistry {
    roots: HashMap<String, VecDeque<BrowserRecordedFrame>>,
    last_signature_by_backend: HashMap<String, u64>,
    active_until_by_backend: HashMap<String, u64>,
}

impl BrowserRecordingRegistry {
    /// Allows frames for `backend_session_id` during an agent Browser action.
    pub(crate) fn arm_backend(&mut self, backend_session_id: &str, duration: Duration) {
        let active_until = now_ms().saturating_add(duration.as_millis() as u64);
        self.active_until_by_backend
            .insert(backend_session_id.to_string(), active_until);
    }

    /// Records a frame and returns it only when the visible screen changed.
    pub(crate) fn record_frame(
        &mut self,
        backend_session_id: &str,
        cdp_frame_id: &str,
        data: &str,
        width: u32,
        height: u32,
        state: &BrowserState,
    ) -> Option<BrowserRecordedFrame> {
        let (root_session_id, tab_id) = parse_backend_session_id(backend_session_id)?;
        if !self.is_backend_active(backend_session_id) {
            return None;
        }
        let signature = frame_signature(data, width, height);
        if self
            .last_signature_by_backend
            .get(backend_session_id)
            .is_some_and(|last| *last == signature)
        {
            return None;
        }
        self.last_signature_by_backend
            .insert(backend_session_id.to_string(), signature);
        let frame = BrowserRecordedFrame {
            frame_id: format!("{backend_session_id}:{cdp_frame_id}:{}", now_ms()),
            backend_session_id: backend_session_id.to_string(),
            root_session_id: root_session_id.to_string(),
            tab_id: tab_id.to_string(),
            url: state.url.clone(),
            title: state.title.clone(),
            mime_type: "image/jpeg".to_string(),
            encoding: "base64".to_string(),
            data: data.to_string(),
            width,
            height,
            recorded_at_ms: now_ms(),
        };
        let frames = self.roots.entry(root_session_id.to_string()).or_default();
        frames.push_back(frame.clone());
        while frames.len() > MAX_FRAMES_PER_ROOT {
            frames.pop_front();
        }
        Some(frame)
    }

    /// Returns the retained recording for one Puffer session.
    pub(crate) fn frames_for(&self, root_session_id: &str) -> Vec<BrowserRecordedFrame> {
        self.roots
            .get(root_session_id)
            .map(|frames| frames.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn is_backend_active(&mut self, backend_session_id: &str) -> bool {
        let now = now_ms();
        self.active_until_by_backend
            .retain(|_, active_until| *active_until >= now);
        self.active_until_by_backend
            .get(backend_session_id)
            .is_some_and(|active_until| *active_until >= now)
    }
}

fn frame_signature(data: &str, width: u32, height: u32) -> u64 {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    hasher.finish()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
