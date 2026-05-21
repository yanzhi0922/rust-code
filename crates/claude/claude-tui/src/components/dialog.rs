//! Generic dialog component for the TUI.
//!
//! Provides reusable dialog widgets for confirmation prompts, text input,
//! and general-purpose bordered dialogs. Each component produces
//! `Vec<Line<'static>>` for ratatui rendering.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`Dialog`] | General-purpose dialog with title, content, and buttons |
//! | [`render_dialog`] | Render a [`Dialog`] into lines |
//! | [`render_confirm_dialog`] | Convenience for yes/no confirmation dialogs |
//! | [`render_input_dialog`] | Convenience for text-input dialogs |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// Dialog struct
// ---------------------------------------------------------------------------

/// A generic dialog with a title, body content, and action buttons.
///
/// # Example
///
/// ```ignore
/// let dlg = Dialog {
///     title: "Confirm".to_owned(),
///     body: vec![Line::from("Are you sure?")],
///     buttons: vec!["Yes".to_owned(), "No".to_owned()],
///     selected: 0,
///     border_color: Color::Yellow,
/// };
/// let lines = render_dialog(&dlg, &style);
/// ```
#[derive(Debug, Clone)]
pub struct Dialog {
    /// Dialog title shown in the top border.
    pub title: String,
    /// Body content lines.
    pub body: Vec<Line<'static>>,
    /// Action button labels.
    pub buttons: Vec<String>,
    /// Index of the currently selected button.
    pub selected: usize,
    /// Border color.
    pub border_color: Color,
}

impl Dialog {
    /// Create a new dialog with the given title and body.
    pub fn new(title: impl Into<String>, body: Vec<Line<'static>>) -> Self {
        Self {
            title: title.into(),
            body,
            buttons: Vec::new(),
            selected: 0,
            border_color: Color::Blue,
        }
    }

    /// Set the buttons.
    pub fn with_buttons(mut self, buttons: Vec<String>) -> Self {
        self.buttons = buttons;
        self
    }

    /// Set the selected button index.
    pub fn with_selected(mut self, idx: usize) -> Self {
        self.selected = idx;
        self
    }

    /// Set the border color.
    pub fn with_border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }
}

// ---------------------------------------------------------------------------
// Render helpers
// ---------------------------------------------------------------------------

