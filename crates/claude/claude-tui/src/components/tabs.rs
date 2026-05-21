//! Tab bar component for the TUI.
//!
//! Provides a horizontal tab bar widget with support for labeled tabs,
//! active tab highlighting, and optional close indicators.
//!
//! # Example
//!
//! ```ignore
//! let tab_bar = TabBar {
//!     tabs: vec!["Chat".to_owned(), "Sessions".to_owned(), "Help".to_owned()],
//!     active: 0,
//!     show_close: false,
//! };
//! let lines = render_tab_bar(&tab_bar, &style);
//! ```

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// TabBar
// ---------------------------------------------------------------------------

/// A horizontal tab bar with labeled tabs and an active indicator.
#[derive(Debug, Clone)]
pub struct TabBar {
    /// Tab labels.
    pub tabs: Vec<String>,
    /// Index of the currently active tab.
    pub active: usize,
    /// Whether to show close indicators on tabs.
    pub show_close: bool,
}

impl TabBar {
    /// Create a new tab bar with the given labels.
    pub fn new(tabs: Vec<String>) -> Self {
        Self {
            tabs,
            active: 0,
            show_close: false,
        }
    }

    /// Set the active tab index.
    pub fn with_active(mut self, idx: usize) -> Self {
        self.active = idx;
        self
    }

    /// Enable close indicators on tabs.
    pub fn with_close(mut self, show: bool) -> Self {
        self.show_close = show;
        self
    }

    /// Get the number of tabs.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Check if there are no tabs.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Move to the next tab (wraps around).
    pub fn next(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
    }

