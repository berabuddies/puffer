use super::*;
use tempfile::tempdir;

#[test]
fn generated_media_preview_by_artifact_uses_sidecar_path() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let image_path = service
        .write_image_artifact_bytes("artifact-1", "image.jpeg", &[0xff, 0xd8, 0xff, 0xd9])
        .unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-1".to_string(),
            job_id: "job-1".to_string(),
            kind: crate::media::MediaKind::Image,
            path: image_path,
            mime_type: "image/jpeg".to_string(),
            byte_count: 4,
            metadata: serde_json::json!({}),
            preview: None,
            created_at_ms: 1,
        })
        .unwrap();

    let result = read_generated_media_preview_by_artifact(workspace.path(), "artifact-1");

    assert_eq!(
        result,
        GeneratedMediaPreviewResult::Available {
            mime_type: "image/jpeg".to_string(),
            bytes: vec![0xff, 0xd8, 0xff, 0xd9],
        }
    );
}

#[test]
fn generated_media_preview_by_artifact_rejects_symlink_escape() {
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let outside_image = outside.path().join("image.jpeg");
    std::fs::write(&outside_image, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
    let link_dir = workspace.path().join(".puffer/media/images/artifact-1");
    std::fs::create_dir_all(&link_dir).unwrap();
    let link = link_dir.join("image.jpeg");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_image, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&outside_image, &link).unwrap();

    let service = crate::media::MediaGenerationService::new(workspace.path());
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-1".to_string(),
            job_id: "job-1".to_string(),
            kind: crate::media::MediaKind::Image,
            path: link,
            mime_type: "image/jpeg".to_string(),
            byte_count: 4,
            metadata: serde_json::json!({}),
            preview: None,
            created_at_ms: 1,
        })
        .unwrap();

    assert_eq!(
        read_generated_media_preview_by_artifact(workspace.path(), "artifact-1"),
        GeneratedMediaPreviewResult::Unsupported
    );
}

#[test]
fn generated_media_attachment_metadata_rejects_symlink_escape() {
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let outside_image = outside.path().join("image.jpeg");
    std::fs::write(&outside_image, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
    let link_dir = workspace.path().join(".puffer/media/images/artifact-1");
    std::fs::create_dir_all(&link_dir).unwrap();
    let link = link_dir.join("image.jpeg");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_image, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&outside_image, &link).unwrap();

    let service = crate::media::MediaGenerationService::new(workspace.path());
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-1".to_string(),
            job_id: "job-1".to_string(),
            kind: crate::media::MediaKind::Image,
            path: link,
            mime_type: "image/jpeg".to_string(),
            byte_count: 4,
            metadata: serde_json::json!({}),
            preview: None,
            created_at_ms: 1,
        })
        .unwrap();

    assert_eq!(
        generated_media_attachment_metadata(workspace.path(), "artifact-1"),
        None
    );
}

#[test]
fn generated_media_attachment_metadata_falls_back_when_sidecar_is_missing() {
    let workspace = tempdir().unwrap();

    let metadata = generated_media_attachment_metadata_with_fallback(
        workspace.path(),
        "artifact-1",
        "image/webp",
        42,
    )
    .unwrap();

    assert_eq!(metadata.artifact_id, "artifact-1");
    assert_eq!(metadata.mime_type, "image/webp");
    assert_eq!(metadata.byte_count, 42);
    assert_eq!(metadata.state, "missing");
}

