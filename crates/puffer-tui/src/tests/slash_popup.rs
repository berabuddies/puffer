use super::*;
use puffer_core::supported_commands;
use ratatui::backend::TestBackend;

#[test]
fn slash_completion_fills_workflow_subcommands() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_str("/workflows app", &commands);

    assert!(tui.apply_selected_command(&commands));
    assert_eq!(tui.input, "/workflows append ");
    assert_eq!(tui.cursor, tui.input.len());
}

#[test]
fn enter_completion_fills_partial_workflow_subcommands() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_str("/workflows run", &commands);

    assert!(tui.complete_on_enter(&commands));
    assert_eq!(tui.input, "/workflows runs ");
}

#[test]
fn enter_completion_lets_exact_workflow_subcommands_submit() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_str("/workflows runs", &commands);

    assert!(!tui.complete_on_enter(&commands));
    assert_eq!(tui.input, "/workflows runs");
}

#[test]
fn render_shows_workflow_subcommand_popup_after_command_space() {
    let backend = TestBackend::new(110, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();
    terminal
        .draw(|frame| {
            render::render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "/workflows con",
                14,
                0,
                0,
                &supported_commands(),
            )
        })
        .unwrap();
    let rendered = buffer_to_string(terminal.backend().buffer());
    assert!(rendered.contains("/workflows connections"));
    assert!(rendered.contains("/workflows connectors"));
    assert!(rendered.contains("Search connector catalog"));
}
