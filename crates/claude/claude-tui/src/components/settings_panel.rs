//! Settings panel component for the TUI.
//!
//! Provides a full-screen panel for viewing and editing application settings,
//! including provider configuration, model selection, and usage statistics.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`SettingsSection`] | Section within the settings panel |
//! | [`SettingsEntry`] | A single key-value setting entry |
//! | [`SettingsPanel`] | Top-level panel state |
//! | [`render_settings_panel`] | Render the settings panel |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// SettingsEntry
// ---------------------------------------------------------------------------

/// A single setting entry with key, value, and optional description.
#[derive(Debug, Clone)]
pub struct SettingsEntry {
    /// Setting key name.
    pub key: String,
    /// Current value (displayed as-is).
    pub value: String,
    /// Source of the setting (e.g., "default", "user", "project", "env").
    pub source: String,
    /// Optional description of what this setting controls.
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// SettingsSection
// ---------------------------------------------------------------------------

/// A section grouping related settings.
#[derive(Debug, Clone)]
pub struct SettingsSection {
    /// Section title.
    pub title: String,
    /// Settings entries in this section.
    pub entries: Vec<SettingsEntry>,
}

// ---------------------------------------------------------------------------
// SettingsPanel
// ---------------------------------------------------------------------------

/// Settings panel view state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsView {
    /// Show the list of sections.
    SectionList,
    /// Show entries for a specific section.
    Section { index: usize },
}

/// Top-level settings panel state.
#[derive(Debug, Clone)]
pub struct SettingsPanel {
    /// Sections to display.
    pub sections: Vec<SettingsSection>,
    /// Current view.
    pub view: SettingsView,
    /// Currently selected item index.
    pub selected: usize,
    /// Scroll offset.
    pub scroll_offset: usize,
}

impl SettingsPanel {
    /// Create a new settings panel.
    pub fn new(sections: Vec<SettingsSection>) -> Self {
        Self {
            sections,
            view: SettingsView::SectionList,
            selected: 0,
            scroll_offset: 0,
        }
    }

    /// Enter the selected section.
    pub fn enter_section(&mut self) {
        if self.selected < self.sections.len() {
            self.view = SettingsView::Section {
                index: self.selected,
            };
            self.selected = 0;
            self.scroll_offset = 0;
        }
    }

    /// Go back to the previous view.
    pub fn go_back(&mut self) {
        self.view = SettingsView::SectionList;
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        let max = match &self.view {
            SettingsView::SectionList => self.sections.len(),
            SettingsView::Section { index } => {
                self.sections.get(*index).map_or(0, |s| s.entries.len())
            }
        };
        if self.selected + 1 < max {
            self.selected += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn dim_span(text: &str) -> Span<'static> {
    Span::styled(
        text.to_owned(),
        Style::default().add_modifier(Modifier::DIM),
    )
}

fn header_span(text: &str, style: &StyleConfig) -> Span<'static> {
    Span::styled(
        text.to_owned(),
        Style::default()
            .fg(style.accent_color)
            .add_modifier(Modifier::BOLD),
    )
}

/// Render the settings panel.
pub fn render_settings_panel(panel: &SettingsPanel, style: &StyleConfig) -> Vec<Line<'static>> {
    match &panel.view {
        SettingsView::SectionList => render_section_list(panel, style),
        SettingsView::Section { index } => render_section_entries(panel, *index, style),
    }
}