#[test]
fn generated_media_attachment_metadata_fallback_does_not_bypass_unsafe_sidecar() {
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let outside_image = outside.path().join("image.jpeg");
    std::fs::write(&outside_image, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
    let link_dir = workspace.path().join(".puffer/media/images/artifact-1");
    std::fs::create_dir_all(&link_dir).unwrap();
    let link = link_dir.join("image.jpeg");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_image, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&outside_image, &link).unwrap();

    let service = crate::media::MediaGenerationService::new(workspace.path());
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-1".to_string(),
            job_id: "job-1".to_string(),
            kind: crate::media::MediaKind::Image,
            path: link,
            mime_type: "image/jpeg".to_string(),
            byte_count: 4,
            metadata: serde_json::json!({}),
            preview: None,
            created_at_ms: 1,
        })
        .unwrap();

    assert_eq!(
        generated_media_attachment_metadata_with_fallback(
            workspace.path(),
            "artifact-1",
            "image/webp",
            42,
        ),
        None
    );
}

#[test]
fn generated_media_attachment_metadata_fallback_does_not_mask_corrupt_sidecar() {
    let workspace = tempdir().unwrap();
    let sidecar_dir = workspace.path().join(".puffer/media/artifact-sidecars");
    std::fs::create_dir_all(&sidecar_dir).unwrap();
    std::fs::write(sidecar_dir.join("artifact-1.json"), b"{not-json").unwrap();

    assert_eq!(
        generated_media_attachment_metadata_with_fallback(
            workspace.path(),
            "artifact-1",
            "image/webp",
            42,
        ),
        None
    );
}

#[test]
fn generated_media_preview_by_artifact_sniffs_mime_when_extension_lies() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let image_path = service
        .write_image_artifact_bytes("artifact-1", "image.png", &[0xff, 0xd8, 0xff, 0xd9])
        .unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-1".to_string(),
            job_id: "job-1".to_string(),
            kind: crate::media::MediaKind::Image,
            path: image_path,
            mime_type: "image/png".to_string(),
            byte_count: 4,
            metadata: serde_json::json!({}),
            preview: None,
            created_at_ms: 1,
        })
        .unwrap();

    let result = read_generated_media_preview_by_artifact(workspace.path(), "artifact-1");

    assert!(matches!(
        result,
        GeneratedMediaPreviewResult::Available { mime_type, .. } if mime_type == "image/jpeg"
    ));
}

#[test]
fn generated_media_preview_by_artifact_reads_video_poster_preview() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let video_path = service
        .write_video_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
        .unwrap();
    let poster_path = service
        .poster_artifact_file_path("artifact-video-1")
        .unwrap();
    std::fs::create_dir_all(poster_path.parent().unwrap()).unwrap();
    std::fs::write(&poster_path, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: crate::media::MediaKind::Video,
            path: video_path,
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: serde_json::json!({}),
            preview: Some(
                crate::media::artifacts::MediaArtifactPreview::available_poster(poster_path, 4),
            ),
            created_at_ms: 1,
        })
        .unwrap();

    let result = read_generated_media_preview_by_artifact(workspace.path(), "artifact-video-1");

    assert_eq!(
        result,
        GeneratedMediaPreviewResult::Available {
            mime_type: "image/jpeg".to_string(),
            bytes: vec![0xff, 0xd8, 0xff, 0xd9],
        }
    );
}

#[test]
fn generated_media_preview_by_artifact_returns_missing_for_video_without_preview() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let video_path = service
        .write_video_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
        .unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: crate::media::MediaKind::Video,
            path: video_path,
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: serde_json::json!({}),
            preview: None,
            created_at_ms: 1,
        })
        .unwrap();

    assert_eq!(
        read_generated_media_preview_by_artifact(workspace.path(), "artifact-video-1"),
        GeneratedMediaPreviewResult::Missing
    );
}

#[test]
fn generated_media_preview_by_artifact_returns_missing_for_failed_video_preview() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let video_path = service
        .write_video_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
        .unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: crate::media::MediaKind::Video,
            path: video_path,
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: serde_json::json!({}),
            preview: Some(
                crate::media::artifacts::MediaArtifactPreview::missing_poster(
                    "ffmpeg timed out after 10s",
                ),
            ),
            created_at_ms: 1,
        })
        .unwrap();

    assert_eq!(
        read_generated_media_preview_by_artifact(workspace.path(), "artifact-video-1"),
        GeneratedMediaPreviewResult::Missing
    );
}

