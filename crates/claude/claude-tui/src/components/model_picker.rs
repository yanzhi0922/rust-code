//! Model selection component.
//!
//! Reference: Claude Code's `ModelPicker.tsx` — provides a selectable list
//! of AI models grouped by family (Sonnet / Opus / Haiku).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// Model data
// ---------------------------------------------------------------------------

/// A single model entry.
#[derive(Debug, Clone)]
pub struct ModelEntry {
    /// Display name (e.g., "claude-sonnet-4-20250514").
    pub name: String,
    /// Short label for the picker (e.g., "Sonnet 4").
    pub label: String,
    /// Family grouping key (e.g., "sonnet", "opus", "haiku").
    pub family: String,
    /// Whether this model is currently available.
    pub available: bool,
}

impl ModelEntry {
    /// Create a new model entry.
    pub fn new(name: &str, label: &str, family: &str) -> Self {
        Self {
            name: name.to_owned(),
            label: label.to_owned(),
            family: family.to_owned(),
            available: true,
        }
    }
}

// ---------------------------------------------------------------------------
// ModelPicker
// ---------------------------------------------------------------------------

/// A selectable model picker component.
#[derive(Debug, Clone)]
pub struct ModelPicker {
    /// All available models.
    pub models: Vec<ModelEntry>,
    /// Index of the currently selected model.
    pub selected_index: usize,
    /// Index of the currently highlighted model (for navigation).
    pub highlighted_index: usize,
}

impl ModelPicker {
    /// Create a new model picker with default models.
    pub fn new() -> Self {
        Self {
            models: Self::default_models(),
            selected_index: 0,
            highlighted_index: 0,
        }
    }

    /// Create a picker with custom models.
    pub fn with_models(models: Vec<ModelEntry>) -> Self {
        Self {
            models,
            selected_index: 0,
            highlighted_index: 0,
        }
    }

    /// Default model list.
    pub fn default_models() -> Vec<ModelEntry> {
        vec![
            ModelEntry::new("claude-sonnet-4-20250514", "Sonnet 4", "sonnet"),
            ModelEntry::new("claude-sonnet-4-20250514", "Sonnet 3.5", "sonnet"),
            ModelEntry::new("claude-opus-4-20250514", "Opus 4", "opus"),
            ModelEntry::new("claude-haiku-3-20250514", "Haiku 3.5", "haiku"),
        ]
    }

    /// Get the currently selected model entry.
    pub fn selected(&self) -> Option<&ModelEntry> {
        self.models.get(self.selected_index)
    }

    /// Get the currently highlighted model entry.
    pub fn highlighted(&self) -> Option<&ModelEntry> {
        self.models.get(self.highlighted_index)
    }

    /// Move the highlight up.
    pub fn move_up(&mut self) {
        if self.highlighted_index > 0 {
            self.highlighted_index -= 1;
        }
    }

    /// Move the highlight down.
    pub fn move_down(&mut self) {
        if self.highlighted_index + 1 < self.models.len() {
            self.highlighted_index += 1;
        }
    }

    /// Confirm selection — set selected to the highlighted index.
    pub fn confirm(&mut self) {
        self.selected_index = self.highlighted_index;
    }

    /// Render the model picker into ratatui lines.
    pub fn render(&self, style: &StyleConfig) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Title.
        lines.push(Line::from(vec![Span::styled(
            " Select Model ".to_owned(),
            Style::default()
                .fg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(""));

        // Group models by family.
        let mut current_family = String::new();
        for (i, model) in self.models.iter().enumerate() {
            // Family header.
            if model.family != current_family {
                current_family = model.family.clone();
                let family_label = match current_family.as_str() {
                    "sonnet" => "Sonnet",
                    "opus" => "Opus",
                    "haiku" => "Haiku",
                    other => other,
                };
                lines.push(Line::from(vec![Span::styled(
                    format!("  {family_label}"),
                    Style::default()
                        .fg(style.info_color)
                        .add_modifier(Modifier::BOLD),
                )]));
            }

            // Model entry.
            let is_selected = i == self.selected_index;
            let is_highlighted = i == self.highlighted_index;

            let cursor = if is_highlighted { "▸" } else { " " };
            let check = if is_selected { "●" } else { "○" };
            let avail_icon = if model.available {
                ""
            } else {
                " (unavailable)"
            };

            let fg = if !model.available {
                style.info_color
            } else if is_highlighted {
                style.accent_color
            } else {
                style.status_fg
            };

            lines.push(Line::from(vec![
                Span::styled(format!(" {cursor} "), Style::default().fg(fg)),
                Span::styled(
                    format!("{check} "),
                    Style::default().fg(if is_selected { style.accent_color } else { fg }),
                ),
                Span::styled(
                    format!("{}{avail_icon}", model.label),
                    Style::default().fg(fg),
                ),
            ]));
        }

        lines
    }
}

impl Default for ModelPicker {
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
    fn default_models_populated() {
        let picker = ModelPicker::new();
        assert!(!picker.models.is_empty());
        assert!(picker.models.len() >= 4);
    }

    #[test]
    fn selected_default() {
        let picker = ModelPicker::new();
        let sel = picker.selected().expect("default selection should exist");
        assert_eq!(sel.family, "sonnet");
    }

    #[test]
    fn navigation_up_down() {
        let mut picker = ModelPicker::new();
        assert_eq!(picker.highlighted_index, 0);
        picker.move_down();
        assert_eq!(picker.highlighted_index, 1);
        picker.move_up();
        assert_eq!(picker.highlighted_index, 0);
        // Can't go above 0.
        picker.move_up();
        assert_eq!(picker.highlighted_index, 0);
    }

    #[test]
    fn confirm_selection() {
        let mut picker = ModelPicker::new();
        picker.move_down();
        picker.move_down();
        picker.confirm();
        assert_eq!(picker.selected_index, 2);
        let sel = picker.selected().expect("confirmed selection should exist");
        assert_eq!(sel.family, "opus");
    }

    #[test]
    fn render_produces_lines() {
        let picker = ModelPicker::new();
        let lines = picker.render(&test_style());
        assert!(lines.len() >= 4); // title + blank + at least 2 entries
    }

    #[test]
    fn custom_models() {
        let models = vec![
            ModelEntry::new("gpt-4", "GPT-4", "openai"),
            ModelEntry::new("gpt-3.5", "GPT-3.5", "openai"),
        ];
        let picker = ModelPicker::with_models(models);
        assert_eq!(picker.models.len(), 2);
    }

    #[test]
    fn unavailable_model() {
        let mut models = ModelPicker::default_models();
        models[0].available = false;
        let picker = ModelPicker::with_models(models);
        let lines = picker.render(&test_style());
        // Should still render, with "(unavailable)" text
        let rendered: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.clone()))
            .collect();
        assert!(rendered.contains("unavailable"));
    }

    #[test]
    fn cannot_navigate_past_bounds() {
        let mut picker = ModelPicker::new();
        let last = picker.models.len() - 1;
        picker.highlighted_index = last;
        picker.move_down();
        assert_eq!(picker.highlighted_index, last);
    }
}
