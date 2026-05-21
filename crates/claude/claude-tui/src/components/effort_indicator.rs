//! Effort level indicator component.
//!
//! Displays a compact effort-level badge: "⚡ Low", "⚖ Medium", "🔥 High".

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Effort level for a task or session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortLevel {
    Low,
    Medium,
    High,
}

impl EffortLevel {
    /// Symbol used for the effort badge.
    pub fn symbol(&self) -> &'static str {
        match self {
            EffortLevel::Low => "⚡",
            EffortLevel::Medium => "⚖",
            EffortLevel::High => "🔥",
        }
    }

    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            EffortLevel::Low => "Low",
            EffortLevel::Medium => "Medium",
            EffortLevel::High => "High",
        }
    }

    /// Colour associated with the effort level.
    pub fn color(&self) -> Color {
        match self {
            EffortLevel::Low => Color::Green,
            EffortLevel::Medium => Color::Yellow,
            EffortLevel::High => Color::Red,
        }
    }
}

// ---------------------------------------------------------------------------
// EffortIndicator
// ---------------------------------------------------------------------------

/// Renders an effort-level indicator as a single line.
pub struct EffortIndicator;

impl EffortIndicator {
    /// Render the effort indicator as a single styled line.
    pub fn render(level: EffortLevel) -> Line<'static> {
        Line::from(vec![
            Span::styled(
                format!("{} ", level.symbol()),
                Style::default().fg(level.color()),
            ),
            Span::styled(
                level.label().to_string(),
                Style::default()
                    .fg(level.color())
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_effort_symbols() {
        assert_eq!(EffortLevel::Low.symbol(), "⚡");
        assert_eq!(EffortLevel::Medium.symbol(), "⚖");
        assert_eq!(EffortLevel::High.symbol(), "🔥");
    }

    #[test]
    fn test_effort_labels() {
        assert_eq!(EffortLevel::Low.label(), "Low");
        assert_eq!(EffortLevel::Medium.label(), "Medium");
        assert_eq!(EffortLevel::High.label(), "High");
    }

    #[test]
    fn test_effort_colors() {
        assert_eq!(EffortLevel::Low.color(), Color::Green);
        assert_eq!(EffortLevel::Medium.color(), Color::Yellow);
        assert_eq!(EffortLevel::High.color(), Color::Red);
    }

    #[test]
    fn test_render_low() {
        let line = EffortIndicator::render(EffortLevel::Low);
        let content: String = line.spans.iter().map(|s| s.content.clone()).collect();
        assert!(content.contains("⚡"));
        assert!(content.contains("Low"));
    }

    #[test]
    fn test_render_high() {
        let line = EffortIndicator::render(EffortLevel::High);
        let content: String = line.spans.iter().map(|s| s.content.clone()).collect();
        assert!(content.contains("🔥"));
        assert!(content.contains("High"));
    }

    #[test]
    fn test_render_medium_has_two_spans() {
        let line = EffortIndicator::render(EffortLevel::Medium);
        assert_eq!(line.spans.len(), 2);
    }
}