#[test]
fn generated_media_preview_by_artifact_returns_missing_for_absent_video_poster_file() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let video_path = service
        .write_video_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
        .unwrap();
    let poster_path = service
        .poster_artifact_file_path("artifact-video-1")
        .unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: crate::media::MediaKind::Video,
            path: video_path,
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: serde_json::json!({}),
            preview: Some(
                crate::media::artifacts::MediaArtifactPreview::available_poster(poster_path, 4),
            ),
            created_at_ms: 1,
        })
        .unwrap();

    assert_eq!(
        read_generated_media_preview_by_artifact(workspace.path(), "artifact-video-1"),
        GeneratedMediaPreviewResult::Missing
    );
}

#[test]
fn generated_media_preview_by_artifact_rejects_video_poster_bad_mime() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let video_path = service
        .write_video_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
        .unwrap();
    let poster_path = service
        .poster_artifact_file_path("artifact-video-1")
        .unwrap();
    std::fs::create_dir_all(poster_path.parent().unwrap()).unwrap();
    std::fs::write(&poster_path, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: crate::media::MediaKind::Video,
            path: video_path,
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: serde_json::json!({}),
            preview: Some(crate::media::artifacts::MediaArtifactPreview::Poster(
                crate::media::artifacts::MediaPosterPreview {
                    state: crate::media::artifacts::MediaArtifactPreviewState::Available,
                    path: Some(poster_path),
                    mime_type: Some("image/png".to_string()),
                    byte_count: Some(4),
                    reason: None,
                },
            )),
            created_at_ms: 1,
        })
        .unwrap();

    assert_eq!(
        read_generated_media_preview_by_artifact(workspace.path(), "artifact-video-1"),
        GeneratedMediaPreviewResult::Unsupported
    );
}

#[test]
fn generated_media_preview_by_artifact_rejects_video_poster_non_image_bytes() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let video_path = service
        .write_video_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
        .unwrap();
    let poster_path = service
        .poster_artifact_file_path("artifact-video-1")
        .unwrap();
    std::fs::create_dir_all(poster_path.parent().unwrap()).unwrap();
    std::fs::write(&poster_path, b"not-image").unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: crate::media::MediaKind::Video,
            path: video_path,
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: serde_json::json!({}),
            preview: Some(
                crate::media::artifacts::MediaArtifactPreview::available_poster(poster_path, 9),
            ),
            created_at_ms: 1,
        })
        .unwrap();

    assert_eq!(
        read_generated_media_preview_by_artifact(workspace.path(), "artifact-video-1"),
        GeneratedMediaPreviewResult::Unsupported
    );
}

#[test]
fn generated_media_preview_by_artifact_rejects_video_poster_unsafe_path() {
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let outside_poster = outside.path().join("poster.jpg");
    std::fs::write(&outside_poster, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let video_path = service
        .write_video_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
        .unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: crate::media::MediaKind::Video,
            path: video_path,
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: serde_json::json!({}),
            preview: Some(
                crate::media::artifacts::MediaArtifactPreview::available_poster(outside_poster, 4),
            ),
            created_at_ms: 1,
        })
        .unwrap();

    assert_eq!(
        read_generated_media_preview_by_artifact(workspace.path(), "artifact-video-1"),
        GeneratedMediaPreviewResult::Unsupported
    );
}

#[test]
fn generated_media_preview_by_artifact_rejects_video_poster_symlink_escape() {
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let outside_poster = outside.path().join("poster.jpg");
    std::fs::write(&outside_poster, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let video_path = service
        .write_video_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
        .unwrap();
    let poster_path = service
        .poster_artifact_file_path("artifact-video-1")
        .unwrap();
    std::fs::create_dir_all(poster_path.parent().unwrap()).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_poster, &poster_path).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&outside_poster, &poster_path).unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: crate::media::MediaKind::Video,
            path: video_path,
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: serde_json::json!({}),
            preview: Some(
                crate::media::artifacts::MediaArtifactPreview::available_poster(poster_path, 4),
            ),
            created_at_ms: 1,
        })
        .unwrap();

    assert_eq!(
        read_generated_media_preview_by_artifact(workspace.path(), "artifact-video-1"),
        GeneratedMediaPreviewResult::Unsupported
    );
}

