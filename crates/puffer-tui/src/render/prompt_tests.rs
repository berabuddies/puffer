use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn render_long_prompt_keeps_cursor_tail_visible() {
    let backend = TestBackend::new(44, 18);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = tests::sample_state();
    let resources = tests::sample_resources();
    let providers = tests::sample_providers();
    let auth_store = tests::sample_auth_store();
    let input = format!("{} tail-visible", "中文".repeat(16));

    terminal
        .draw(|frame| {
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                &input,
                input.len(),
                0,
                0,
                &tests::sample_commands(),
            )
        })
        .unwrap();

    let rendered = tests::terminal_view(&terminal);
    assert!(rendered.contains("tail-visible"));
}
