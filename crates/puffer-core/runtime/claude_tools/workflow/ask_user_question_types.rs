use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Deserialize)]
pub(super) struct AskUserQuestionInput {
    pub(super) questions: Vec<AskUserQuestionItem>,
    #[serde(default)]
    pub(super) answers: Map<String, Value>,
    #[serde(default)]
    pub(super) annotations: Map<String, Value>,
    #[serde(default)]
    pub(super) metadata: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct AskUserQuestionItem {
    pub(super) question: String,
    pub(super) header: String,
    #[serde(default, rename = "type")]
    pub(super) question_type: AskUserQuestionType,
    #[serde(default)]
    pub(super) options: Vec<AskUserQuestionOption>,
    #[serde(default, rename = "multiSelect")]
    pub(super) multi_select: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(super) searchable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub(super) enum AskUserQuestionType {
    #[default]
    Choice,
    Input,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct AskUserQuestionOption {
    pub(super) label: String,
    pub(super) description: String,
    #[serde(default)]
    pub(super) preview: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Validates the bounded question shape used by `AskUserQuestion`.
pub(super) fn validate_ask_user_questions(items: &[AskUserQuestionItem]) -> Result<()> {
    if items.is_empty() || items.len() > 4 {
        bail!("AskUserQuestion requires between 1 and 4 questions");
    }
    let mut seen_questions = std::collections::BTreeSet::new();
    for item in items {
        if item.question.trim().is_empty() {
            bail!("AskUserQuestion questions must not be empty");
        }
        if !seen_questions.insert(item.question.trim().to_ascii_lowercase()) {
            bail!("AskUserQuestion question texts must be unique");
        }
        if item.header.trim().is_empty() {
            bail!("AskUserQuestion headers must not be empty");
        }
        validate_question_options(item)?;
    }
    Ok(())
}

fn validate_question_options(item: &AskUserQuestionItem) -> Result<()> {
    match item.question_type {
        AskUserQuestionType::Choice => validate_choice_question(item),
        AskUserQuestionType::Input => validate_input_question(item),
    }
}

fn validate_choice_question(item: &AskUserQuestionItem) -> Result<()> {
    let max_options = if item.searchable || item.multi_select {
        50
    } else {
        4
    };
    let min_options = if item.searchable || item.multi_select {
        1
    } else {
        2
    };
    if item.options.len() < min_options || item.options.len() > max_options {
        bail!(
            "AskUserQuestion choice question `{}` must provide between {min_options} and {max_options} options",
            item.header
        );
    }
    if item.searchable && item.multi_select {
        bail!(
            "AskUserQuestion searchable question `{}` cannot use multiSelect",
            item.header
        );
    }
    if item.multi_select && item.options.iter().any(|option| option.preview.is_some()) {
        bail!(
            "AskUserQuestion question `{}` cannot use previews with multiSelect",
            item.header
        );
    }
    let mut seen_labels = std::collections::BTreeSet::new();
    for option in &item.options {
        if option.label.trim().is_empty() || option.description.trim().is_empty() {
            bail!(
                "AskUserQuestion question `{}` has an option with empty label or description",
                item.header
            );
        }
        if !seen_labels.insert(option.label.to_ascii_lowercase()) {
            bail!(
                "AskUserQuestion question `{}` has duplicate option labels",
                item.header
            );
        }
    }
    Ok(())
}

fn validate_input_question(item: &AskUserQuestionItem) -> Result<()> {
    if item.searchable {
        bail!(
            "AskUserQuestion input question `{}` cannot use searchable",
            item.header
        );
    }
    if item.multi_select {
        bail!(
            "AskUserQuestion input question `{}` cannot use multiSelect",
            item.header
        );
    }
    if !item.options.is_empty() {
        bail!(
            "AskUserQuestion input question `{}` must not provide options",
            item.header
        );
    }
    Ok(())
}
