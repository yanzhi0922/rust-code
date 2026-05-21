//! Provider selection component.
//!
//! Reference: Claude Code's `ProviderPicker.tsx` — renders a list of
//! available LLM providers with connection status indicators.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// Provider data
// ---------------------------------------------------------------------------

/// Connection status of a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderStatus {
    /// Provider is connected and ready.
    Connected,
    /// Provider is disconnected or unreachable.
    Disconnected,
    /// Provider connection is in progress.
    Connecting,
}

impl ProviderStatus {
    /// Status icon.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Connected => "●",
            Self::Disconnected => "○",
            Self::Connecting => "◌",
        }
    }

    /// Status label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
        }
    }
}

/// A single provider entry.
#[derive(Debug, Clone)]
pub struct ProviderEntry {
    /// Provider display name (e.g., "Anthropic", "OpenAI").
    pub name: String,
    /// Provider identifier (e.g., "anthropic", "openai").
    pub id: String,
    /// Current connection status.
    pub status: ProviderStatus,
}

impl ProviderEntry {
    /// Create a new provider entry.
    pub fn new(name: &str, id: &str, status: ProviderStatus) -> Self {
        Self {
            name: name.to_owned(),
            id: id.to_owned(),
            status,
        }
    }
}

// ---------------------------------------------------------------------------
// ProviderPicker
// ---------------------------------------------------------------------------

/// A selectable provider picker component.
#[derive(Debug, Clone)]
pub struct ProviderPicker {
    /// All available providers.
    pub providers: Vec<ProviderEntry>,
    /// Index of the currently selected provider.
    pub selected_index: usize,
    /// Index of the currently highlighted provider.
    pub highlighted_index: usize,
}

impl ProviderPicker {
    /// Create a new provider picker with default providers.
    pub fn new() -> Self {
        Self {
            providers: Self::default_providers(),
            selected_index: 0,
            highlighted_index: 0,
        }
    }

    /// Create a picker with custom providers.
    pub fn with_providers(providers: Vec<ProviderEntry>) -> Self {
        Self {
            providers,
            selected_index: 0,
            highlighted_index: 0,
        }
    }

    /// Default provider list.
    pub fn default_providers() -> Vec<ProviderEntry> {
        vec![
            ProviderEntry::new("Anthropic", "anthropic", ProviderStatus::Connected),
            ProviderEntry::new("OpenAI", "openai", ProviderStatus::Disconnected),
            ProviderEntry::new("Google", "google", ProviderStatus::Disconnected),
        ]
    }

    /// Get the currently selected provider.
    pub fn selected(&self) -> Option<&ProviderEntry> {
        self.providers.get(self.selected_index)
    }

    /// Move highlight up.
    pub fn move_up(&mut self) {
        if self.highlighted_index > 0 {
            self.highlighted_index -= 1;
        }
    }

    /// Move highlight down.
    pub fn move_down(&mut self) {
        if self.highlighted_index + 1 < self.providers.len() {
            self.highlighted_index += 1;
        }
    }

    /// Confirm selection.
    pub fn confirm(&mut self) {
        self.selected_index = self.highlighted_index;
    }

    /// Render the provider picker into ratatui lines.
    pub fn render(&self, style: &StyleConfig) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Title.
        lines.push(Line::from(vec![Span::styled(
            " Select Provider ".to_owned(),
            Style::default()
                .fg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(""));

        for (i, provider) in self.providers.iter().enumerate() {
            let is_selected = i == self.selected_index;
            let is_highlighted = i == self.highlighted_index;

            let cursor = if is_highlighted { "▸" } else { " " };
            let check = if is_selected { "●" } else { "○" };

            let status_color = match provider.status {
                ProviderStatus::Connected => style.tool_color,
                ProviderStatus::Disconnected => style.error_color,
                ProviderStatus::Connecting => style.info_color,
            };

            let name_fg = if is_highlighted {
                style.accent_color
            } else {
                style.status_fg
            };

            lines.push(Line::from(vec![
                Span::styled(format!(" {cursor} "), Style::default().fg(name_fg)),
                Span::styled(
                    format!("{check} "),
                    Style::default().fg(if is_selected {
                        style.accent_color
                    } else {
                        name_fg
                    }),
                ),
                Span::styled(format!("{} ", provider.name), Style::default().fg(name_fg)),
                Span::styled(
                    provider.status.icon().to_owned(),
                    Style::default().fg(status_color),
                ),
                Span::styled(
                    format!(" {}", provider.status.label()),
                    Style::default().fg(status_color),
                ),
            ]));
        }

        lines
    }
}

impl Default for ProviderPicker {
    fn default() -> Self {
        Self::new()
    }
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

    #[test]
    fn default_providers_populated() {
        let picker = ProviderPicker::new();
        assert_eq!(picker.providers.len(), 3);
    }

    #[test]
    fn selected_default_is_first() {
        let picker = ProviderPicker::new();
        let sel = picker.selected().expect("default provider should exist");
        assert_eq!(sel.id, "anthropic");
        assert_eq!(sel.status, ProviderStatus::Connected);
    }

    #[test]
    fn navigation() {
        let mut picker = ProviderPicker::new();
        picker.move_down();
        assert_eq!(picker.highlighted_index, 1);
        picker.move_up();
        assert_eq!(picker.highlighted_index, 0);
    }

    #[test]
    fn confirm_changes_selected() {
        let mut picker = ProviderPicker::new();
        picker.move_down();
        picker.confirm();
        assert_eq!(picker.selected_index, 1);
        assert_eq!(
            picker
                .selected()
                .expect("confirmed provider should exist")
                .id,
            "openai"
        );
    }

    #[test]
    fn render_produces_lines() {
        let picker = ProviderPicker::new();
        let lines = picker.render(&test_style());
        // title + blank + 3 providers
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn provider_status_icons() {
        assert_eq!(ProviderStatus::Connected.icon(), "●");
        assert_eq!(ProviderStatus::Disconnected.icon(), "○");
        assert_eq!(ProviderStatus::Connecting.icon(), "◌");
    }

    #[test]
    fn custom_providers() {
        let providers = vec![ProviderEntry::new(
            "Test",
            "test",
            ProviderStatus::Connecting,
        )];
        let picker = ProviderPicker::with_providers(providers);
        assert_eq!(picker.providers.len(), 1);
        assert_eq!(picker.providers[0].status, ProviderStatus::Connecting);
    }
}
