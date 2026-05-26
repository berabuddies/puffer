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
fn slash_completion_fills_workflow_new_connector_arguments() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_str("/workflows new email events", &commands);

    assert!(tui.apply_selected_command(&commands));
    assert_eq!(tui.input, "/workflows new email-workflow email");
    assert_eq!(tui.cursor, tui.input.len());
}

#[test]
fn enter_completion_fills_workflow_append_connector_arguments() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_str("/workflows append vote poll", &commands);

    assert!(tui.complete_on_enter(&commands));
    assert_eq!(
        tui.input,
        "/workflows append telegram-user /tmp/telegram-user.log --connector telegram-login"
    );
}

#[test]
fn enter_completion_fills_workflow_append_query_path_and_pattern() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_str(
        "/workflows append telegram-user support ping /tmp/support",
        &commands,
    );

    assert!(tui.complete_on_enter(&commands));
    assert_eq!(
        tui.input,
        "/workflows append telegram-user /tmp/support 'support ping' --connector telegram-login"
    );
}

#[test]
fn enter_completion_fills_monitor_connector_arguments() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_str("/monitor vote poll", &commands);

    assert!(tui.complete_on_enter(&commands));
    assert_eq!(tui.input, "/monitor telegram-user");
}

#[test]
fn enter_completion_lets_exact_monitor_connection_submit() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_str("/monitor telegram-user", &commands);

    assert!(!tui.complete_on_enter(&commands));
    assert_eq!(tui.input, "/monitor telegram-user");
}

#[test]
fn slash_completion_fills_connect_catalog_rows() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_str("/connect email", &commands);

    assert!(tui.apply_selected_command(&commands));
    assert_eq!(tui.input, "/connect email email");
    assert_eq!(tui.cursor, tui.input.len());
}

#[test]
fn enter_completion_fills_partial_connect_catalog_rows() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_str("/connect telegram personal", &commands);

    assert!(tui.complete_on_enter(&commands));
    assert_eq!(tui.input, "/connect telegram-login personal");
}

#[test]
fn enter_completion_preserves_connect_name_after_connector_search() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_str("/connect matrix matrix-main", &commands);

    assert!(tui.complete_on_enter(&commands));
    assert_eq!(tui.input, "/connect matrix-bot matrix-main");
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

#[test]
fn render_shows_connect_catalog_popup_after_command_space() {
    let backend = TestBackend::new(120, 30);
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
                "/connect telegram personal",
                26,
                0,
                0,
                &supported_commands(),
            )
        })
        .unwrap();
    let rendered = buffer_to_string(terminal.backend().buffer());
    assert!(rendered.contains("/connect telegram-login"));
    assert!(rendered.contains("connection=telegram-user"));
    assert!(rendered.contains("Telegram personal account"));
}

#[test]
fn render_shows_workflow_connector_argument_popup() {
    let backend = TestBackend::new(130, 30);
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
                "/workflows append vote poll",
                27,
                0,
                0,
                &supported_commands(),
            )
        })
        .unwrap();
    let rendered = buffer_to_string(terminal.backend().buffer());
    assert!(rendered.contains("/workflows append telegram-user"));
    assert!(rendered.contains("connection=telegram-user"));
    assert!(rendered.contains("Append events from Telegram personal account"));
}

#[test]
fn render_shows_monitor_connector_argument_popup() {
    let backend = TestBackend::new(120, 30);
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
                "/monitor vote poll",
                18,
                0,
                0,
                &supported_commands(),
            )
        })
        .unwrap();
    let rendered = buffer_to_string(terminal.backend().buffer());
    assert!(rendered.contains("/monitor telegram-user"));
    assert!(rendered.contains("connection=telegram-user"));
    assert!(rendered.contains("Monitor events from Telegram personal account"));
}
