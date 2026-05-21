//! Memory panel component for the TUI.
//!
//! Provides rendering for memory file management, including
//! memory file selection and update notifications.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`MemoryFileEntry`] | A memory file entry |
//! | [`MemoryPanel`] | Memory panel state |
//! | [`render_memory_panel`] | Render the memory panel |
//! | [`render_memory_notification`] | Render a memory update notification |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// MemoryFileEntry
// ---------------------------------------------------------------------------

/// A memory file entry.
#[derive(Debug, Clone)]
pub struct MemoryFileEntry {
    /// File name.
    pub name: String,
    /// File path.
    pub path: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Last modified timestamp.
    pub modified: String,
    /// Whether the file is active/loaded.
    pub is_active: bool,
}

// ---------------------------------------------------------------------------
// MemoryPanel
// ---------------------------------------------------------------------------

/// Memory panel state.
#[derive(Debug, Clone)]
pub struct MemoryPanel {
    /// Memory file entries.
    pub files: Vec<MemoryFileEntry>,
    /// Currently selected index.
    pub selected: usize,
}

impl MemoryPanel {
    /// Create a new memory panel.
    pub fn new(files: Vec<MemoryFileEntry>) -> Self {
        Self { files, selected: 0 }
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        if self.selected + 1 < self.files.len() {
            self.selected += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering helpers
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

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// ---------------------------------------------------------------------------
// Render functions
// ---------------------------------------------------------------------------

/// Render the memory panel.
pub fn render_memory_panel(panel: &MemoryPanel, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(header_span(" Memory Files", style)));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    if panel.files.is_empty() {
        lines.push(Line::from(dim_span("   No memory files found.")));
        lines.push(Line::from(""));
        lines.push(Line::from(dim_span(
            "   Memory files (CLAUDE.md, .claude/) will appear here.",
        )));
    } else {
        for (i, file) in panel.files.iter().enumerate() {
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

            // Active indicator
            if file.is_active {
                spans.push(Span::styled(
                    "●".to_owned(),
                    Style::default().fg(Color::Green),
                ));
            } else {
                spans.push(Span::styled(
                    "○".to_owned(),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            spans.push(Span::styled(" ".to_owned(), Style::default()));

            // File name
            spans.push(if is_selected {
                Span::styled(
                    file.name.clone(),
                    Style::default()
                        .fg(style.accent_color)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(file.name.clone(), Style::default().fg(style.status_fg))
            });

            // Size
            spans.push(dim_span(&format!(" {}", format_size(file.size_bytes))));

            // Modified
            spans.push(dim_span(&format!(" ({})", file.modified)));

            lines.push(Line::from(spans));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(dim_span(
        "   ↑↓ navigate │ Enter view │ e edit │ q close",
    )));

    lines
}

/// Render a memory update notification.
pub fn render_memory_notification(
    file_name: &str,
    action: &str,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    vec![Line::from(vec![
        Span::styled(" 💾 ".to_owned(), Style::default().fg(Color::Cyan)),
        Span::styled(format!("{action} "), Style::default().fg(style.status_fg)),
        Span::styled(
            file_name.to_owned(),
            Style::default()
                .fg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        ),
    ])]
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

    fn sample_file(name: &str, active: bool) -> MemoryFileEntry {
        MemoryFileEntry {
            name: name.to_owned(),
            path: format!("/project/{name}"),
            size_bytes: 1024,
            modified: "2025-01-01".to_owned(),
            is_active: active,
        }
    }

    fn sample_panel() -> MemoryPanel {
        MemoryPanel::new(vec![
            sample_file("CLAUDE.md", true),
            sample_file(".claude/context.md", false),
        ])
    }

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(500), "500B");
    }

    #[test]
    fn format_size_kb() {
        assert_eq!(format_size(2048), "2.0KB");
    }

    #[test]
    fn format_size_mb() {
        assert_eq!(format_size(2 * 1024 * 1024), "2.0MB");
    }

    #[test]
    fn panel_move_down() {
        let mut panel = sample_panel();
        panel.move_down();
        assert_eq!(panel.selected, 1);
    }

    #[test]
    fn panel_move_up_clamps() {
        let mut panel = sample_panel();
        panel.move_up();
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn render_panel_shows_files() {
        let panel = sample_panel();
        let lines = render_memory_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("CLAUDE.md"));
        assert!(combined.contains(".claude/context.md"));
    }

    #[test]
    fn render_panel_shows_size() {
        let panel = sample_panel();
        let lines = render_memory_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("1.0KB"));
    }

    #[test]
    fn render_panel_empty() {
        let panel = MemoryPanel::new(vec![]);
        let lines = render_memory_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("No memory files"));
    }

    #[test]
    fn render_notification_shows_action() {
        let lines = render_memory_notification("CLAUDE.md", "Updated", &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Updated"));
        assert!(combined.contains("CLAUDE.md"));
    }
}