#[test]
fn generated_video_access_metadata_accepts_video_under_video_root() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let video_path = service
        .write_video_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
        .unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: crate::media::MediaKind::Video,
            path: video_path.clone(),
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: serde_json::json!({
                "remoteSourceUrl": "https://cdn.example.test/generated.mp4"
            }),
            preview: None,
            created_at_ms: 1,
        })
        .unwrap();

    let result = generated_video_access_metadata_by_artifact(workspace.path(), "artifact-video-1");

    let GeneratedVideoAccessMetadataResult::Available(metadata) = result else {
        panic!("expected available video metadata");
    };
    assert_eq!(metadata.mime_type, "video/mp4");
    assert_eq!(metadata.byte_count, 9);
    assert_eq!(metadata.path, video_path.canonicalize().unwrap());
    assert_eq!(
        metadata.remote_source_url.as_deref(),
        Some("https://cdn.example.test/generated.mp4")
    );
}

#[test]
fn generated_video_access_metadata_accepts_legacy_video_under_artifact_root() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let video_path = service
        .write_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
        .unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: crate::media::MediaKind::Video,
            path: video_path.clone(),
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: serde_json::json!({
                "remoteSourceUrl": "https://cdn.example.test/generated.mp4"
            }),
            preview: None,
            created_at_ms: 1,
        })
        .unwrap();

    let result = generated_video_access_metadata_by_artifact(workspace.path(), "artifact-video-1");

    let GeneratedVideoAccessMetadataResult::Available(metadata) = result else {
        panic!("expected available video metadata");
    };
    assert_eq!(metadata.mime_type, "video/mp4");
    assert_eq!(metadata.byte_count, 9);
    assert_eq!(metadata.path, video_path.canonicalize().unwrap());
}

#[test]
fn generated_video_access_metadata_rejects_non_video_artifact() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let image_path = service
        .write_image_artifact_bytes("artifact-image-1", "image.jpeg", &[0xff, 0xd8, 0xff, 0xd9])
        .unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-image-1".to_string(),
            job_id: "job-image-1".to_string(),
            kind: crate::media::MediaKind::Image,
            path: image_path,
            mime_type: "image/jpeg".to_string(),
            byte_count: 4,
            metadata: serde_json::json!({}),
            preview: None,
            created_at_ms: 1,
        })
        .unwrap();

    assert_eq!(
        generated_video_access_metadata_by_artifact(workspace.path(), "artifact-image-1"),
        GeneratedVideoAccessMetadataResult::Unsupported
    );
}

#[test]
fn generated_video_access_metadata_rejects_symlink_escape() {
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let outside_video = outside.path().join("generated.mp4");
    std::fs::write(&outside_video, b"mp4-bytes").unwrap();
    let link_dir = workspace
        .path()
        .join(".puffer/media/videos/artifact-video-1");
    std::fs::create_dir_all(&link_dir).unwrap();
    let link = link_dir.join("generated.mp4");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_video, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&outside_video, &link).unwrap();

    let service = crate::media::MediaGenerationService::new(workspace.path());
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: crate::media::MediaKind::Video,
            path: link,
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: serde_json::json!({}),
            preview: None,
            created_at_ms: 1,
        })
        .unwrap();

    assert_eq!(
        generated_video_access_metadata_by_artifact(workspace.path(), "artifact-video-1"),
        GeneratedVideoAccessMetadataResult::Unsupported
    );
}

