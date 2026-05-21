//! Compact operation summary component.
//!
//! Renders a summary of a context compaction operation, showing token savings
//! and message preservation statistics.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Summary data produced after a compaction operation.
#[derive(Debug, Clone)]
pub struct CompactSummaryData {
    pub before_tokens: u64,
    pub after_tokens: u64,
    pub messages_preserved: usize,
    pub messages_removed: usize,
}

impl CompactSummaryData {
    /// Compute the token savings percentage.
    pub fn savings_percentage(&self) -> f64 {
        if self.before_tokens == 0 {
            return 0.0;
        }
        let saved = self.before_tokens.saturating_sub(self.after_tokens);
        (saved as f64 / self.before_tokens as f64) * 100.0
    }

    /// Total messages before compaction.
    pub fn total_messages(&self) -> usize {
        self.messages_preserved + self.messages_removed
    }
}

// ---------------------------------------------------------------------------
// CompactSummaryRenderer
// ---------------------------------------------------------------------------

/// Renders a compact summary as a vector of lines.
pub struct CompactSummaryRenderer;

impl CompactSummaryRenderer {
    /// Render the compaction summary.
    pub fn render(data: &CompactSummaryData) -> Vec<Line<'static>> {
        let savings = data.savings_percentage();

        vec![
            // Header
            Line::from(vec![Span::styled(
                "Compact Summary".to_string(),
                Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )]),
            // Token savings
            Line::from(vec![
                Span::raw("  Tokens: "),
                Span::styled(
                    format!("{}", data.before_tokens),
                    Style::default().fg(Color::Red),
                ),
                Span::raw(" → "),
                Span::styled(
                    format!("{}", data.after_tokens),
                    Style::default().fg(Color::Green),
                ),
                Span::styled(
                    format!("  (saved {:.1}%)", savings),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            // Messages
            Line::from(vec![
                Span::raw("  Messages: "),
                Span::styled(
                    format!("{} preserved", data.messages_preserved),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(", "),
                Span::styled(
                    format!("{} removed", data.messages_removed),
                    Style::default().fg(Color::Red),
                ),
            ]),
        ]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_savings_percentage_basic() {
        let data = CompactSummaryData {
            before_tokens: 1000,
            after_tokens: 400,
            messages_preserved: 5,
            messages_removed: 10,
        };
        let pct = data.savings_percentage();
        assert!((pct - 60.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_savings_percentage_zero_before() {
        let data = CompactSummaryData {
            before_tokens: 0,
            after_tokens: 0,
            messages_preserved: 0,
            messages_removed: 0,
        };
        assert_eq!(data.savings_percentage(), 0.0);
    }

    #[test]
    fn test_total_messages() {
        let data = CompactSummaryData {
            before_tokens: 100,
            after_tokens: 50,
            messages_preserved: 3,
            messages_removed: 7,
        };
        assert_eq!(data.total_messages(), 10);
    }

    #[test]
    fn test_render_line_count() {
        let data = CompactSummaryData {
            before_tokens: 1000,
            after_tokens: 500,
            messages_preserved: 4,
            messages_removed: 6,
        };
        let lines = CompactSummaryRenderer::render(&data);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_render_contains_savings() {
        let data = CompactSummaryData {
            before_tokens: 1000,
            after_tokens: 500,
            messages_preserved: 4,
            messages_removed: 6,
        };
        let lines = CompactSummaryRenderer::render(&data);
        let token_line: String = lines[1].spans.iter().map(|s| s.content.clone()).collect();
        assert!(token_line.contains("50.0%"));
    }

    #[test]
    fn test_render_contains_preserved_count() {
        let data = CompactSummaryData {
            before_tokens: 2000,
            after_tokens: 800,
            messages_preserved: 12,
            messages_removed: 8,
        };
        let lines = CompactSummaryRenderer::render(&data);
        let msg_line: String = lines[2].spans.iter().map(|s| s.content.clone()).collect();
        assert!(msg_line.contains("12 preserved"));
        assert!(msg_line.contains("8 removed"));
    }

    #[test]
    fn test_render_header() {
        let data = CompactSummaryData {
            before_tokens: 100,
            after_tokens: 50,
            messages_preserved: 1,
            messages_removed: 1,
        };
        let lines = CompactSummaryRenderer::render(&data);
        let header: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert_eq!(header, "Compact Summary");
    }
}