    /// Move to the previous tab (wraps around).
    pub fn prev(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render a tab bar into ratatui lines.
///
/// The active tab is highlighted with the accent color background. Inactive
/// tabs are rendered with a dim style. Tabs are separated by `│` characters.
pub fn render_tab_bar(tab_bar: &TabBar, style: &StyleConfig) -> Vec<Line<'static>> {
    if tab_bar.tabs.is_empty() {
        return vec![Line::from(vec![Span::styled(
            " (no tabs) ",
            Style::default()
                .fg(style.info_color)
                .add_modifier(Modifier::DIM),
        )])];
    }

    let mut spans = Vec::new();

    for (i, tab_label) in tab_bar.tabs.iter().enumerate() {
        let is_active = i == tab_bar.active;

        // Separator between tabs
        if i > 0 {
            spans.push(Span::styled(
                " │ ".to_owned(),
                Style::default().fg(style.info_color),
            ));
        }

        // Tab label
        let label = if tab_bar.show_close {
            format!(" {tab_label} ✕ ")
        } else {
            format!(" {tab_label} ")
        };

        if is_active {
            spans.push(Span::styled(
                label,
                Style::default()
                    .fg(Color::Black)
                    .bg(style.accent_color)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(label, Style::default().fg(style.info_color)));
        }
    }

    vec![Line::from(spans)]
}

/// Render a tab bar with an underline indicator beneath the active tab.
///
/// This variant draws a row of tab labels followed by a second line with
/// an underline positioned beneath the active tab.
pub fn render_tab_bar_underlined(tab_bar: &TabBar, style: &StyleConfig) -> Vec<Line<'static>> {
    if tab_bar.tabs.is_empty() {
        return vec![Line::from(vec![Span::styled(
            " (no tabs) ",
            Style::default()
                .fg(style.info_color)
                .add_modifier(Modifier::DIM),
        )])];
    }

    let mut label_spans = Vec::new();
    let mut widths = Vec::new();

    for (i, tab_label) in tab_bar.tabs.iter().enumerate() {
        let is_active = i == tab_bar.active;

        // Separator
        if i > 0 {
            label_spans.push(Span::styled("  ".to_owned(), Style::default()));
        }

        let label = format!(" {tab_label} ");
        let width = label.len();
        widths.push(width);

        if is_active {
            label_spans.push(Span::styled(
                label,
                Style::default()
                    .fg(style.accent_color)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            label_spans.push(Span::styled(label, Style::default().fg(style.info_color)));
        }
    }

    // Build underline line
    let mut underline_spans = Vec::new();
    for (i, width) in widths.iter().enumerate() {
        if i > 0 {
            underline_spans.push(Span::styled("  ", Style::default()));
        }

        let is_active = i == tab_bar.active;
        if is_active {
            underline_spans.push(Span::styled(
                "─".repeat(*width),
                Style::default().fg(style.accent_color),
            ));
        } else {
            underline_spans.push(Span::styled(" ".repeat(*width), Style::default()));
        }
    }

    vec![Line::from(label_spans), Line::from(underline_spans)]
}

/// Render a compact tab bar suitable for narrow areas.
///
/// Only shows the active tab and a count indicator.
pub fn render_tab_bar_compact(tab_bar: &TabBar, style: &StyleConfig) -> Vec<Line<'static>> {
    if tab_bar.tabs.is_empty() {
        return vec![Line::from("")];
    }

    let active_label = tab_bar
        .tabs
        .get(tab_bar.active)
        .map(|s| s.as_str())
        .unwrap_or("?");

    let total = tab_bar.tabs.len();
    let indicator = if total > 1 {
        format!(" ({}/{})", tab_bar.active + 1, total)
    } else {
        String::new()
    };

    vec![Line::from(vec![
        Span::styled(
            format!(" {active_label} "),
            Style::default()
                .fg(Color::Black)
                .bg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(indicator, Style::default().fg(style.info_color)),
    ])]
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

    // --- TabBar struct tests ---

    #[test]
    fn tab_bar_new() {
        let bar = TabBar::new(vec!["Tab1".to_owned(), "Tab2".to_owned()]);
        assert_eq!(bar.tabs.len(), 2);
        assert_eq!(bar.active, 0);
        assert!(!bar.show_close);
    }

    #[test]
    fn tab_bar_builder() {
        let bar = TabBar::new(vec!["A".to_owned()])
            .with_active(0)
            .with_close(true);
        assert!(bar.show_close);
    }

    #[test]
    fn tab_bar_len() {
        let bar = TabBar::new(vec!["A".to_owned(), "B".to_owned(), "C".to_owned()]);
        assert_eq!(bar.len(), 3);
    }

    #[test]
    fn tab_bar_is_empty() {
        assert!(TabBar::new(vec![]).is_empty());
        assert!(!TabBar::new(vec!["X".to_owned()]).is_empty());
    }

    #[test]
    fn tab_bar_next() {
        let mut bar = TabBar::new(vec!["A".to_owned(), "B".to_owned(), "C".to_owned()]);
        assert_eq!(bar.active, 0);
        bar.next();
        assert_eq!(bar.active, 1);
        bar.next();
        assert_eq!(bar.active, 2);
        bar.next(); // wraps around
        assert_eq!(bar.active, 0);
    }

    #[test]
    fn tab_bar_prev() {
        let mut bar = TabBar::new(vec!["A".to_owned(), "B".to_owned(), "C".to_owned()]);
        bar.prev(); // wraps around to last
        assert_eq!(bar.active, 2);
        bar.prev();
        assert_eq!(bar.active, 1);
    }

    #[test]
    fn tab_bar_next_empty() {
        let mut bar = TabBar::new(vec![]);
        bar.next(); // should not panic
        assert_eq!(bar.active, 0);
    }

    // --- render_tab_bar tests ---

    #[test]
    fn render_empty_tab_bar() {
        let bar = TabBar::new(vec![]);
        let lines = render_tab_bar(&bar, &test_style());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].to_string().contains("no tabs"));
    }

    #[test]
    fn render_single_tab() {
        let bar = TabBar::new(vec!["Only".to_owned()]);
        let lines = render_tab_bar(&bar, &test_style());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].to_string().contains("Only"));
    }

    #[test]
    fn render_multiple_tabs() {
        let bar = TabBar::new(vec![
            "Chat".to_owned(),
            "Sessions".to_owned(),
            "Help".to_owned(),
        ]);
        let lines = render_tab_bar(&bar, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("Chat"));
        assert!(text.contains("Sessions"));
        assert!(text.contains("Help"));
    }

    #[test]
    fn render_tab_bar_with_close() {
        let bar = TabBar::new(vec!["Tab1".to_owned()]).with_close(true);
        let lines = render_tab_bar(&bar, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains('✕'));
    }

    #[test]
    fn render_tab_bar_has_separator() {
        let bar = TabBar::new(vec!["A".to_owned(), "B".to_owned()]);
        let lines = render_tab_bar(&bar, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains('│'));
    }

    // --- render_tab_bar_underlined tests ---

    #[test]
    fn render_underlined_two_lines() {
        let bar = TabBar::new(vec!["Tab1".to_owned(), "Tab2".to_owned()]);
        let lines = render_tab_bar_underlined(&bar, &test_style());
        assert_eq!(lines.len(), 2); // labels + underline
    }

    #[test]
    fn render_underlined_active_has_dash() {
        let bar = TabBar::new(vec!["A".to_owned(), "B".to_owned()]).with_active(1);
        let lines = render_tab_bar_underlined(&bar, &test_style());
        let underline = lines[1].to_string();
        assert!(underline.contains('─'));
    }

    #[test]
    fn render_underlined_empty() {
        let bar = TabBar::new(vec![]);
        let lines = render_tab_bar_underlined(&bar, &test_style());
        assert!(lines[0].to_string().contains("no tabs"));
    }

    // --- render_tab_bar_compact tests ---

    #[test]
    fn render_compact_single_tab() {
        let bar = TabBar::new(vec!["Only".to_owned()]);
        let lines = render_tab_bar_compact(&bar, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("Only"));
        // Single tab should not show count
        assert!(!text.contains("1/1"));
    }

    #[test]
    fn render_compact_multiple_tabs() {
        let bar = TabBar::new(vec!["A".to_owned(), "B".to_owned(), "C".to_owned()]).with_active(1);
        let lines = render_tab_bar_compact(&bar, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("B"));
        assert!(text.contains("2/3"));
    }

    #[test]
    fn render_compact_empty() {
        let bar = TabBar::new(vec![]);
        let lines = render_tab_bar_compact(&bar, &test_style());
        assert_eq!(lines.len(), 1);
    }
}
