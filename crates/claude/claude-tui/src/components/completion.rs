//! Tab completion popup component.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

/// Completion item for the popup.
#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
}

/// Kind of completion item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Command,
    Tool,
    File,
    History,
}

impl CompletionKind {
    /// Icon prefix for display.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Command => "⚡",
            Self::Tool => "⚙",
            Self::File => "📄",
            Self::History => "🕐",
        }
    }

    /// Color for this kind.
    pub fn color(self, style: &StyleConfig) -> ratatui::style::Color {
        match self {
            Self::Command => style.accent_color,
            Self::Tool => style.tool_color,
            Self::File => style.mode_insert,
            Self::History => style.info_color,
        }
    }
}

/// Render completion items as a list of Lines for a popup.
pub fn render_completions(
    items: &[CompletionItem],
    selected: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    if items.is_empty() {
        return vec![Line::from(Span::styled(
            "  No completions",
            Style::default().fg(style.info_color),
        ))];
    }

    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let is_selected = i == selected;
            let icon = item.kind.icon();
            let color = item.kind.color(style);

            let selector = if is_selected { "▸" } else { " " };
            let label_style = if is_selected {
                Style::default().fg(color).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(color)
            };

            Line::from(vec![
                Span::styled(format!(" {selector} "), Style::default()),
                Span::styled(format!("{icon} "), Style::default().fg(color)),
                Span::styled(item.label.clone(), label_style),
            ])
        })
        .collect()
}

/// Build completion items from slash command completions.
pub fn slash_completions(partial: &str) -> Vec<CompletionItem> {
    crate::tab_complete::complete_slash_command(partial)
        .into_iter()
        .map(|cmd| CompletionItem {
            label: cmd,
            kind: CompletionKind::Command,
        })
        .collect()
}

/// Build completion items from tool name completions.
pub fn tool_completions(prefix: &str) -> Vec<CompletionItem> {
    crate::tab_complete::get_tool_completions(prefix)
        .into_iter()
        .map(|name| CompletionItem {
            label: name,
            kind: CompletionKind::Tool,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::StyleConfig;

    #[test]
    fn empty_completions_shows_message() {
        let style = StyleConfig::dark();
        let lines = render_completions(&[], 0, &style);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn completions_with_items() {
        let style = StyleConfig::dark();
        let items = vec![
            CompletionItem {
                label: "/help".to_owned(),
                kind: CompletionKind::Command,
            },
            CompletionItem {
                label: "/status".to_owned(),
                kind: CompletionKind::Command,
            },
        ];
        let lines = render_completions(&items, 0, &style);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn completion_kind_icons() {
        assert_eq!(CompletionKind::Command.icon(), "⚡");
        assert_eq!(CompletionKind::Tool.icon(), "⚙");
        assert_eq!(CompletionKind::File.icon(), "📄");
        assert_eq!(CompletionKind::History.icon(), "🕐");
    }

    #[test]
    fn slash_completions_returns_items() {
        let items = slash_completions("/h");
        assert!(!items.is_empty());
        assert!(items.iter().any(|i| i.label == "/help"));
    }

    #[test]
    fn tool_completions_returns_items() {
        let items = tool_completions("bash");
        assert!(!items.is_empty());
    }
}
