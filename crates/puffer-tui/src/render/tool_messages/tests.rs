use super::*;
use ratatui::style::Color;
use ratatui::text::Line;

fn render_with_pulse(text: &str, pulse_running: bool) -> Vec<Line<'static>> {
    render_tool_message(text, false, pulse_running).expect("tool message")
}

#[test]
fn bash_tool_message_renders_claude_style_header() {
    let rendered = render_with_pulse(
        "Tool bash [ok]\ninput: {\"command\":\"python --version 2>/dev/null || python3 --version\"}\nPython 3.12.1",
        false,
    );
    assert_eq!(
        rendered[0].to_string(),
        "● Bash python --version 2>/dev/null || python3 --version"
    );
    assert_eq!(rendered[1].to_string(), "└ Python 3.12.1");
    assert_eq!(rendered[0].spans[0].style.fg, Some(Color::LightGreen));
    assert_eq!(rendered[0].spans[1].style.fg, Some(ORANGE_ACCENT));
}

#[test]
fn failed_tool_message_uses_red_indicator() {
    let rendered = render_with_pulse(
        "Tool bash [error]\ninput: {\"command\":\"false\"}\nexit status 1",
        false,
    );
    assert_eq!(rendered[0].spans[0].style.fg, Some(Color::LightRed));
}

#[test]
fn long_output_is_compacted_to_first_hidden_last_two_lines() {
    let rendered = render_with_pulse(
        "Tool bash [ok]\ninput: {\"command\":\"printf 'a\\nb\\nc\\nd'\"}\na\nb\nc\nd",
        false,
    );
    let text = rendered
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        text,
        vec![
            "● Bash printf 'a b c d'".to_string(),
            "└ a".to_string(),
            "  [+1 lines · press Ctrl+O to expand]".to_string(),
            "  c".to_string(),
            "  d".to_string(),
        ]
    );
}

#[test]
fn read_tool_message_shows_file_content_instead_of_json() {
    let rendered = render_with_pulse(
        "Tool Read [ok]\ninput: {\"file_path\":\"/tmp/demo.txt\"}\n{\n  \"type\": \"text\",\n  \"file\": {\n    \"filePath\": \"/tmp/demo.txt\",\n    \"content\": \"     1\\thello\\n     2\\tworld\\n\",\n    \"numLines\": 2,\n    \"startLine\": 1,\n    \"totalLines\": 2\n  }\n}",
        false,
    );
    let text = rendered
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        text,
        vec![
            "● Read /tmp/demo.txt".to_string(),
            "└      1  hello".to_string(),
            "       2  world".to_string(),
        ]
    );
}

#[test]
fn tool_output_sanitizes_tabs_and_control_characters() {
    assert_eq!(
        sanitize_display_line("     1\thello\r\x1b[31m"),
        "     1  hello"
    );
}

#[test]
fn web_fetch_message_shows_result_instead_of_json() {
    let rendered = render_with_pulse(
        "Tool WebFetch [ok]\ninput: {\"url\":\"https://example.com\"}\n{\n  \"bytes\": 100,\n  \"code\": 200,\n  \"codeText\": \"OK\",\n  \"result\": \"Line one\\nLine two\",\n  \"durationMs\": 12,\n  \"url\": \"https://example.com\"\n}",
        false,
    );
    let text = rendered
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        text,
        vec![
            "● Web Fetch https://example.com".to_string(),
            "└ Line one".to_string(),
            "  Line two".to_string(),
        ]
    );
}

#[test]
fn ask_user_question_output_shows_selected_answer() {
    let rendered = render_tool_message(
        "Tool AskUserQuestion [ok]\ninput: {\"questions\":[{\"question\":\"Pick one\",\"header\":\"Mode\",\"options\":[{\"label\":\"Fast\",\"description\":\"Prioritize speed\"},{\"label\":\"Careful\",\"description\":\"Prioritize review\"}]}]}\n{\"questions\":[{\"question\":\"Pick one\",\"header\":\"Mode\",\"options\":[{\"label\":\"Fast\",\"description\":\"Prioritize speed\"},{\"label\":\"Careful\",\"description\":\"Prioritize review\"}]}],\"answers\":{\"Pick one\":\"Careful\"},\"annotations\":{},\"metadata\":{},\"pending\":false,\"pendingFile\":\"/tmp/pending_questions.json\"}",
        false,
        false,
    )
    .expect("rendered");
    let text = rendered
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        text,
        vec![
            "● Ask User Question Pick one".to_string(),
            "└ Pick one: Careful".to_string(),
        ]
    );
}

