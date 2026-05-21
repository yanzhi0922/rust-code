//! UI primitives component for the TUI.
//!
//! Provides low-level UI building blocks: dividers, bylines, list items,
//! loading states, and pane containers. Mirrors `cc-haha/src/components/design-system/`.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`render_divider`] | Horizontal divider line |
//! | [`render_byline`] | Byline with left/right text |
//! | [`render_list_item`] | Bulleted list item |
//! | [`render_loading_state`] | Loading spinner with message |
//! | [`render_pane_header`] | Pane header with title |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// Render functions
// ---------------------------------------------------------------------------

fn dim_span(text: &str) -> Span<'static> {
    Span::styled(
        text.to_owned(),
        Style::default().add_modifier(Modifier::DIM),
    )
}

/// Render a horizontal divider line.
pub fn render_divider(width: usize, _style: &StyleConfig) -> Vec<Line<'static>> {
    let line: String = "─".repeat(width.min(80));
    vec![Line::from(Span::styled(
        format!(" {line}"),
        Style::default().fg(Color::DarkGray),
    ))]
}

/// Render a byline with left-aligned label and right-aligned detail.
pub fn render_byline(
    left: &str,
    right: &str,
    total_width: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let left_len = left.len();
    let right_len = right.len();
    let padding = if total_width > left_len + right_len + 2 {
        total_width - left_len - right_len - 2
    } else {
        1
    };

    vec![Line::from(vec![
        Span::styled(
            format!(" {left}"),
            Style::default()
                .fg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(padding), Style::default()),
        Span::styled(
            right.to_owned(),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ])]
}

/// Render a bulleted list item.
pub fn render_list_item(
    bullet: &str,
    text: &str,
    is_selected: bool,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let spans = if is_selected {
        vec![
            Span::styled(
                format!(" {bullet} "),
                Style::default()
                    .fg(style.accent_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                text.to_owned(),
                Style::default()
                    .fg(style.accent_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]
    } else {
        vec![
            Span::styled(
                format!(" {bullet} "),
                Style::default().add_modifier(Modifier::DIM),
            ),
            Span::styled(text.to_owned(), Style::default().fg(style.status_fg)),
        ]
    };

    vec![Line::from(spans)]
}

/// Render a loading state with spinner animation.
pub fn render_loading_state(
    message: &str,
    frame: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let idx = frame % spinner_chars.len();
    let spinner = spinner_chars[idx];

    vec![Line::from(vec![
        Span::styled(
            format!(" {spinner} "),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(message.to_owned(), Style::default().fg(style.status_fg)),
    ])]
}

/// Render a pane header with title and optional subtitle.
pub fn render_pane_header(
    title: &str,
    subtitle: Option<&str>,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut spans = vec![Span::styled(
        format!(" {title}"),
        Style::default()
            .fg(style.accent_color)
            .add_modifier(Modifier::BOLD),
    )];

    if let Some(sub) = subtitle {
        spans.push(dim_span(&format!(" — {sub}")));
    }

    let mut lines = vec![Line::from(spans)];
    lines.push(Line::from(Span::styled(
        " ─────────────────────────────────────────".to_owned(),
        Style::default().fg(Color::DarkGray),
    )));
    lines
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_style() -> StyleConfig {
        StyleConfig::dark()
    }

    #[test]
    fn divider_renders_line() {
        let lines = render_divider(40, &test_style());
        assert_eq!(lines.len(), 1);
        let text = lines[0].to_string();
        assert!(text.contains('─'));
    }

    #[test]
    fn divider_clamps_width() {
        let lines = render_divider(200, &test_style());
        let text = lines[0].to_string();
        // 80 chars of ─ + space prefix; UTF-8 ─ is 3 bytes each
        assert!(text.chars().count() <= 82);
    }

    #[test]
    fn byline_renders_both_sides() {
        let lines = render_byline("Left", "Right", 40, &test_style());
        assert_eq!(lines.len(), 1);
        let text = lines[0].to_string();
        assert!(text.contains("Left"));
        assert!(text.contains("Right"));
    }

    #[test]
    fn byline_handles_narrow_width() {
        let lines = render_byline("LongLeftText", "Right", 5, &test_style());
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn list_item_selected() {
        let lines = render_list_item("•", "Hello", true, &test_style());
        assert_eq!(lines.len(), 1);
        let text = lines[0].to_string();
        assert!(text.contains("Hello"));
    }

    #[test]
    fn list_item_not_selected() {
        let lines = render_list_item("•", "World", false, &test_style());
        assert_eq!(lines.len(), 1);
        let text = lines[0].to_string();
        assert!(text.contains("World"));
    }

    #[test]
    fn loading_state_shows_message() {
        let lines = render_loading_state("Loading data…", 0, &test_style());
        assert_eq!(lines.len(), 1);
        let text = lines[0].to_string();
        assert!(text.contains("Loading data"));
    }

    #[test]
    fn loading_state_cycles_spinner() {
        let l0 = render_loading_state("T", 0, &test_style())[0].to_string();
        let l5 = render_loading_state("T", 5, &test_style())[0].to_string();
        assert_ne!(l0, l5);
    }

    #[test]
    fn pane_header_title_only() {
        let lines = render_pane_header("My Panel", None, &test_style());
        assert_eq!(lines.len(), 2); // header + divider
        let text = lines[0].to_string();
        assert!(text.contains("My Panel"));
    }

    #[test]
    fn pane_header_with_subtitle() {
        let lines = render_pane_header("Panel", Some("v2"), &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("Panel"));
        assert!(text.contains("v2"));
    }

    #[test]
    fn pane_header_has_divider() {
        let lines = render_pane_header("Test", None, &test_style());
        let divider_text = lines[1].to_string();
        assert!(divider_text.contains('─'));
    }
}