/// Render a [`Dialog`] into ratatui lines.
///
/// The dialog is drawn with a rounded border, title, body content, and a
/// button bar at the bottom where the selected button is highlighted.
pub fn render_dialog(dialog: &Dialog, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Compute width from the widest body line or title
    let body_width = dialog.body.iter().map(|l| l.width()).max().unwrap_or(0);
    let button_width: usize = dialog
        .buttons
        .iter()
        .map(|b| b.len() + 4) // padding + brackets
        .sum();
    let inner_width = body_width.max(button_width).max(dialog.title.len()).max(20);
    let total_width = inner_width + 4; // border + padding

    // Top border with title
    let title_text = &dialog.title;
    let top_border = format!(
        "╭─ {} ─{}╮",
        title_text,
        "─".repeat(total_width.saturating_sub(title_text.len() + 4))
    );
    lines.push(Line::from(vec![Span::styled(
        top_border,
        Style::default().fg(dialog.border_color),
    )]));

    // Body lines
    for body_line in &dialog.body {
        let content_width = body_line.width();
        let padding = inner_width.saturating_sub(content_width);
        let mut spans = vec![Span::styled(
            "│ ".to_owned(),
            Style::default().fg(dialog.border_color),
        )];
        for span in body_line.spans.iter() {
            spans.push(span.clone());
        }
        spans.push(Span::styled(
            format!("{}│", " ".repeat(padding)),
            Style::default().fg(dialog.border_color),
        ));
        lines.push(Line::from(spans));
    }

    // Separator before buttons
    let sep = format!("├{}┤", "─".repeat(total_width.saturating_sub(2)));
    lines.push(Line::from(vec![Span::styled(
        sep,
        Style::default().fg(dialog.border_color),
    )]));

    // Button bar
    let mut button_spans = vec![Span::styled(
        "│ ".to_owned(),
        Style::default().fg(dialog.border_color),
    )];
    for (i, btn) in dialog.buttons.iter().enumerate() {
        if i == dialog.selected {
            button_spans.push(Span::styled(
                format!(" [{btn}] "),
                Style::default()
                    .fg(Color::Black)
                    .bg(style.accent_color)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            button_spans.push(Span::styled(
                format!(" [{btn}] "),
                Style::default().fg(style.status_fg),
            ));
        }
    }

    let used_width: usize = dialog.buttons.iter().map(|b| b.len() + 4).sum();
    let trailing = inner_width.saturating_sub(used_width);
    button_spans.push(Span::styled(
        format!("{}│", " ".repeat(trailing)),
        Style::default().fg(dialog.border_color),
    ));
    lines.push(Line::from(button_spans));

    // Bottom border
    let bottom = format!("╰{}╯", "─".repeat(total_width.saturating_sub(2)));
    lines.push(Line::from(vec![Span::styled(
        bottom,
        Style::default().fg(dialog.border_color),
    )]));

    lines
}

// ---------------------------------------------------------------------------
// Confirm dialog
// ---------------------------------------------------------------------------

/// Render a confirmation dialog with "Yes" / "No" buttons.
///
/// `selected=0` means "Yes" is highlighted, `selected=1` means "No".
pub fn render_confirm_dialog(
    title: &str,
    message: &str,
    selected: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let body: Vec<Line<'static>> = message
        .lines()
        .map(|l| {
            Line::from(Span::styled(
                l.to_owned(),
                Style::default().fg(style.status_fg),
            ))
        })
        .collect();

    let dialog = Dialog {
        title: title.to_owned(),
        body,
        buttons: vec!["Yes".to_owned(), "No".to_owned()],
        selected,
        border_color: Color::Yellow,
    };

    render_dialog(&dialog, style)
}

// ---------------------------------------------------------------------------
// Input dialog
// ---------------------------------------------------------------------------

/// Render an input dialog with a prompt, current input value, and cursor.
pub fn render_input_dialog(
    title: &str,
    prompt: &str,
    input_value: &str,
    cursor_pos: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut body = Vec::new();

    // Prompt line
    body.push(Line::from(Span::styled(
        prompt.to_owned(),
        Style::default().fg(style.status_fg),
    )));

    // Input line with cursor
    let before: String = input_value.chars().take(cursor_pos).collect();
    let at_cursor: String = input_value
        .chars()
        .nth(cursor_pos)
        .map(|c| c.to_string())
        .unwrap_or_else(|| " ".to_owned());
    let after: String = input_value.chars().skip(cursor_pos + 1).collect();

    body.push(Line::from(vec![
        Span::styled("  > ".to_owned(), Style::default().fg(style.accent_color)),
        Span::styled(before, Style::default().fg(style.status_fg)),
        Span::styled(
            at_cursor,
            Style::default()
                .fg(Color::Black)
                .bg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(after, Style::default().fg(style.status_fg)),
    ]));

    let dialog = Dialog {
        title: title.to_owned(),
        body,
        buttons: vec!["OK".to_owned(), "Cancel".to_owned()],
        selected: 0,
        border_color: Color::Cyan,
    };

    render_dialog(&dialog, style)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_style() -> StyleConfig {
        StyleConfig::dark()
    }

    // --- Dialog struct tests ---

    #[test]
    fn dialog_new_basic() {
        let dlg = Dialog::new("Test", vec![Line::from("Hello")]);
        assert_eq!(dlg.title, "Test");
        assert_eq!(dlg.body.len(), 1);
        assert!(dlg.buttons.is_empty());
        assert_eq!(dlg.selected, 0);
    }

    #[test]
    fn dialog_builder_pattern() {
        let dlg = Dialog::new("Title", vec![])
            .with_buttons(vec!["A".to_owned(), "B".to_owned()])
            .with_selected(1)
            .with_border_color(Color::Red);
        assert_eq!(dlg.buttons.len(), 2);
        assert_eq!(dlg.selected, 1);
        assert_eq!(dlg.border_color, Color::Red);
    }

    // --- render_dialog tests ---

    #[test]
    fn render_dialog_has_border() {
        let dlg = Dialog::new("My Dialog", vec![Line::from("Content")])
            .with_buttons(vec!["OK".to_owned()]);
        let lines = render_dialog(&dlg, &test_style());
        assert!(lines.len() >= 4); // top + content + separator + button + bottom
        let first = lines[0].to_string();
        assert!(first.contains("My Dialog"));
        assert!(first.starts_with('╭'));
    }

    #[test]
    fn render_dialog_shows_all_buttons() {
        let dlg = Dialog::new("Q", vec![]).with_buttons(vec![
            "Yes".to_owned(),
            "No".to_owned(),
            "Maybe".to_owned(),
        ]);
        let lines = render_dialog(&dlg, &test_style());
        // Find button line (second to last)
        let button_line = lines[lines.len() - 2].to_string();
        assert!(button_line.contains("Yes"));
        assert!(button_line.contains("No"));
        assert!(button_line.contains("Maybe"));
    }

    #[test]
    fn render_dialog_bottom_border() {
        let dlg = Dialog::new("T", vec![Line::from("x")]);
        let lines = render_dialog(&dlg, &test_style());
        let last = lines
            .last()
            .expect("dialog should have at least one line")
            .to_string();
        assert!(last.starts_with('╰'));
        assert!(last.ends_with('╯'));
    }

    #[test]
    fn render_dialog_multiline_body() {
        let body = vec![
            Line::from("Line 1"),
            Line::from("Line 2"),
            Line::from("Line 3"),
        ];
        let dlg = Dialog::new("Multi", body);
        let lines = render_dialog(&dlg, &test_style());
        // top + 3 body + separator + button + bottom = 7
        assert!(lines.len() >= 6);
    }

    // --- Confirm dialog tests ---

    #[test]
    fn confirm_dialog_yes_selected() {
        let lines = render_confirm_dialog("Confirm", "Are you sure?", 0, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("Confirm"));
        assert!(combined.contains("Are you sure?"));
        assert!(combined.contains("Yes"));
        assert!(combined.contains("No"));
    }

    #[test]
    fn confirm_dialog_no_selected() {
        let lines = render_confirm_dialog("Delete?", "Delete file?", 1, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("Delete"));
    }

    #[test]
    fn confirm_dialog_has_top_border() {
        let lines = render_confirm_dialog("Test", "msg", 0, &test_style());
        let first = lines[0].to_string();
        assert!(first.starts_with('╭'));
    }

    // --- Input dialog tests ---

    #[test]
    fn input_dialog_basic() {
        let lines = render_input_dialog("Input", "Enter name:", "hello", 2, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("Enter name:"));
        assert!(combined.contains("hello"));
        assert!(combined.contains("OK"));
        assert!(combined.contains("Cancel"));
    }

    #[test]
    fn input_dialog_empty_value() {
        let lines = render_input_dialog("Input", "Type:", "", 0, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("Type:"));
    }

    #[test]
    fn input_dialog_cursor_at_end() {
        let lines = render_input_dialog("Input", ">", "abc", 3, &test_style());
        // Should not panic with cursor beyond content
        assert!(!lines.is_empty());
    }

    #[test]
    fn input_dialog_multiline_prompt() {
        let lines = render_input_dialog("Input", "Enter value:", "test", 1, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("test"));
    }

    #[test]
    fn input_dialog_has_borders() {
        let lines = render_input_dialog("Title", "p", "v", 0, &test_style());
        let first = lines[0].to_string();
        let last = lines
            .last()
            .expect("input dialog should have at least one line")
            .to_string();
        assert!(first.starts_with('╭'));
        assert!(last.ends_with('╯'));
    }
}
