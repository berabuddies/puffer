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
    question_type: UserQuestionType,
    options: Vec<UserQuestionOption>,
    multi_select: bool,
    searchable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum UserQuestionType {
    #[default]
    Choice,
    Input,
}

/// One selectable answer option in an `AskUserQuestion` prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserQuestionOption {
    label: String,
    description: String,
    preview: Option<String>,
    search_text: String,
}

/// Modal list state for answering `AskUserQuestion` prompts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserQuestionOverlay {
    questions: Vec<UserQuestion>,
    question_index: usize,
    lists: Vec<ListSelectionView>,
    selected_multi: Vec<Vec<usize>>,
    custom_answers: Vec<String>,
    custom_answer_active: Vec<bool>,
    answers: Map<String, Value>,
}

/// Result of trying to activate a question option shortcut.
pub(crate) enum UserQuestionShortcutActivation {
    Ignored,
    Pending,
    Response(UserQuestionPromptResponse),
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
                question_type: raw.question_type,
                options: raw
                    .options
                    .into_iter()
                    .map(|option| UserQuestionOption {
                        search_text: searchable_option_text(&option.label, &option.description),
                        label: option.label,
                        description: option.description,
                        preview: option.preview,
                    })
                    .collect(),
                multi_select: raw.multi_select,
                searchable: raw.searchable,
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
        let custom_answers = vec![String::new(); questions.len()];
        let custom_answer_active = questions
            .iter()
            .map(|question| question.question_type == UserQuestionType::Input)
            .collect::<Vec<_>>();
        Ok(Self {
            questions,
            question_index: 0,
            lists,
            selected_multi,
            custom_answers,
            custom_answer_active,
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
        let custom_answer = self.current_custom_answer().trim();
        if question.question_type == UserQuestionType::Input {
            let body = if custom_answer.is_empty() {
                "Type answer".to_string()
            } else {
                custom_answer.to_string()
            };
            return vec![(true, format!("Input  {body}"))];
        }
        if question.searchable {
            let indices = self.filtered_option_indices(question, custom_answer);
            if indices.is_empty() {
                let text = if custom_answer.is_empty() {
                    "No options available".to_string()
                } else {
                    format!("No matches for {custom_answer}")
                };
                return vec![(false, text)];
            }
            return indices
                .iter()
                .enumerate()
                .filter_map(|(row_index, option_index)| {
                    question.options.get(*option_index).map(|option| {
                        let text = if option.description.trim().is_empty() {
                            option.label.clone()
                        } else {
                            format!("{}  {}", option.label, option.description)
                        };
                        (row_index == selection, text)
                    })
                })
                .collect();
        }
        let custom_active = self.is_custom_answer_active();
        let custom_selected =
            custom_active || (!question.multi_select && !custom_answer.is_empty());
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
                (index == selection && !custom_selected, text)
            })
            .chain(std::iter::once({
                let marker = if question.multi_select {
                    if custom_answer.is_empty() {
                        "[ ] "
                    } else {
                        "[x] "
                    }
                } else {
                    ""
                };
                let body = if custom_answer.is_empty() {
                    "Type a custom answer".to_string()
                } else {
                    custom_answer.to_string()
                };
                (custom_selected, format!("{marker}Other  {body}"))
            }))
            .collect()
    }

    /// Returns the preview for the active single-select option.
    pub(crate) fn selected_preview(&self) -> Option<&str> {
        let question = self.current_question()?;
        if question.question_type == UserQuestionType::Input {
            return None;
        }
        if question.multi_select {
            return None;
        }
        if !question.searchable
            && (self.is_custom_answer_active() || !self.current_custom_answer().trim().is_empty())
        {
            return None;
        }
        question
            .options
            .get(self.selected_option_index(question)?)?
            .preview
            .as_deref()
            .filter(|preview| !preview.trim().is_empty())
    }

    /// Moves the selection upward.
    pub(crate) fn select_previous(&mut self) {
        if self.current_question_type() == Some(UserQuestionType::Input) {
            self.set_custom_answer_active(true);
            return;
        }
        if self
            .current_question()
            .is_some_and(|question| question.searchable)
        {
            self.select_previous_search_result();
            return;
        }
        if self.is_custom_answer_active() {
            if let Some(list) = self.current_list_mut() {
                list.select_last();
            }
            self.set_custom_answer_active(false);
            return;
        }
        if let Some(list) = self.current_list_mut() {
            list.select_previous();
        }
    }

    /// Moves the selection downward.
    pub(crate) fn select_next(&mut self) {
        if self.current_question_type() == Some(UserQuestionType::Input) {
            self.set_custom_answer_active(true);
            return;
        }
        if self
            .current_question()
            .is_some_and(|question| question.searchable)
        {
            self.select_next_search_result();
            return;
        }
        if self.is_custom_answer_active() {
            return;
        }
        let custom_index = self.custom_row_index();
        if self.selection() >= custom_index.saturating_sub(1) {
            self.set_custom_answer_active(true);
            return;
        }
        if let Some(list) = self.current_list_mut() {
            list.select_next();
        }
    }

    /// Moves the selection upward by one page.
    pub(crate) fn page_up(&mut self) {
        if self.current_question_type() == Some(UserQuestionType::Input) {
            self.set_custom_answer_active(true);
            return;
        }
        if self
            .current_question()
            .is_some_and(|question| question.searchable)
        {
            self.page_up_search_results();
            return;
        }
        self.set_custom_answer_active(false);
        if let Some(list) = self.current_list_mut() {
            list.page_up();
        }
    }

    /// Moves the selection downward by one page.
    pub(crate) fn page_down(&mut self) {
        if self.current_question_type() == Some(UserQuestionType::Input) {
            self.set_custom_answer_active(true);
            return;
        }
        if self
            .current_question()
            .is_some_and(|question| question.searchable)
        {
            self.page_down_search_results();
            return;
        }
        if self.is_custom_answer_active() {
            return;
        }
        let before = self.selection();
        if let Some(list) = self.current_list_mut() {
            list.page_down();
        }
        if self.selection() == before
            && self.selection() >= self.custom_row_index().saturating_sub(1)
        {
            self.set_custom_answer_active(true);
        }
    }

    /// Toggles the highlighted option for the active multi-select question.
    pub(crate) fn toggle_current(&mut self) {
        let Some(question) = self.current_question() else {
            return;
        };
        if question.question_type == UserQuestionType::Input || !question.multi_select {
            return;
        }
        if self.is_custom_answer_active() {
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
    pub(crate) fn activate_shortcut(&mut self, key: char) -> UserQuestionShortcutActivation {
        if self
            .current_question()
            .is_some_and(|question| question.searchable)
        {
            return UserQuestionShortcutActivation::Ignored;
        }
        if self
            .current_list_mut()
            .and_then(|list| list.select_shortcut(key))
            .is_none()
        {
            return UserQuestionShortcutActivation::Ignored;
        }
        self.set_custom_answer_active(false);
        let Some(question) = self.current_question() else {
            return UserQuestionShortcutActivation::Ignored;
        };
        if question.multi_select {
            self.toggle_current();
            return UserQuestionShortcutActivation::Pending;
        }
        match self.confirm_current() {
            Some(response) => UserQuestionShortcutActivation::Response(response),
            None => UserQuestionShortcutActivation::Pending,
        }
    }

    /// Confirms the active question and returns a response when all questions are answered.
    pub(crate) fn confirm_current(&mut self) -> Option<UserQuestionPromptResponse> {
        let question_index = self.question_index;
        let question = self.questions.get(question_index)?.clone();
        let selection = self.selection();
        let custom = self
            .custom_answers
            .get(question_index)
            .map(|answer| answer.trim().to_string())
            .unwrap_or_default();
        let answer = if question.question_type == UserQuestionType::Input {
            if custom.is_empty() {
                return None;
            }
            Value::String(custom)
        } else if question.searchable {
            let option_index = self.selected_option_index(&question)?;
            let option = question.options.get(option_index)?;
            Value::String(option.label.clone())
        } else if question.multi_select {
            if self.selected_multi[question_index].is_empty() && custom.is_empty() {
                if self
                    .custom_answer_active
                    .get(question_index)
                    .copied()
                    .unwrap_or(false)
                {
                    return None;
                }
                self.selected_multi[question_index].push(selection);
            }
            let mut values = self.selected_multi[question_index]
                .iter()
                .filter_map(|index| question.options.get(*index))
                .map(|option| Value::String(option.label.clone()))
                .collect::<Vec<_>>();
            if !custom.is_empty() {
                values.push(Value::String(custom));
            }
            Value::Array(values)
        } else if !custom.is_empty() {
            Value::String(custom)
        } else if self
            .custom_answer_active
            .get(question_index)
            .copied()
            .unwrap_or(false)
        {
            return None;
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

    fn current_question_type(&self) -> Option<UserQuestionType> {
        self.current_question()
            .map(|question| question.question_type)
    }

    fn current_list(&self) -> Option<&ListSelectionView> {
        self.lists.get(self.question_index)
    }

    fn current_list_mut(&mut self) -> Option<&mut ListSelectionView> {
        self.lists.get_mut(self.question_index)
    }

    /// Returns the custom-answer text for the active question.
    pub(crate) fn custom_answer(&self) -> &str {
        self.current_custom_answer()
    }

    /// Inserts one character into the active custom answer.
    pub(crate) fn insert_custom_char(&mut self, ch: char) {
        self.set_custom_answer_active(true);
        if let Some(answer) = self.custom_answers.get_mut(self.question_index) {
            answer.push(ch);
        }
        self.clamp_search_selection();
    }

    /// Removes one character from the active custom answer.
    pub(crate) fn backspace_custom_answer(&mut self) {
        if let Some(answer) = self.custom_answers.get_mut(self.question_index) {
            answer.pop();
        }
        self.clamp_search_selection();
    }

    /// Returns true when the active custom answer has text.
    pub(crate) fn has_custom_answer(&self) -> bool {
        !self.current_custom_answer().is_empty()
    }

    /// Returns true when typing should edit the active custom answer.
    pub(crate) fn custom_answer_active(&self) -> bool {
        self.is_custom_answer_active()
    }

    /// Returns the composer placeholder for the active answer field.
    pub(crate) fn prompt_placeholder(&self) -> &'static str {
        if self
            .current_question()
            .is_some_and(|question| question.searchable)
        {
            return "Search options";
        }
        if self.current_question_type() == Some(UserQuestionType::Input) {
            "Type answer"
        } else {
            "Type custom answer"
        }
    }

    /// Returns true when the active question filters choices as the user types.
    pub(crate) fn is_searchable_choice(&self) -> bool {
        self.current_question()
            .is_some_and(|question| question.searchable)
    }

    /// Returns the footer hint for the active question.
    pub(crate) fn footer_hint(&self) -> &'static str {
        if self.is_searchable_choice() {
            "Type to search · Arrows to move · Enter to select · Esc to close"
        } else {
            "Use arrows or shortcuts · Enter to select · Esc to close"
        }
    }

    fn selected_option_index(&self, question: &UserQuestion) -> Option<usize> {
        if question.searchable {
            self.filtered_option_indices(question, self.current_custom_answer())
                .get(self.selection())
                .copied()
        } else {
            Some(self.selection())
        }
    }

    fn filtered_option_indices(&self, question: &UserQuestion, query: &str) -> Vec<usize> {
        let query = normalize_search_query(query);
        let terms = query.split_whitespace().collect::<Vec<_>>();
        question
            .options
            .iter()
            .enumerate()
            .filter(|(_, option)| {
                terms.is_empty() || terms.iter().all(|term| option.search_text.contains(term))
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn filtered_option_count(&self) -> usize {
        self.current_question()
            .map(|question| {
                self.filtered_option_indices(question, self.current_custom_answer())
                    .len()
            })
            .unwrap_or(0)
    }

    fn select_search_index(&mut self, index: usize) {
        let count = self.filtered_option_count();
        let selection = index.min(count.saturating_sub(1));
        if let Some(list) = self.current_list_mut() {
            list.select_index(selection);
        }
    }

    fn select_previous_search_result(&mut self) {
        self.select_search_index(self.selection().saturating_sub(1));
    }

    fn select_next_search_result(&mut self) {
        self.select_search_index(self.selection() + 1);
    }

    fn page_up_search_results(&mut self) {
        self.select_search_index(self.selection().saturating_sub(10));
    }

    fn page_down_search_results(&mut self) {
        self.select_search_index(self.selection() + 10);
    }

    fn clamp_search_selection(&mut self) {
        if self.is_searchable_choice() {
            self.select_search_index(self.selection());
        }
    }

    fn current_custom_answer(&self) -> &str {
        self.custom_answers
            .get(self.question_index)
            .map(String::as_str)
            .unwrap_or("")
    }

    fn custom_row_index(&self) -> usize {
        self.current_question()
            .map(|question| question.options.len())
            .unwrap_or(0)
    }

    fn is_custom_answer_active(&self) -> bool {
        self.custom_answer_active
            .get(self.question_index)
            .copied()
            .unwrap_or(false)
    }

    fn set_custom_answer_active(&mut self, active: bool) {
        if let Some(value) = self.custom_answer_active.get_mut(self.question_index) {
            *value = active;
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawUserQuestion {
    question: String,
    header: String,
    #[serde(default, rename = "type")]
    question_type: UserQuestionType,
    #[serde(default)]
    options: Vec<RawUserQuestionOption>,
    #[serde(default, rename = "multiSelect")]
    multi_select: bool,
    #[serde(default)]
    searchable: bool,
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

fn normalize_search_query(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn searchable_option_text(label: &str, description: &str) -> String {
    format!("{} {}", label.trim(), description.trim()).to_ascii_lowercase()
}