/// Render the section list view.
pub fn render_section_list(panel: &SettingsPanel, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(header_span(" Settings", style)));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    if panel.sections.is_empty() {
        lines.push(Line::from(dim_span("   No settings sections available.")));
    } else {
        for (i, section) in panel.sections.iter().enumerate() {
            let is_selected = i == panel.selected;

            let mut spans = Vec::new();

            if is_selected {
                spans.push(Span::styled(
                    " ❯ ".to_owned(),
                    Style::default()
                        .fg(style.accent_color)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled("   ", Style::default()));
            }

            spans.push(if is_selected {
                Span::styled(
                    section.title.clone(),
                    Style::default()
                        .fg(style.accent_color)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(section.title.clone(), Style::default().fg(style.status_fg))
            });

            spans.push(dim_span(&format!(" ({} entries)", section.entries.len())));

            lines.push(Line::from(spans));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(dim_span(
        "   ↑↓ navigate │ Enter view section │ Esc back │ q close",
    )));

    lines
}

/// Render entries for a specific section.
pub fn render_section_entries(
    panel: &SettingsPanel,
    section_index: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let section = match panel.sections.get(section_index) {
        Some(s) => s,
        None => {
            lines.push(Line::from("Section not found."));
            return lines;
        }
    };

    lines.push(Line::from(vec![
        Span::styled(" ◀ ".to_owned(), Style::default().fg(style.accent_color)),
        header_span(&section.title, style),
    ]));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    if section.entries.is_empty() {
        lines.push(Line::from(dim_span("   No entries in this section.")));
    } else {
        for (i, entry) in section.entries.iter().enumerate() {
            let is_selected = i == panel.selected;

            let mut spans = Vec::new();

            if is_selected {
                spans.push(Span::styled(
                    " ❯ ".to_owned(),
                    Style::default()
                        .fg(style.accent_color)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled("   ", Style::default()));
            }

            // Key
            spans.push(if is_selected {
                Span::styled(
                    entry.key.clone(),
                    Style::default()
                        .fg(style.accent_color)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(entry.key.clone(), Style::default().fg(style.status_fg))
            });

            // Value
            spans.push(Span::styled(": ".to_owned(), Style::default()));
            spans.push(Span::styled(
                entry.value.clone(),
                Style::default().fg(Color::Cyan),
            ));

            // Source
            spans.push(dim_span(&format!(" [{}]", entry.source)));

            lines.push(Line::from(spans));

            // Description
            if let Some(desc) = &entry.description {
                lines.push(Line::from(dim_span(&format!("     {desc}"))));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(dim_span("   ↑↓ navigate │ Esc back")));

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

    fn sample_section(title: &str, count: usize) -> SettingsSection {
        SettingsSection {
            title: title.to_owned(),
            entries: (0..count)
                .map(|i| SettingsEntry {
                    key: format!("key_{i}"),
                    value: format!("value_{i}"),
                    source: if i % 2 == 0 {
                        "default".to_owned()
                    } else {
                        "user".to_owned()
                    },
                    description: if i == 0 {
                        Some(format!("Description for key {i}"))
                    } else {
                        None
                    },
                })
                .collect(),
        }
    }

    fn sample_panel() -> SettingsPanel {
        SettingsPanel::new(vec![
            sample_section("Provider", 3),
            sample_section("Model", 2),
            sample_section("Permissions", 1),
        ])
    }

    #[test]
    fn new_panel_starts_at_section_list() {
        let panel = sample_panel();
        assert_eq!(panel.view, SettingsView::SectionList);
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn move_down_increments() {
        let mut panel = sample_panel();
        panel.move_down();
        assert_eq!(panel.selected, 1);
    }

    #[test]
    fn move_down_clamps() {
        let mut panel = sample_panel();
        panel.selected = 2;
        panel.move_down();
        assert_eq!(panel.selected, 2);
    }

    #[test]
    fn move_up_decrements() {
        let mut panel = sample_panel();
        panel.selected = 2;
        panel.move_up();
        assert_eq!(panel.selected, 1);
    }

    #[test]
    fn move_up_clamps_at_zero() {
        let mut panel = sample_panel();
        panel.move_up();
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn enter_section_transitions() {
        let mut panel = sample_panel();
        panel.selected = 1;
        panel.enter_section();
        assert_eq!(panel.view, SettingsView::Section { index: 1 });
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn go_back_returns_to_list() {
        let mut panel = sample_panel();
        panel.enter_section();
        panel.go_back();
        assert_eq!(panel.view, SettingsView::SectionList);
    }

    #[test]
    fn render_section_list_contains_titles() {
        let panel = sample_panel();
        let lines = render_section_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Provider"));
        assert!(combined.contains("Model"));
        assert!(combined.contains("Permissions"));
    }

    #[test]
    fn render_section_list_shows_entry_counts() {
        let panel = sample_panel();
        let lines = render_section_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("3 entries"));
        assert!(combined.contains("2 entries"));
    }

    #[test]
    fn render_section_list_empty() {
        let panel = SettingsPanel::new(vec![]);
        let lines = render_section_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("No settings sections"));
    }

    #[test]
    fn render_section_entries_shows_keys() {
        let panel = sample_panel();
        let lines = render_section_entries(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("key_0"));
        assert!(combined.contains("key_1"));
        assert!(combined.contains("key_2"));
    }

    #[test]
    fn render_section_entries_shows_values() {
        let panel = sample_panel();
        let lines = render_section_entries(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("value_0"));
    }

    #[test]
    fn render_section_entries_shows_sources() {
        let panel = sample_panel();
        let lines = render_section_entries(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("[default]"));
        assert!(combined.contains("[user]"));
    }

    #[test]
    fn render_section_entries_shows_description() {
        let panel = sample_panel();
        let lines = render_section_entries(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Description for key 0"));
    }

    #[test]
    fn render_section_entries_invalid() {
        let panel = sample_panel();
        let lines = render_section_entries(&panel, 99, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Section not found"));
    }

    #[test]
    fn render_panel_dispatches_to_list() {
        let panel = sample_panel();
        let lines = render_settings_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Settings"));
    }

    #[test]
    fn render_panel_dispatches_to_section() {
        let mut panel = sample_panel();
        panel.view = SettingsView::Section { index: 0 };
        let lines = render_settings_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Provider"));
        assert!(combined.contains("key_0"));
    }

    #[test]
    fn render_section_entries_empty() {
        let panel = SettingsPanel::new(vec![SettingsSection {
            title: "Empty".to_owned(),
            entries: vec![],
        }]);
        let lines = render_section_entries(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("No entries"));
    }
}
