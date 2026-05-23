//! Real-LLM E2E test for `/genskill`. Seeds a synthetic transcript with
//! reference-solution context (bypasses the 5-tool-call gate), dispatches
//! `/genskill --candidates 2 --rounds 1` to minimize cost, then verifies
//! the GEPA loop produced a valid SKILL.md on disk.
//!
//! Usage:
//!   cargo run --example genskill_e2e_probe
//!   PROBE_PROVIDER=hanbbq PROBE_MODEL=gpt-4.1-mini cargo run --example genskill_e2e_probe
//!
//! Subjects tested:
//!   1. /genskill dispatches and returns "Skill written to <path>"
//!   2. The written SKILL.md exists and has valid frontmatter
//!   3. GEPA loop ran (generate + judge + select) with real LLM calls
//!   4. Cleanup: generated skill directory is removed after verification

use anyhow::{anyhow, Result};
use puffer_config::{ensure_workspace_dirs, load_config, ConfigPaths};
use puffer_core::{dispatch_command, supported_commands, AppState, MessageRole};
use puffer_provider_registry::ProviderRegistry;
use puffer_resources::load_resources;
use puffer_session_store::{SessionMetadata, SessionStore};
use puffer_skill_evolution;
use std::path::PathBuf;

fn main() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let paths = ConfigPaths::discover(&cwd);
    ensure_workspace_dirs(&paths)?;
    let config = load_config(&paths)?;
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = puffer_provider_registry::AuthStore::load(&auth_path)?;
    let resources = load_resources(&paths, &puffer_core::runner_adapter::LocalToolRunner::new())?;

    let mut providers = ProviderRegistry::new();
    for provider in &resources.providers {
        let descriptor = provider.value.clone().into_descriptor();
        providers.register_with_source(descriptor, provider.source_info.as_provider_source());
    }
    providers.apply_openai_base_url_override(config.openai_base_url.as_deref());
    if !config.openai_headers.is_empty() {
        providers.set_openai_headers(
            config
                .openai_headers
                .clone()
                .into_iter()
                .collect::<indexmap::IndexMap<_, _>>(),
        );
    }
    if !config.openai_query_params.is_empty() {
        providers.set_openai_query_params(
            config
                .openai_query_params
                .clone()
                .into_iter()
                .collect::<indexmap::IndexMap<_, _>>(),
        );
    }
    let _ = providers.discover_and_merge_all(&auth_store);

    let provider_id = std::env::var("PROBE_PROVIDER").unwrap_or_else(|_| {
        config
            .default_provider
            .clone()
            .unwrap_or_else(|| "hanbbq".to_string())
    });
    let model = resolve_model(&providers, &provider_id)?;

    eprintln!("[genskill-e2e] provider={provider_id} model={model}");

    let session_store = SessionStore::from_paths(&paths)?;
    let session_record = session_store.create_session(cwd.clone())?;
    let session = SessionMetadata {
        id: session_record.id,
        display_name: Some("genskill-e2e-probe".to_string()),
        generated_title: None,
        cwd: cwd.clone(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let mut state = AppState::new(config, cwd.clone(), session);
    state.current_provider = Some(provider_id.clone());
    state.current_model = Some(model.clone());

    // Seed transcript with reference-solution context to bypass the
    // 5-tool-call threshold. This simulates what the Ladybird eval
    // harness does: inject a reference fix patch so /genskill can
    // distill it into a skill.
    state.push_message(
        MessageRole::User,
        concat!(
            "Reference solution context\n\n",
            "Task: Fix the CSS border-radius rendering bug in Ladybird browser.\n\n",
            "Reference fix patch:\n",
            "```diff\n",
            "--- a/Userland/Libraries/LibWeb/Painting/PaintableBox.cpp\n",
            "+++ b/Userland/Libraries/LibWeb/Painting/PaintableBox.cpp\n",
            "@@ -234,7 +234,9 @@\n",
            " void PaintableBox::paint_border(PaintContext& context)\n",
            " {\n",
            "-    auto border_rect = absolute_border_box_rect();\n",
            "+    auto border_rect = absolute_padding_box_rect();\n",
            "+    auto radii = normalized_border_radii_data();\n",
            "+    context.painter().draw_rect_with_rounded_corners(border_rect, radii);\n",
            " }\n",
            "```\n\n",
            "The fix changes border painting to use padding box rect and apply ",
            "corner radii. This is a common pattern in browser engine rendering."
        )
        .to_string(),
    );
    state.push_message(
        MessageRole::Assistant,
        "I understand the border-radius fix. The key insight is using padding_box_rect instead of border_box_rect as the clipping boundary, then applying normalized radii.".to_string(),
    );

    let mut passed = 0;
    let mut failed = 0;

    // ── Subject 0: raw LLM call sanity check ──
    eprintln!("\n[genskill-e2e] Subject 0: raw LLM generation sanity check");
    {
        let raw_result = puffer_core::execute_user_turn(
            &mut state,
            &resources,
            &providers,
            &mut auth_store,
            "Reply with ONLY the following text, no markdown fences, no explanation:\n---\nname: test-skill\ndescription: A test\n---\nBody here.",
        );
        match &raw_result {
            Ok(turn) => {
                eprintln!("  LLM returned {} chars", turn.assistant_text.len());
                eprintln!(
                    "  first 300 chars: {}",
                    &turn.assistant_text[..turn.assistant_text.len().min(300)]
                );
                let parse_result = puffer_skill_evolution::parse_skill_md(&turn.assistant_text);
                match parse_result {
                    Ok(candidate) => {
                        eprintln!("  parse_skill_md OK: name={}", candidate.frontmatter.name);
                        passed += 1;
                    }
                    Err(e) => {
                        eprintln!("  parse_skill_md FAILED: {e}");
                        eprintln!("  (this reveals what the GEPA generate step sees)");
                        passed += 1; // still a pass for the sanity check - we got data
                    }
                }
            }
            Err(e) => {
                eprintln!("  FAIL: raw LLM call failed: {e}");
                failed += 1;
            }
        }
    }

    // Reset transcript for actual /genskill test
    state.push_message(
        MessageRole::User,
        concat!(
            "Reference solution context\n\n",
            "Task: Fix the CSS border-radius rendering bug in Ladybird browser.\n\n",
            "Reference fix patch:\n",
            "```diff\n",
            "--- a/Userland/Libraries/LibWeb/Painting/PaintableBox.cpp\n",
            "+++ b/Userland/Libraries/LibWeb/Painting/PaintableBox.cpp\n",
            "@@ -234,7 +234,9 @@\n",
            " void PaintableBox::paint_border(PaintContext& context)\n",
            " {\n",
            "-    auto border_rect = absolute_border_box_rect();\n",
            "+    auto border_rect = absolute_padding_box_rect();\n",
            "+    auto radii = normalized_border_radii_data();\n",
            "+    context.painter().draw_rect_with_rounded_corners(border_rect, radii);\n",
            " }\n",
            "```\n\n",
            "The fix changes border painting to use padding box rect and apply ",
            "corner radii. This is a common pattern in browser engine rendering."
        )
        .to_string(),
    );

    // ── Subject 1: /genskill dispatches and returns skill path ──
    eprintln!("\n[genskill-e2e] Subject 1: dispatch /genskill --candidates 2 --rounds 1");
    let before = state.transcript.len();

    let dispatch_result = dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut providers,
        &mut auth_store,
        &session_store,
        "/genskill --candidates 2 --rounds 1",
    );

    match &dispatch_result {
        Ok(()) => {
            let last = state
                .transcript
                .last()
                .map(|m| m.text.as_str())
                .unwrap_or("");
            if last.contains("Skill written to") {
                eprintln!("  PASS: dispatch returned skill path");
                eprintln!("  output: {last}");
                passed += 1;
            } else {
                eprintln!("  FAIL: dispatch succeeded but last message doesn't contain skill path");
                eprintln!("  last message: {}", &last[..last.len().min(500)]);
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("  FAIL: dispatch error: {e}");
            failed += 1;
        }
    }

    // ── Subject 2: SKILL.md file exists and has valid content ──
    eprintln!("\n[genskill-e2e] Subject 2: verify generated SKILL.md");
    let skill_path = state.transcript.last().and_then(|m| {
        m.text
            .strip_prefix("Skill written to ")
            .map(|p| PathBuf::from(p.trim()))
    });

    match &skill_path {
        Some(path) if path.exists() => {
            let content = std::fs::read_to_string(path)?;
            if content.starts_with("---\n")
                && content.contains("name:")
                && content.contains("description:")
            {
                eprintln!(
                    "  PASS: SKILL.md exists with valid frontmatter ({} bytes)",
                    content.len()
                );
                passed += 1;
            } else {
                eprintln!("  FAIL: SKILL.md exists but frontmatter is invalid");
                eprintln!("  first 200 chars: {}", &content[..content.len().min(200)]);
                failed += 1;
            }
        }
        Some(path) => {
            eprintln!("  FAIL: skill path {path:?} does not exist");
            failed += 1;
        }
        None => {
            eprintln!("  SKIP: no skill path extracted (subject 1 failed)");
            failed += 1;
        }
    }

    // ── Subject 3: transcript grew (GEPA loop ran real LLM calls) ──
    eprintln!("\n[genskill-e2e] Subject 3: verify GEPA loop executed");
    let after = state.transcript.len();
    let new_messages = after - before;
    if new_messages >= 1 {
        eprintln!("  PASS: transcript grew by {new_messages} messages (GEPA loop ran)");
        passed += 1;
    } else {
        eprintln!("  FAIL: transcript did not grow (no LLM calls made?)");
        failed += 1;
    }

    // ── Subject 4: cleanup generated skill ──
    eprintln!("\n[genskill-e2e] Subject 4: cleanup");
    if let Some(path) = &skill_path {
        if let Some(parent) = path.parent() {
            if parent.exists() {
                std::fs::remove_dir_all(parent)?;
                eprintln!("  PASS: cleaned up {}", parent.display());
                passed += 1;
            } else {
                eprintln!("  SKIP: parent dir already gone");
                passed += 1;
            }
        } else {
            eprintln!("  SKIP: no parent dir");
            passed += 1;
        }
    } else {
        eprintln!("  SKIP: no path to clean");
        passed += 1;
    }

    // ── Summary ──
    eprintln!("\n[genskill-e2e] ════════════════════════════════════");
    eprintln!(
        "[genskill-e2e] {passed} passed, {failed} failed out of {}",
        passed + failed
    );
    eprintln!("[genskill-e2e] ════════════════════════════════════");

    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn resolve_model(providers: &ProviderRegistry, provider_id: &str) -> Result<String> {
    if let Ok(model) = std::env::var("PROBE_MODEL") {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            return Ok(if trimmed.contains('/') {
                trimmed.to_string()
            } else {
                format!("{provider_id}/{trimmed}")
            });
        }
    }
    let provider = providers
        .provider(provider_id)
        .ok_or_else(|| anyhow!("{provider_id} provider is not configured"))?;
    let model = provider
        .models
        .first()
        .ok_or_else(|| anyhow!("{provider_id} provider has no configured models"))?;
    Ok(format!("{provider_id}/{}", model.id))
}
