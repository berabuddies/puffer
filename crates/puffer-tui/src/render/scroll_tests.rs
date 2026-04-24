use super::*;
use puffer_core::MessageRole;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn tall_transcript_state() -> AppState {
    let mut state = tests::sample_state();
    for index in 0..18 {
        state.push_message(
            MessageRole::Assistant,
            &format!(
                "assistant line {index}\nthis line keeps the transcript tall enough to scroll"
            ),
        );
    }
    state
}

#[test]
fn render_scrolls_top_panel_with_transcript() {
    let backend = TestBackend::new(80, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = tall_transcript_state();
    let resources = tests::sample_resources();
    let providers = tests::sample_providers();
    let auth_store = tests::sample_auth_store();

    terminal
        .draw(|frame| {
            set_follow_output(false);
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &tests::sample_commands(),
            );
            set_follow_output(true);
        })
        .unwrap();

    let rendered = tests::terminal_view(&terminal);
    let lines: Vec<&str> = rendered.lines().collect();
    assert!(lines
        .iter()
        .take(6)
        .any(|line| line.contains("Puffer Code")));
    assert!(lines.iter().take(8).any(|line| line.contains("Account")));
}

#[test]
fn render_manual_scroll_moves_top_panel_offscreen() {
    let backend = TestBackend::new(80, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = tall_transcript_state();
    let resources = tests::sample_resources();
    let providers = tests::sample_providers();
    let auth_store = tests::sample_auth_store();

    terminal
        .draw(|frame| {
            set_follow_output(false);
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                24,
                &tests::sample_commands(),
            );
            set_follow_output(true);
        })
        .unwrap();

    let rendered = tests::terminal_view(&terminal);
    assert!(!rendered.contains("╭ Puffer Code"));
    assert!(!rendered.contains("Puffer Code"));
    assert!(rendered.contains("assistant line"));
}

#[test]
fn render_follow_output_shows_latest_content_without_pinned_top_panel() {
    let backend = TestBackend::new(80, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = tall_transcript_state();
    state.push_message(
        MessageRole::Assistant,
        "latest assistant line\nthis should stay visible at the bottom",
    );
    let resources = tests::sample_resources();
    let providers = tests::sample_providers();
    let auth_store = tests::sample_auth_store();

    terminal
        .draw(|frame| {
            set_follow_output(true);
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &tests::sample_commands(),
            );
        })
        .unwrap();

    let rendered = tests::terminal_view(&terminal);
    assert!(rendered.contains("latest assistant line"));
    assert!(!rendered.contains("╭ Puffer Code"));
    assert!(!rendered.contains("Puffer Code"));
}
