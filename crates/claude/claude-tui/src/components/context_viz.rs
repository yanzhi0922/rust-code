//! Context visualization component.
//!
//! Renders context usage as horizontal blocks, showing the distribution of
//! tokens across User / Assistant / Tool / System message types.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// The type of a context block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextBlockType {
    User,
    Assistant,
    Tool,
    System,
}

impl ContextBlockType {
    /// Colour used when rendering this block type.
    pub fn color(&self) -> Color {
        match self {
            ContextBlockType::User => Color::Blue,
            ContextBlockType::Assistant => Color::Green,
            ContextBlockType::Tool => Color::Yellow,
            ContextBlockType::System => Color::Magenta,
        }
    }

    /// Short label for the legend.
    pub fn label(&self) -> &'static str {
        match self {
            ContextBlockType::User => "User",
            ContextBlockType::Assistant => "Asst",
            ContextBlockType::Tool => "Tool",
            ContextBlockType::System => "Sys",
        }
    }
}

/// A single context block representing token usage for one message type.
#[derive(Debug, Clone)]
pub struct ContextBlock {
    pub block_type: ContextBlockType,
    pub token_count: u64,
}

// ---------------------------------------------------------------------------
// ContextVisualizer
// ---------------------------------------------------------------------------

/// Renders context usage as horizontal blocks with a summary.
pub struct ContextVisualizer;

impl ContextVisualizer {
    /// Total width of the visual bar in characters.
    const BAR_WIDTH: usize = 30;

    /// Render the context visualization.
    pub fn render(blocks: &[ContextBlock]) -> Vec<Line<'static>> {
        let total_tokens: u64 = blocks.iter().map(|b| b.token_count).sum();
        let mut lines = Vec::new();

        // Header
        lines.push(Line::from(vec![Span::styled(
            format!("Context: {total_tokens} tokens"),
            Style::default().add_modifier(Modifier::BOLD),
        )]));

        if total_tokens == 0 {
            lines.push(Line::from(Span::styled(
                "  (empty)".to_string(),
                Style::default().fg(Color::DarkGray),
            )));
            return lines;
        }

        // Visual bar
        let mut bar_spans: Vec<Span<'static>> = Vec::new();
        for block in blocks {
            if block.token_count == 0 {
                continue;
            }
            let width = ((block.token_count as f64 / total_tokens as f64) * Self::BAR_WIDTH as f64)
                .round() as usize;
            let width = width.max(1);
            bar_spans.push(Span::styled(
                "█".repeat(width),
                Style::default().fg(block.block_type.color()),
            ));
        }
        // Pad to full width if rounding caused shortfalls
        let current_width: usize = bar_spans.iter().map(|s| s.content.chars().count()).sum();
        if current_width < Self::BAR_WIDTH {
            bar_spans.push(Span::styled(
                "░".repeat(Self::BAR_WIDTH - current_width),
                Style::default().fg(Color::DarkGray),
            ));
        }
        lines.push(Line::from(bar_spans));

        // Legend
        let mut legend_spans: Vec<Span<'static>> = Vec::new();
        for block in blocks {
            let pct = (block.token_count as f64 / total_tokens as f64) * 100.0;
            legend_spans.push(Span::styled(
                format!(
                    " {}:{} {:.0}%",
                    block.block_type.label(),
                    block.token_count,
                    pct
                ),
                Style::default().fg(block.block_type.color()),
            ));
        }
        lines.push(Line::from(legend_spans));

        lines
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_empty_blocks() {
        let lines = ContextVisualizer::render(&[]);
        assert_eq!(lines.len(), 2); // header + (empty)
        let header: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(header.contains("0 tokens"));
    }

    #[test]
    fn test_render_single_block() {
        let blocks = vec![ContextBlock {
            block_type: ContextBlockType::User,
            token_count: 100,
        }];
        let lines = ContextVisualizer::render(&blocks);
        assert_eq!(lines.len(), 3); // header + bar + legend
    }

    #[test]
    fn test_render_header_shows_total() {
        let blocks = vec![
            ContextBlock {
                block_type: ContextBlockType::User,
                token_count: 300,
            },
            ContextBlock {
                block_type: ContextBlockType::Assistant,
                token_count: 200,
            },
        ];
        let lines = ContextVisualizer::render(&blocks);
        let header: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(header.contains("500 tokens"));
    }

    #[test]
    fn test_render_legend_contains_percentages() {
        let blocks = vec![
            ContextBlock {
                block_type: ContextBlockType::User,
                token_count: 750,
            },
            ContextBlock {
                block_type: ContextBlockType::Assistant,
                token_count: 250,
            },
        ];
        let lines = ContextVisualizer::render(&blocks);
        let legend: String = lines[2].spans.iter().map(|s| s.content.clone()).collect();
        assert!(legend.contains("75%"));
        assert!(legend.contains("25%"));
    }

    #[test]
    fn test_render_bar_width() {
        let blocks = vec![ContextBlock {
            block_type: ContextBlockType::System,
            token_count: 1000,
        }];
        let lines = ContextVisualizer::render(&blocks);
        let bar: String = lines[1].spans.iter().map(|s| s.content.clone()).collect();
        assert_eq!(bar.chars().count(), ContextVisualizer::BAR_WIDTH);
    }

    #[test]
    fn test_block_type_colors() {
        assert_eq!(ContextBlockType::User.color(), Color::Blue);
        assert_eq!(ContextBlockType::Assistant.color(), Color::Green);
        assert_eq!(ContextBlockType::Tool.color(), Color::Yellow);
        assert_eq!(ContextBlockType::System.color(), Color::Magenta);
    }

    #[test]
    fn test_render_multiple_blocks() {
        let blocks = vec![
            ContextBlock {
                block_type: ContextBlockType::User,
                token_count: 100,
            },
            ContextBlock {
                block_type: ContextBlockType::Assistant,
                token_count: 200,
            },
            ContextBlock {
                block_type: ContextBlockType::Tool,
                token_count: 50,
            },
            ContextBlock {
                block_type: ContextBlockType::System,
                token_count: 150,
            },
        ];
        let lines = ContextVisualizer::render(&blocks);
        assert_eq!(lines.len(), 3); // header + bar + legend
        let header: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(header.contains("500 tokens"));
    }
}
