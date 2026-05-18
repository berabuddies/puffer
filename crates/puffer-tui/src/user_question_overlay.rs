use crate::list_selection_view::{ListSelectionView, SelectionItem};
use anyhow::{Context, Result};
use puffer_core::UserQuestionPromptResponse;
use serde::Deserialize;
use serde_json::{Map, Value};

/// One parsed `AskUserQuestion` prompt shown by the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserQuestion {
    header: String,
    question: String,
    options: Vec<UserQuestionOption>,
    multi_select: bool,
}

/// One selectable answer option in an `AskUserQuestion` prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserQuestionOption {
    label: String,
    description: String,
    preview: Option<String>,
}

/// Modal list state for answering `AskUserQuestion` prompts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserQuestionOverlay {
    questions: Vec<UserQuestion>,
    question_index: usize,
    lists: Vec<ListSelectionView>,
    selected_multi: Vec<Vec<usize>>,
    answers: Map<String, Value>,
}

impl UserQuestionOverlay {
    /// Builds an overlay from the runtime `questions` payload.
    pub(crate) fn from_value(value: Value) -> Result<Self> {
        let raw_questions: Vec<RawUserQuestion> =
            serde_json::from_value(value).context("invalid AskUserQuestion questions")?;
        let questions = raw_questions
            .into_iter()
            .map(|raw| UserQuestion {
                header: raw.header,
                question: raw.question,
                options: raw
                    .options
                    .into_iter()
                    .map(|option| UserQuestionOption {
                        label: option.label,
                        description: option.description,
                        preview: option.preview,
                    })
                    .collect(),
                multi_select: raw.multi_select,
            })
            .collect::<Vec<_>>();
        let lists = questions
            .iter()
            .map(|question| {
                ListSelectionView::new(
                    question
                        .options
                        .iter()
                        .enumerate()
                        .map(|(index, option)| SelectionItem {
                            label: option.label.clone(),
                            description: Some(option.description.clone()),
                            shortcuts: number_shortcut(index).into_iter().collect(),
                        })
                        .collect(),
                )
            })
            .collect::<Vec<_>>();
        let selected_multi = vec![Vec::new(); questions.len()];
        Ok(Self {
            questions,
            question_index: 0,
            lists,
            selected_multi,
            answers: Map::new(),
        })
    }

    /// Returns an empty response used when the prompt is dismissed.
    pub(crate) fn empty_response() -> UserQuestionPromptResponse {
        UserQuestionPromptResponse {
            answers: Map::new(),
            annotations: Map::new(),
        }
    }

    /// Returns the currently selected row index.
    pub(crate) fn selection(&self) -> usize {
        self.current_list()
            .map(ListSelectionView::selection)
            .unwrap_or(0)
    }

    /// Returns the title for the active question.
    pub(crate) fn title(&self) -> String {
        let Some(question) = self.current_question() else {
            return "Question".to_string();
        };
        let progress = if self.questions.len() > 1 {
            format!(" ({}/{})", self.question_index + 1, self.questions.len())
        } else {
            String::new()
        };
        format!("{}: {}{}", question.header, question.question, progress)
    }

    /// Returns renderable option rows for the active question.
    pub(crate) fn rows(&self) -> Vec<(bool, String)> {
        let Some(question) = self.current_question() else {
            return Vec::new();
        };
        let selection = self.selection();
        question
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                let marker = if question.multi_select {
                    if self.selected_multi[self.question_index].contains(&index) {
                        "[x] "
                    } else {
                        "[ ] "
                    }
                } else {
                    ""
                };
                let text = if option.description.trim().is_empty() {
                    format!("{marker}{}", option.label)
                } else {
                    format!("{marker}{}  {}", option.label, option.description)
                };
                (index == selection, text)
            })
            .collect()
    }

    /// Returns the preview for the active single-select option.
    pub(crate) fn selected_preview(&self) -> Option<&str> {
        let question = self.current_question()?;
        if question.multi_select {
            return None;
        }
        question
            .options
            .get(self.selection())?
            .preview
            .as_deref()
            .filter(|preview| !preview.trim().is_empty())
    }

    /// Moves the selection upward.
    pub(crate) fn select_previous(&mut self) {
        if let Some(list) = self.current_list_mut() {
            list.select_previous();
        }
    }

    /// Moves the selection downward.
    pub(crate) fn select_next(&mut self) {
        if let Some(list) = self.current_list_mut() {
            list.select_next();
        }
    }

    /// Moves the selection upward by one page.
    pub(crate) fn page_up(&mut self) {
        if let Some(list) = self.current_list_mut() {
            list.page_up();
        }
    }

    /// Moves the selection downward by one page.
    pub(crate) fn page_down(&mut self) {
        if let Some(list) = self.current_list_mut() {
            list.page_down();
        }
    }

    /// Toggles the highlighted option for the active multi-select question.
    pub(crate) fn toggle_current(&mut self) {
        let Some(question) = self.current_question() else {
            return;
        };
        if !question.multi_select {
            return;
        }
        let option_index = self.selection();
        let selected = &mut self.selected_multi[self.question_index];
        if let Some(position) = selected.iter().position(|index| *index == option_index) {
            selected.remove(position);
        } else {
            selected.push(option_index);
            selected.sort_unstable();
        }
    }

    /// Activates the option matching a numeric shortcut.
    pub(crate) fn activate_shortcut(&mut self, key: char) -> Option<UserQuestionPromptResponse> {
        self.current_list_mut()?.select_shortcut(key)?;
        if self.current_question()?.multi_select {
            self.toggle_current();
            return None;
        }
        self.confirm_current()
    }

    /// Confirms the active question and returns a response when all questions are answered.
    pub(crate) fn confirm_current(&mut self) -> Option<UserQuestionPromptResponse> {
        let question_index = self.question_index;
        let question = self.questions.get(question_index)?.clone();
        let selection = self.selection();
        let answer = if question.multi_select {
            if self.selected_multi[question_index].is_empty() {
                self.selected_multi[question_index].push(selection);
            }
            let values = self.selected_multi[question_index]
                .iter()
                .filter_map(|index| question.options.get(*index))
                .map(|option| Value::String(option.label.clone()))
                .collect::<Vec<_>>();
            Value::Array(values)
        } else {
            let option = question.options.get(selection)?;
            Value::String(option.label.clone())
        };
        self.answers.insert(question.question, answer);
        if question_index + 1 < self.questions.len() {
            self.question_index += 1;
            return None;
        }
        Some(UserQuestionPromptResponse {
            answers: self.answers.clone(),
            annotations: Map::new(),
        })
    }

    fn current_question(&self) -> Option<&UserQuestion> {
        self.questions.get(self.question_index)
    }

    fn current_list(&self) -> Option<&ListSelectionView> {
        self.lists.get(self.question_index)
    }

    fn current_list_mut(&mut self) -> Option<&mut ListSelectionView> {
        self.lists.get_mut(self.question_index)
    }
}

#[derive(Debug, Deserialize)]
struct RawUserQuestion {
    question: String,
    header: String,
    options: Vec<RawUserQuestionOption>,
    #[serde(default, rename = "multiSelect")]
    multi_select: bool,
}

#[derive(Debug, Deserialize)]
struct RawUserQuestionOption {
    label: String,
    description: String,
    #[serde(default)]
    preview: Option<String>,
}

fn number_shortcut(index: usize) -> Option<char> {
    if index < 9 {
        char::from_digit((index + 1) as u32, 10)
    } else {
        None
    }
}