#[test]
fn expanded_tool_message_shows_full_decoded_output() {
    let rendered = render_tool_message(
        "Tool WebSearch [ok]\ninput: {\"query\":\"rust tui streaming\"}\nRust TUI streaming guide",
        true,
        false,
    )
    .expect("expanded tool message");
    assert_eq!(
        rendered
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>(),
        vec![
            "● Web Search rust tui streaming".to_string(),
            "└ Rust TUI streaming guide".to_string(),
        ]
    );
}

#[test]
fn telegram_login_submit_password_redacts_value_in_header() {
    let rendered = render_tool_message(
        "Tool TelegramLoginSubmitPassword [ok]\ninput: {\"password\":\"hunter2\"}\n{\"status\":\"submitted\"}",
        false,
        false,
    )
    .expect("rendered");
    let header = rendered[0].to_string();
    assert!(header.contains("Telegram Login (2FA)"), "got: {header}");
    assert!(header.contains("redacted"), "got: {header}");
    assert!(
        !header.contains("hunter2"),
        "header leaked password: {header}"
    );
}

#[test]
fn consolidated_telegram_tool_redacts_login_secrets_in_header() {
    let rendered = render_tool_message(
        "Tool Telegram [ok]\ninput: {\"action\":\"login_submit_code\",\"code\":\"12345\"}\n{\"status\":\"complete\"}",
        false,
        false,
    )
    .expect("rendered");
    let header = rendered[0].to_string();
    assert!(header.contains("Telegram"), "got: {header}");
    assert!(header.contains("code redacted"), "got: {header}");
    assert!(!header.contains("12345"), "header leaked code: {header}");
}

#[test]
fn email_configure_header_shows_username_only() {
    let rendered = render_tool_message(
        "Tool EmailConfigure [ok]\ninput: {\"imap_host\":\"imap.example.com\",\"smtp_host\":\"smtp.example.com\",\"username\":\"alice@example.com\",\"password\":\"sekret\",\"from_address\":\"alice@example.com\"}\n{\"status\":\"configured\"}",
        false,
        false,
    )
    .expect("rendered");
    let header = rendered[0].to_string();
    assert!(header.contains("alice@example.com"));
    assert!(!header.contains("sekret"), "header leaked password");
    assert!(!header.contains("imap.example.com"), "header leaked host");
}

#[test]
fn subscription_create_output_summarizes_topic_and_action() {
    let rendered = render_tool_message(
        "Tool SubscriptionCreate [ok]\ninput: {\"id\":\"ioc-watch\"}\n{\"id\":\"ioc-watch\",\"source_topic\":\"telegram-user\",\"action\":{\"type\":\"sqlite_insert\",\"path\":\"/tmp/x.db\",\"table\":\"t\"}}",
        true,
        false,
    )
    .expect("rendered");
    let body: Vec<String> = rendered.into_iter().map(|l| l.to_string()).collect();
    assert!(body[0].contains("ioc-watch"), "got: {body:?}");
    assert!(
        body.iter()
            .any(|l| l.contains("telegram-user") && l.contains("sqlite_insert")),
        "got: {body:?}"
    );
}

#[test]
fn subscription_list_output_lists_each_subscription() {
    let rendered = render_tool_message(
        "Tool SubscriptionList [ok]\ninput: {}\n{\"subscriptions\":[{\"id\":\"a\",\"status\":\"enabled\",\"source_topic\":\"telegram-user\",\"action\":{\"type\":\"sqlite_insert\"}},{\"id\":\"b\",\"status\":\"paused\",\"source_topic\":\"email\",\"action\":{\"type\":\"forward_message\"}}],\"running_subscribers\":[\"telegram-user\"]}",
        true,
        false,
    )
    .expect("rendered");
    let body: Vec<String> = rendered.into_iter().map(|l| l.to_string()).collect();
    assert!(body
        .iter()
        .any(|l| l.contains("a") && l.contains("enabled")));
    assert!(body.iter().any(|l| l.contains("b") && l.contains("paused")));
    assert!(body
        .iter()
        .any(|l| l.contains("running subscribers") && l.contains("telegram-user")));
}
