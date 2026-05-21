//! Token usage indicator component.
//!
//! Renders a progress bar showing token consumption with colour-coded
//! thresholds: green (<50 %), yellow (50–80 %), red (>80 %).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Token-usage snapshot used for rendering.
#[derive(Debug, Clone)]
pub struct TokenUsageData {
    pub used_tokens: u64,
    pub max_tokens: u64,
    pub percentage: f64,
}

impl TokenUsageData {
    /// Convenience constructor that auto-computes `percentage`.
    pub fn new(used_tokens: u64, max_tokens: u64) -> Self {
        let percentage = if max_tokens == 0 {
            0.0
        } else {
            (used_tokens as f64 / max_tokens as f64) * 100.0
        };
        Self {
            used_tokens,
            max_tokens,
            percentage,
        }
    }

    /// Returns the colour that corresponds to the current usage level.
    pub fn color(&self) -> Color {
        if self.percentage < 50.0 {
            Color::Green
        } else if self.percentage <= 80.0 {
            Color::Yellow
        } else {
            Color::Red
        }
    }
}

// ---------------------------------------------------------------------------
// TokenIndicator — progress bar renderer
// ---------------------------------------------------------------------------

/// Renders a token-usage progress bar.
pub struct TokenIndicator;

impl TokenIndicator {
    /// Width of the visual progress bar (characters).
    const BAR_WIDTH: usize = 20;

    /// Render the token indicator as a vector of lines.
    pub fn render(data: &TokenUsageData) -> Vec<Line<'static>> {
        let color = data.color();
        let filled = ((data.percentage / 100.0) * Self::BAR_WIDTH as f64).round() as usize;
        let filled = filled.min(Self::BAR_WIDTH);
        let empty = Self::BAR_WIDTH - filled;

        let bar_str = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

        let label = format!(
            "Tokens: {}/{} ({:.1}%)",
            data.used_tokens, data.max_tokens, data.percentage
        );

        vec![
            Line::from(vec![Span::styled(
                label,
                Style::default().add_modifier(Modifier::BOLD),
            )]),
            Line::from(vec![Span::styled(bar_str, Style::default().fg(color))]),
        ]
    }
}

// ---------------------------------------------------------------------------
// TokenWarning — renders a warning when usage exceeds 80 %
// ---------------------------------------------------------------------------

/// Renders a warning banner when token usage is critically high.
pub struct TokenWarning;

impl TokenWarning {
    /// Threshold above which a warning is emitted.
    const WARNING_THRESHOLD: f64 = 80.0;

    /// Render a warning line if usage exceeds the threshold.
    /// Returns `Some(lines)` when a warning is active, `None` otherwise.
    pub fn render(data: &TokenUsageData) -> Option<Vec<Line<'static>>> {
        if data.percentage <= Self::WARNING_THRESHOLD {
            return None;
        }

        let msg = format!(
            "⚠ Warning: Token usage at {:.1}% — consider compacting context",
            data.percentage,
        );

        Some(vec![Line::from(vec![Span::styled(
            msg,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )])])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- TokenUsageData tests --

    #[test]
    fn test_usage_data_new_computes_percentage() {
        let data = TokenUsageData::new(500, 1000);
        assert_eq!(data.used_tokens, 500);
        assert_eq!(data.max_tokens, 1000);
        assert!((data.percentage - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_usage_data_new_zero_max() {
        let data = TokenUsageData::new(100, 0);
        assert_eq!(data.percentage, 0.0);
    }

    #[test]
    fn test_color_green_below_50() {
        let data = TokenUsageData::new(30, 100);
        assert_eq!(data.color(), Color::Green);
    }

    #[test]
    fn test_color_yellow_50_to_80() {
        let data = TokenUsageData::new(65, 100);
        assert_eq!(data.color(), Color::Yellow);
    }

    #[test]
    fn test_color_red_above_80() {
        let data = TokenUsageData::new(90, 100);
        assert_eq!(data.color(), Color::Red);
    }

    // -- TokenIndicator tests --

    #[test]
    fn test_render_produces_two_lines() {
        let data = TokenUsageData::new(500, 1000);
        let lines = TokenIndicator::render(&data);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_render_label_contains_percentage() {
        let data = TokenUsageData::new(250, 1000);
        let lines = TokenIndicator::render(&data);
        // First line should contain "25.0%"
        let label_span = &lines[0];
        let content: String = label_span.spans.iter().map(|s| s.content.clone()).collect();
        assert!(content.contains("25.0%"));
    }

    #[test]
    fn test_render_bar_has_correct_width() {
        let data = TokenUsageData::new(500, 1000);
        let lines = TokenIndicator::render(&data);
        let bar_span = &lines[1];
        let bar_content: String = bar_span.spans.iter().map(|s| s.content.clone()).collect();
        assert_eq!(bar_content.chars().count(), TokenIndicator::BAR_WIDTH);
    }

    // -- TokenWarning tests --

    #[test]
    fn test_warning_none_below_threshold() {
        let data = TokenUsageData::new(50, 100);
        assert!(TokenWarning::render(&data).is_none());
    }

    #[test]
    fn test_warning_some_above_threshold() {
        let data = TokenUsageData::new(90, 100);
        let result = TokenWarning::render(&data);
        assert!(result.is_some());
        let lines = result.expect("warning should render above threshold");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_warning_message_contains_percentage() {
        let data = TokenUsageData::new(85, 100);
        let lines = TokenWarning::render(&data).expect("warning should render above threshold");
        let content: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(content.contains("85.0%"));
    }
}
