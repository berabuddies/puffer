//! Bridges uploaded chat attachments to model-readable turn inputs.
//!
//! File attachments can still be materialized to temp paths for the `Read`
//! tool. Image attachments are hydrated to transient `data:` URLs on
//! `AppState` immediately before provider execution.

use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use puffer_core::AppState;
use puffer_session_store::{SessionStore, StoredAttachment, StoredAttachmentKind};
use uuid::Uuid;

const MAX_MODEL_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_MODEL_IMAGE_TOTAL_BYTES: u64 = 50 * 1024 * 1024;
const MAX_MODEL_IMAGES_PER_REQUEST: usize = 10;

/// A staged attachment plus the temp path it was materialized to (`None` if
/// the source bytes were missing or the copy failed).
pub(crate) struct MaterializedAttachment {
    pub attachment: StoredAttachment,
    pub path: Option<PathBuf>,
}

/// Returns a path-safe basename for an attachment, guaranteed to contain no
/// path separators or `..` components, with a file extension when possible.
fn safe_filename(name: &str, extension: &str) -> String {
    // Take the last path component only — defends against names like
    // "../../etc/passwd" or "a/b.png".
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    // Reject any all-dots basename (``, `.`, `..`, `...`) — none is a usable
    // filename and `.`/`..` are traversal components.
    let base = if base.is_empty() || base.bytes().all(|b| b == b'.') {
        ""
    } else {
        base
    };

    let ext = extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();

    if base.is_empty() {
        return if ext.is_empty() {
            "attachment".to_string()
        } else {
            format!("attachment.{ext}")
        };
    }

    if base.contains('.') || ext.is_empty() {
        base.to_string()
    } else {
        format!("{base}.{ext}")
    }
}

/// Root for all materialized attachments. `/tmp` is always in the agent's
/// readable workspace roots (see `puffer-core/workspace_paths.rs`), so files
/// here are readable in any sandbox mode without polluting the user's workspace.
const ATTACHMENTS_ROOT: &str = "/tmp/puffer-attachments";

/// Per-session directory holding every materialized attachment for that session.
fn session_temp_dir(session_id: Uuid) -> PathBuf {
    PathBuf::from(ATTACHMENTS_ROOT).join(session_id.to_string())
}

/// Per-attachment directory, scoped by session and attachment id so re-runs are
/// idempotent and one attachment never collides with another.
fn attachment_temp_dir(session_id: Uuid, attachment_id: &str) -> PathBuf {
    session_temp_dir(session_id).join(attachment_id)
}

/// Materializes every attachment to a temp path. Best-effort: an attachment
/// whose source is missing or whose copy fails gets `path: None` rather than
/// failing the whole turn.
pub(crate) fn materialize_attachments(
    store: &SessionStore,
    session_id: Uuid,
    attachments: &[StoredAttachment],
) -> Vec<MaterializedAttachment> {
    attachments
        .iter()
        .map(|attachment| {
            let path = match materialize_one(store, session_id, attachment) {
                Ok(path) => Some(path),
                Err(err) => {
                    // Best-effort: a degraded single attachment beats failing
                    // the turn. Surface why so an `(unavailable)` line below is
                    // diagnosable instead of a silent drop.
                    eprintln!(
                        "attachment_bridge: skipping attachment {} ({}): {err:#}",
                        attachment.name, attachment.id
                    );
                    None
                }
            };
            MaterializedAttachment {
                attachment: attachment.clone(),
                path,
            }
        })
        .collect()
}