#[test]
fn generated_media_internal_command_kind_detects_supported_helpers() {
    assert_eq!(
        generated_media_internal_command_kind("imagegen --prompt 'ship at sea' --count 1"),
        Some(GeneratedMediaInternalCommandKind::Image)
    );
    assert_eq!(
        generated_media_internal_command_kind("videogen --prompt clip"),
        Some(GeneratedMediaInternalCommandKind::Video)
    );
}

#[test]
fn generated_media_internal_command_kind_rejects_raw_puffer_internal_tools() {
    assert_eq!(
        generated_media_internal_command_kind(
            "puffer internal-tool image-generation --prompt 'ship at sea' --count 1"
        ),
        None
    );
    assert_eq!(
        generated_media_internal_command_kind(
            "puffer internal-tool video-generation --prompt clip"
        ),
        None
    );
}

#[test]
fn generated_media_internal_command_kind_allows_quoted_control_tokens() {
    assert_eq!(
        generated_media_internal_command_kind("imagegen --prompt 'ship && sea | sky' --count 1"),
        Some(GeneratedMediaInternalCommandKind::Image)
    );
}

#[test]
fn generated_media_internal_command_kind_rejects_shell_control_tokens() {
    assert_eq!(
        generated_media_internal_command_kind("imagegen --prompt ship && echo ok"),
        None
    );
    assert_eq!(
        generated_media_internal_command_kind("imagegen --prompt ship; echo ok"),
        None
    );
}

#[test]
fn generated_media_internal_command_kind_rejects_arbitrary_commands() {
    assert_eq!(
        generated_media_internal_command_kind("cat generated-media.json"),
        None
    );
}

#[test]
fn generated_media_internal_bash_output_extracts_stdout_json() {
    let input = serde_json::json!({ "command": "imagegen --prompt ship" }).to_string();
    let stdout = serde_json::json!({
        "jobId": "job-1",
        "artifacts": []
    })
    .to_string();
    let output = serde_json::json!({ "stdout": stdout }).to_string();

    let (kind, value) = generated_media_internal_bash_output("Bash", &input, &output).unwrap();

    assert_eq!(kind, GeneratedMediaInternalCommandKind::Image);
    assert_eq!(value["jobId"], "job-1");
}

#[test]
fn generated_media_internal_bash_output_rejects_non_bash_tool() {
    let input = serde_json::json!({ "command": "imagegen --prompt ship" }).to_string();
    let output = serde_json::json!({
        "stdout": serde_json::json!({ "jobId": "job-1" }).to_string()
    })
    .to_string();

    assert_eq!(
        generated_media_internal_bash_output("Read", &input, &output),
        None
    );
}

#[test]
fn generated_media_timeline_attachments_extracts_image_metadata() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let image_path = service
        .write_image_artifact_bytes("artifact-1", "image.webp", b"RIFFxxxxWEBP")
        .unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-1".to_string(),
            job_id: "job-1".to_string(),
            kind: crate::media::MediaKind::Image,
            path: image_path,
            mime_type: "image/webp".to_string(),
            byte_count: 12,
            metadata: serde_json::json!({"remoteSourceUrl": "https://example.test/image.webp"}),
            preview: None,
            created_at_ms: 1,
        })
        .unwrap();
    let input = serde_json::json!({
        "command": "imagegen --prompt ship --count 1"
    })
    .to_string();
    let output = serde_json::json!({
        "stdout": serde_json::json!({
            "jobId": "job-1",
            "artifacts": [
                {"artifactId": "artifact-1", "index": 2, "mimeType": "image/png", "size": 99}
            ]
        }).to_string()
    })
    .to_string();

    let attachments =
        generated_media_timeline_attachments(workspace.path(), "Bash", &input, &output);

    assert_eq!(attachments.len(), 1);
    let attachment = &attachments[0];
    assert_eq!(attachment.id, "generated-image:artifact-1");
    assert_eq!(attachment.name, "Generated image");
    assert_eq!(attachment.kind, GeneratedMediaTimelineAttachmentKind::Image);
    assert_eq!(attachment.mime_type, "image/webp");
    assert_eq!(attachment.extension, "WEBP");
    assert_eq!(attachment.byte_count, 12);
    assert_eq!(attachment.state, "available");
    assert_eq!(attachment.job_id, "job-1");
    assert_eq!(attachment.artifact_id, "artifact-1");
    assert_eq!(attachment.index, 2);
    assert!(attachment
        .local_path
        .as_deref()
        .unwrap()
        .ends_with("image.webp"));
    assert_eq!(
        attachment.remote_source_url.as_deref(),
        Some("https://example.test/image.webp")
    );
}

