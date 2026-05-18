use super::*;
use crate::text_overlay::{render_text_overlay, TextOverlay};
use crate::{ModelPickerEntry, OverlayState};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn visible_overlay_rows_reports_hidden_entries() {
    let rows = (0..6)
        .map(|index| OverlayRow {
            text: format!("model-{index}"),
            selected: index == 0,
        })
        .collect::<Vec<_>>();

    let visible = visible_overlay_rows(rows, Some(0), 3);
    let texts = visible.into_iter().map(|row| row.text).collect::<Vec<_>>();

    assert_eq!(texts, vec!["model-0", "model-1", "... 3 more"]);
}

#[test]
fn visible_overlay_rows_scrolls_with_selection() {
    let rows = (0..8)
        .map(|index| OverlayRow {
            text: format!("model-{index}"),
            selected: index == 5,
        })
        .collect::<Vec<_>>();

    let visible = visible_overlay_rows(rows, Some(5), 4);
    let texts = visible.into_iter().map(|row| row.text).collect::<Vec<_>>();

    assert_eq!(
        texts,
        vec!["... 3 above", "model-4", "model-5", "... 1 more"]
    );
}

#[test]
fn render_model_overlay_shows_overflow_indicator() {
    let backend = TestBackend::new(72, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    let overlay = OverlayState::ModelPicker {
        provider_id: "openai".to_string(),
        entries: (0..8)
            .map(|index| ModelPickerEntry {
                selector: format!("model-{index}"),
                description: format!("Model {index}"),
                command: None,
            })
            .collect(),
        selection: 0,
        onboarding: false,
    };

    terminal
        .draw(|frame| {
            render_overlay(frame, frame.area(), &overlay);
        })
        .unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(rendered.contains("model-0  Model 0"));
    assert!(rendered.contains("..."));
    assert!(rendered.contains("more"));
}

#[test]
fn render_model_entry_deduplicates_case_only_labels() {
    let entry = ModelPickerEntry {
        selector: "openai".to_string(),
        description: "OpenAI".to_string(),
        command: None,
    };

    assert_eq!(render_model_entry(&entry), "OpenAI");
}

#[test]
fn render_command_picker_uses_custom_title() {
    let backend = TestBackend::new(72, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    let overlay = OverlayState::CommandPicker {
        title: "Remove Tag?".to_string(),
        entries: vec![ModelPickerEntry {
            selector: "Yes, remove tag".to_string(),
            description: "Current tag: #review".to_string(),
            command: Some("/tag --confirm-remove review".to_string()),
        }],
        selection: 0,
    };

    terminal
        .draw(|frame| {
            render_overlay(frame, frame.area(), &overlay);
        })
        .unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(rendered.contains("Remove Tag?"));
}

#[test]
fn render_text_overlay_clamps_bottom_scroll() {
    let backend = TestBackend::new(72, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    let overlay = TextOverlay::open(
        "Config",
        (0..30)
            .map(|index| format!("line-{index:02}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    if let OverlayState::Text(text) = &overlay {
        text.scroll_to_bottom();
    }

    terminal
        .draw(|frame| {
            if let OverlayState::Text(text) = &overlay {
                render_text_overlay(frame, frame.area(), text);
            }
        })
        .unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(rendered.contains("line-29"));
    assert!(rendered.contains("Config"));
}