fn materialize_one(
    store: &SessionStore,
    session_id: Uuid,
    attachment: &StoredAttachment,
) -> Result<PathBuf> {
    let src = store.attachment_original_path(session_id, attachment);
    if !src.is_file() {
        anyhow::bail!("staged attachment original missing: {}", src.display());
    }
    let dir = attachment_temp_dir(session_id, &attachment.id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create attachment temp dir {}", dir.display()))?;
    let dest = dir.join(safe_filename(&attachment.name, &attachment.extension));

    // Idempotent: skip the copy if a same-size file is already there.
    if let Ok(meta) = std::fs::metadata(&dest) {
        if meta.len() == attachment.size {
            return Ok(dest);
        }
    }
    std::fs::copy(&src, &dest).with_context(|| format!("copy attachment to {}", dest.display()))?;
    Ok(dest)
}

/// Removes the temp directory tree for a session's materialized attachments.
/// Best-effort; errors are ignored. Call when a session is deleted.
pub(crate) fn cleanup_session_attachments(session_id: Uuid) {
    let _ = std::fs::remove_dir_all(session_temp_dir(session_id));
}

fn model_image_mime_type(attachment: &StoredAttachment) -> Result<&str> {
    match attachment.mime_type.as_str() {
        "image/jpeg" | "image/png" | "image/webp" | "image/gif" => {
            Ok(attachment.mime_type.as_str())
        }
        other => anyhow::bail!(
            "unsupported image attachment MIME type `{other}` for {} ({})",
            attachment.name,
            attachment.id
        ),
    }
}

/// Fills in-memory data URLs for image attachments referenced by the transcript.
pub(crate) fn hydrate_model_image_urls(
    state: &mut AppState,
    store: &SessionStore,
    session_id: Uuid,
) -> Result<()> {
    let mut image_count = 0usize;
    let mut total_image_bytes = 0u64;
    for message in &mut state.transcript {
        for rendered in &mut message.attachments {
            if rendered.attachment.kind != StoredAttachmentKind::Image {
                continue;
            }
            image_count += 1;
            if image_count > MAX_MODEL_IMAGES_PER_REQUEST {
                anyhow::bail!(
                    "too many image attachments in model request history (max {MAX_MODEL_IMAGES_PER_REQUEST})"
                );
            }
            if rendered.attachment.size > MAX_MODEL_IMAGE_BYTES {
                anyhow::bail!(
                    "image attachment {} ({}) exceeds {MAX_MODEL_IMAGE_BYTES} bytes",
                    rendered.attachment.name,
                    rendered.attachment.id
                );
            }
            total_image_bytes = total_image_bytes.saturating_add(rendered.attachment.size);
            if total_image_bytes > MAX_MODEL_IMAGE_TOTAL_BYTES {
                anyhow::bail!(
                    "image attachments exceed total model request budget of {MAX_MODEL_IMAGE_TOTAL_BYTES} bytes"
                );
            }
            let _ = model_image_mime_type(&rendered.attachment)?;
        }
    }
    for message in &mut state.transcript {
        for rendered in &mut message.attachments {
            if rendered.attachment.kind != StoredAttachmentKind::Image {
                continue;
            }
            let mime_type = model_image_mime_type(&rendered.attachment)?;
            let path = store.attachment_original_path(session_id, &rendered.attachment);
            let bytes = std::fs::read(&path).with_context(|| {
                format!(
                    "read image attachment {} ({})",
                    rendered.attachment.name, rendered.attachment.id
                )
            })?;
            if bytes.len() as u64 != rendered.attachment.size {
                anyhow::bail!(
                    "image attachment {} ({}) size changed before model request",
                    rendered.attachment.name,
                    rendered.attachment.id
                );
            }
            rendered.model_url = Some(format!(
                "data:{mime_type};base64,{}",
                BASE64_STANDARD.encode(bytes)
            ));
        }
    }
    Ok(())
}

/// Builds the text the model receives: the original message plus a labeled
/// block listing each file attachment's local path. Returns the original
/// unchanged when there are no file attachments.
pub(crate) fn build_model_input(original: &str, materialized: &[MaterializedAttachment]) -> String {
    let lines: Vec<String> = materialized
        .iter()
        .filter(|m| m.attachment.kind == StoredAttachmentKind::File)
        .map(|m| match &m.path {
            Some(path) => format!(
                "[File: {} -> {} - read this path with Read]",
                m.attachment.name,
                path.display()
            ),
            None => format!("[File: {} (unavailable)]", m.attachment.name),
        })
        .collect();

    if lines.is_empty() {
        return original.to_string();
    }

    let mut out = original.to_string();
    if !out.trim().is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("Attached files (already saved locally):\n");
    out.push_str(&lines.join("\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_session_store::StageAttachmentInput;
    use std::path::Path;

    fn test_store(temp: &Path) -> (SessionStore, Uuid) {
        let user_config = temp.join(".puffer");
        std::fs::create_dir_all(&user_config).unwrap();
        let paths = puffer_config::ConfigPaths {
            workspace_root: temp.to_path_buf(),
            workspace_config_dir: temp.join("ws.puffer"),
            user_config_dir: user_config,
            builtin_resources_dir: temp.join("resources"),
        };
        (SessionStore::from_paths(&paths).unwrap(), Uuid::new_v4())
    }

    fn stage(store: &SessionStore, session: Uuid, name: &str) -> StoredAttachment {
        store
            .stage_attachment(
                session,
                StageAttachmentInput {
                    name: name.to_string(),
                    mime_type: "image/png".to_string(),
                    extension: "PNG".to_string(),
                    kind: StoredAttachmentKind::Image,
                    bytes: vec![1, 2, 3],
                },
            )
            .unwrap()
    }

    fn sample_session_metadata(session: Uuid, cwd: &Path) -> puffer_session_store::SessionMetadata {
        puffer_session_store::SessionMetadata {
            id: session,
            display_name: None,
            generated_title: None,
            cwd: cwd.to_path_buf(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        }
    }

    #[test]
    fn materialize_writes_temp_file_with_extension() {
        let temp = tempfile::tempdir().unwrap();
        let (store, session) = test_store(temp.path());
        let att = stage(&store, session, "shot.png");

        let out = materialize_attachments(&store, session, std::slice::from_ref(&att));
        let path = out[0].path.clone().expect("materialized path");

        assert!(path.starts_with("/tmp/puffer-attachments"));
        assert_eq!(path.file_name().unwrap(), "shot.png");
        assert_eq!(std::fs::read(&path).unwrap(), vec![1, 2, 3]);
        cleanup_session_attachments(session);
    }

    #[test]
    fn materialize_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let (store, session) = test_store(temp.path());
        let att = stage(&store, session, "shot.png");

        let first = materialize_attachments(&store, session, std::slice::from_ref(&att))[0]
            .path
            .clone()
            .unwrap();
        let second = materialize_attachments(&store, session, std::slice::from_ref(&att))[0]
            .path
            .clone()
            .unwrap();
        assert_eq!(first, second);
        cleanup_session_attachments(session);
    }

    #[test]
    fn materialize_missing_original_yields_none() {
        let temp = tempfile::tempdir().unwrap();
        let (store, session) = test_store(temp.path());
        let mut att = stage(&store, session, "shot.png");
        att.id = "11111111-1111-1111-1111-111111111111".to_string(); // never staged

        let out = materialize_attachments(&store, session, std::slice::from_ref(&att));
        assert!(out[0].path.is_none());
        cleanup_session_attachments(session);
    }

    fn attachment(name: &str, kind: StoredAttachmentKind) -> StoredAttachment {
        StoredAttachment {
            id: "00000000-0000-0000-0000-000000000001".to_string(),
            name: name.to_string(),
            mime_type: "application/octet-stream".to_string(),
            size: 3,
            extension: "BIN".to_string(),
            kind,
            storage_key: "k".to_string(),
        }
    }

    #[test]
    fn build_model_input_passthrough_when_no_attachments() {
        assert_eq!(build_model_input("hello", &[]), "hello");
    }

    #[test]
    fn build_model_input_does_not_emit_image_path_hints() {
        let m = vec![MaterializedAttachment {
            attachment: attachment("1.jpg", StoredAttachmentKind::Image),
            path: Some(PathBuf::from("/tmp/puffer-attachments/s/i/1.jpg")),
        }];

        let out = build_model_input("read image", &m);

        assert_eq!(out, "read image");
        assert!(!out.contains("/tmp/puffer-attachments"));
        assert!(!out.contains("VisionAnalyze"));
    }

    #[test]
    fn hydrate_model_image_urls_sets_data_url() {
        let temp = tempfile::tempdir().unwrap();
        let (store, session) = test_store(temp.path());
        let att = stage(&store, session, "pixel.png");
        let metadata = puffer_session_store::SessionMetadata {
            id: session,
            display_name: None,
            generated_title: None,
            cwd: temp.path().to_path_buf(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        let mut state = puffer_core::AppState::new(
            puffer_config::PufferConfig::default(),
            temp.path().to_path_buf(),
            metadata,
        );
        state.push_user_message_with_attachments(
            "[Image: pixel.png]",
            vec![puffer_core::RenderedAttachment::from_stored(att)],
        );

        hydrate_model_image_urls(&mut state, &store, session).unwrap();

        let url = state.transcript[0].attachments[0]
            .model_url
            .as_deref()
            .expect("model url");
        assert!(url.starts_with("data:image/png;base64,"));
        assert!(url.ends_with("AQID"));
    }

    #[test]
    fn hydrate_model_image_urls_rejects_missing_original() {
        let temp = tempfile::tempdir().unwrap();
        let (store, session) = test_store(temp.path());
        let mut att = stage(&store, session, "pixel.png");
        att.id = "22222222-2222-2222-2222-222222222222".to_string();
        let metadata = sample_session_metadata(session, temp.path());
        let mut state = puffer_core::AppState::new(
            puffer_config::PufferConfig::default(),
            temp.path().to_path_buf(),
            metadata,
        );
        state.push_user_message_with_attachments(
            "[Image: pixel.png]",
            vec![puffer_core::RenderedAttachment::from_stored(att)],
        );

        let err = hydrate_model_image_urls(&mut state, &store, session).unwrap_err();

        assert!(err.to_string().contains("read image attachment"));
    }

    #[test]
    fn hydrate_model_image_urls_rejects_total_image_budget_before_reading() {
        let temp = tempfile::tempdir().unwrap();
        let (store, session) = test_store(temp.path());
        let attachments = (0..3)
            .map(|index| {
                let mut att = stage(&store, session, &format!("pixel-{index}.png"));
                att.size = 18 * 1024 * 1024;
                att
            })
            .map(puffer_core::RenderedAttachment::from_stored)
            .collect::<Vec<_>>();
        let metadata = sample_session_metadata(session, temp.path());
        let mut state = puffer_core::AppState::new(
            puffer_config::PufferConfig::default(),
            temp.path().to_path_buf(),
            metadata,
        );
        state.push_user_message_with_attachments("[Image: pixels]", attachments);

        let err = hydrate_model_image_urls(&mut state, &store, session).unwrap_err();

        assert!(err.to_string().contains("total model request budget"));
    }

    #[test]
    fn build_model_input_appends_file_path_with_hint() {
        let m = vec![MaterializedAttachment {
            attachment: attachment("report.pdf", StoredAttachmentKind::File),
            path: Some(PathBuf::from("/tmp/puffer-attachments/s/i/report.pdf")),
        }];
        let out = build_model_input("read file", &m);
        assert!(out.starts_with("read file"));
        assert!(out.contains("/tmp/puffer-attachments/s/i/report.pdf"));
        assert!(out.contains("read this path with Read"));
        assert!(!out.contains("VisionAnalyze"));
    }

    #[test]
    fn build_model_input_marks_missing_unavailable() {
        let m = vec![MaterializedAttachment {
            attachment: attachment("gone.pdf", StoredAttachmentKind::File),
            path: None,
        }];
        let out = build_model_input("see file", &m);
        assert!(out.contains("[File: gone.pdf (unavailable)]"));
        assert!(!out.contains("->"));
    }

    #[test]
    fn safe_filename_keeps_plain_name() {
        assert_eq!(safe_filename("pixel.png", "PNG"), "pixel.png");
    }

    #[test]
    fn safe_filename_strips_path_components() {
        assert_eq!(safe_filename("../../etc/passwd", ""), "passwd");
        assert_eq!(safe_filename("a/b/c.jpg", "JPG"), "c.jpg");
    }

    #[test]
    fn safe_filename_appends_extension_when_missing() {
        assert_eq!(safe_filename("report", "PDF"), "report.pdf");
    }

    #[test]
    fn safe_filename_falls_back_when_empty_or_dots() {
        assert_eq!(safe_filename("..", "PNG"), "attachment.png");
        assert_eq!(safe_filename("...", "PNG"), "attachment.png");
        assert_eq!(safe_filename("", ""), "attachment");
        assert_eq!(safe_filename("   ", "TXT"), "attachment.txt");
    }
}