#[test]
fn generated_media_timeline_attachments_trusts_video_sidecar_path() {
    let workspace = tempdir().unwrap();
    let service = crate::media::MediaGenerationService::new(workspace.path());
    let video_path = service
        .write_video_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
        .unwrap();
    service
        .save_artifact(&crate::media::MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: crate::media::MediaKind::Video,
            path: video_path.clone(),
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: serde_json::json!({
                "remoteSourceUrl": "https://cdn.example.test/generated.mp4"
            }),
            preview: None,
            created_at_ms: 1,
        })
        .unwrap();
    let outside = tempdir().unwrap();
    let forged_path = outside.path().join("forged.mp4");
    let input = serde_json::json!({ "command": "videogen --prompt clip" }).to_string();
    let output = serde_json::json!({
        "stdout": serde_json::json!({
            "jobId": "job-video-1",
            "kind": "video",
            "artifacts": [
                {
                    "artifactId": "artifact-video-1",
                    "index": 0,
                    "path": forged_path,
                    "mimeType": "video/mp4",
                    "size": 99
                }
            ]
        }).to_string()
    })
    .to_string();

    let attachments =
        generated_media_timeline_attachments(workspace.path(), "Bash", &input, &output);

    assert_eq!(attachments.len(), 1);
    let attachment = &attachments[0];
    assert_eq!(attachment.id, "generated-video:artifact-video-1");
    assert_eq!(attachment.kind, GeneratedMediaTimelineAttachmentKind::Video);
    assert_eq!(attachment.mime_type, "video/mp4");
    assert_eq!(attachment.extension, "MP4");
    assert_eq!(attachment.byte_count, 9);
    assert_eq!(attachment.state, "available");
    let canonical_video_path = std::fs::canonicalize(video_path).unwrap();
    let expected_local_path = canonical_video_path.display().to_string();
    assert_eq!(
        attachment.local_path.as_deref(),
        Some(expected_local_path.as_str())
    );
    assert_eq!(
        attachment.remote_source_url.as_deref(),
        Some("https://cdn.example.test/generated.mp4")
    );
}

#[test]
fn generated_media_timeline_attachments_do_not_trust_video_stdout_path() {
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let outside_video = outside.path().join("generated.mp4");
    std::fs::write(&outside_video, b"mp4-bytes").unwrap();
    let input = serde_json::json!({ "command": "videogen --prompt clip" }).to_string();
    let output = serde_json::json!({
        "stdout": serde_json::json!({
            "jobId": "job-video-1",
            "kind": "video",
            "artifacts": [
                {
                    "artifactId": "artifact-video-1",
                    "index": 0,
                    "path": outside_video,
                    "mimeType": "video/mp4",
                    "size": 9
                }
            ]
        }).to_string()
    })
    .to_string();

    let attachments =
        generated_media_timeline_attachments(workspace.path(), "Bash", &input, &output);

    assert_eq!(attachments.len(), 1);
    let attachment = &attachments[0];
    assert_eq!(attachment.kind, GeneratedMediaTimelineAttachmentKind::Video);
    assert_eq!(attachment.state, "missing");
    assert_eq!(attachment.local_path, None);
    assert_eq!(attachment.byte_count, 9);
}
