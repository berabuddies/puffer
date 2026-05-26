use super::*;

#[test]
fn ask_user_question_rejects_duplicate_question_text() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error =
        crate::runtime::claude_tools::workflow::ask_user_question::execute_ask_user_question(
            &mut state,
            &cwd,
            json!({
                "questions": [
                    {
                        "question": "Pick one",
                        "header": "choice",
                        "options": [
                            {"label": "A", "description": "A"},
                            {"label": "B", "description": "B"}
                        ]
                    },
                    {
                        "question": "Pick one",
                        "header": "second",
                        "options": [
                            {"label": "C", "description": "C"},
                            {"label": "D", "description": "D"}
                        ]
                    }
                ]
            }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("question texts must be unique"));
}

#[test]
fn ask_user_question_accepts_input_question_without_options() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = crate::runtime::with_user_question_prompt_handler(
        |_request| crate::runtime::UserQuestionPromptResponse {
            answers: serde_json::Map::from_iter([(
                "What phone number should Telegram use?".to_string(),
                json!("+15551234567"),
            )]),
            annotations: serde_json::Map::new(),
        },
        || {
            crate::runtime::claude_tools::workflow::ask_user_question::execute_ask_user_question(
                &mut state,
                &cwd,
                json!({
                    "questions": [
                        {
                            "type": "input",
                            "question": "What phone number should Telegram use?",
                            "header": "Phone"
                        }
                    ]
                }),
            )
        },
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["pending"], false);
    assert_eq!(
        parsed["answers"]["What phone number should Telegram use?"],
        "+15551234567"
    );
    assert_eq!(parsed["questions"][0]["type"], "input");
}

#[test]
fn ask_user_question_rejects_input_question_options() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error =
        crate::runtime::claude_tools::workflow::ask_user_question::execute_ask_user_question(
            &mut state,
            &cwd,
            json!({
                "questions": [
                    {
                        "type": "input",
                        "question": "What phone number should Telegram use?",
                        "header": "Phone",
                        "options": [
                            {"label": "Other", "description": "Type the value"}
                        ]
                    }
                ]
            }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("must not provide options"));
}

#[test]
fn ask_user_question_accepts_searchable_choice_question() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = crate::runtime::with_user_question_prompt_handler(
        |_request| crate::runtime::UserQuestionPromptResponse {
            answers: serde_json::Map::from_iter([(
                "Which connector should Puffer connect?".to_string(),
                json!("telegram-login"),
            )]),
            annotations: serde_json::Map::new(),
        },
        || {
            crate::runtime::claude_tools::workflow::ask_user_question::execute_ask_user_question(
                &mut state,
                &cwd,
                json!({
                    "questions": [
                        {
                            "type": "choice",
                            "question": "Which connector should Puffer connect?",
                            "header": "Connector",
                            "searchable": true,
                            "options": [
                                {"label": "email", "description": "Email over IMAP and SMTP"},
                                {"label": "slack-login", "description": "Slack user account"},
                                {"label": "telegram-login", "description": "Telegram personal account"},
                                {"label": "webhook", "description": "HTTP webhook ingress"},
                                {"label": "matrix", "description": "Matrix room connector"}
                            ]
                        }
                    ]
                }),
            )
        },
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["pending"], false);
    assert_eq!(
        parsed["answers"]["Which connector should Puffer connect?"],
        "telegram-login"
    );
    assert_eq!(parsed["questions"][0]["searchable"], true);
}

#[test]
fn ask_user_question_rejects_searchable_multi_select_question() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error =
        crate::runtime::claude_tools::workflow::ask_user_question::execute_ask_user_question(
            &mut state,
            &cwd,
            json!({
                "questions": [
                    {
                        "question": "Which connectors should Puffer connect?",
                        "header": "Connectors",
                        "searchable": true,
                        "multiSelect": true,
                        "options": [
                            {"label": "email", "description": "Email over IMAP and SMTP"},
                            {"label": "telegram-login", "description": "Telegram personal account"}
                        ]
                    }
                ]
            }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("cannot use multiSelect"));
}

#[test]
fn ask_user_question_rejects_searchable_input_question() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error =
        crate::runtime::claude_tools::workflow::ask_user_question::execute_ask_user_question(
            &mut state,
            &cwd,
            json!({
                "questions": [
                    {
                        "type": "input",
                        "question": "What phone number should Puffer use?",
                        "header": "Phone",
                        "searchable": true
                    }
                ]
            }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("cannot use searchable"));
}

#[test]
fn ask_user_question_uses_prompt_handler_answers() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = crate::runtime::with_user_question_prompt_handler(
        |_request| crate::runtime::UserQuestionPromptResponse {
            answers: serde_json::Map::from_iter([(
                "Where is Lily?".to_string(),
                json!("In the garden"),
            )]),
            annotations: serde_json::Map::new(),
        },
        || {
            crate::runtime::claude_tools::workflow::ask_user_question::execute_ask_user_question(
                &mut state,
                &cwd,
                json!({
                    "questions": [
                        {
                            "question": "Where is Lily?",
                            "header": "Location",
                            "options": [
                                {"label": "In the garden", "description": "Lily is outside"},
                                {"label": "In the kitchen", "description": "Lily is inside"}
                            ]
                        }
                    ]
                }),
            )
        },
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["pending"], false);
    assert_eq!(parsed["answers"]["Where is Lily?"], "In the garden");
}
