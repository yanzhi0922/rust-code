//! Progress bar component for the TUI.
//!
//! Provides configurable progress bar widgets with multiple visual styles,
//! including solid blocks, dotted bars, and pip-style indicators.
//!
//! # Styles
//!
//! | Style | Description |
//! |-------|-------------|
//! | [`ProgressStyle::Solid`] | `████░░░░` solid block bar |
//! | [`ProgressStyle::Dotted`] | `⣿⣿⣿⣀⣀` braille dot bar |
//! | [`ProgressStyle::Pip`] | `▸▸▸▸ · · · ·` arrow pip bar |
//! | [`ProgressStyle::Hash`] | `####----` hash bar |
//! | [`ProgressStyle::Minimal`] | `●●●○○` minimal circle bar |

#![allow(dead_code)]

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// ProgressStyle
// ---------------------------------------------------------------------------

/// Visual style for the progress bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStyle {
    /// Solid block characters: `████░░░░`
    Solid,
    /// Braille dot characters: `⣿⣿⣿⣀⣀`
    Dotted,
    /// Arrow pip characters: `▸▸▸▸ · · · ·`
    Pip,
    /// Hash characters: `####----`
    Hash,
    /// Minimal circle characters: `●●●○○`
    Minimal,
}

impl ProgressStyle {
    /// Get the filled and empty characters for this style.
    pub fn chars(self) -> (&'static str, &'static str) {
        match self {
            Self::Solid => ("█", "░"),
            Self::Dotted => ("⣿", "⣀"),
            Self::Pip => ("▸", "·"),
            Self::Hash => ("#", "-"),
            Self::Minimal => ("●", "○"),
        }
    }
}

// ---------------------------------------------------------------------------
// ProgressBar
// ---------------------------------------------------------------------------

/// A progress bar with configurable label, value, and style.
#[derive(Debug, Clone)]
pub struct ProgressBar {
    /// Label displayed before the bar.
    pub label: String,
    /// Current progress value.
    pub current: usize,
    /// Maximum progress value.
    pub total: usize,
    /// Visual style of the bar.
    pub style: ProgressStyle,
    /// Width of the bar in characters (excluding label).
    pub width: usize,
    /// Whether to show the percentage.
    pub show_percent: bool,
    /// Whether to show the fraction (current/total).
    pub show_fraction: bool,
}

impl ProgressBar {
    /// Create a new progress bar.
    pub fn new(label: impl Into<String>, current: usize, total: usize) -> Self {
        Self {
            label: label.into(),
            current,
            total,
            style: ProgressStyle::Solid,
            width: 30,
            show_percent: true,
            show_fraction: false,
        }
    }

    /// Set the visual style.
    pub fn with_style(mut self, style: ProgressStyle) -> Self {
        self.style = style;
        self
    }

    /// Set the bar width.
    pub fn with_width(mut self, width: usize) -> Self {
        self.width = width.max(1);
        self
    }

    /// Show the percentage.
    pub fn with_percent(mut self, show: bool) -> Self {
        self.show_percent = show;
        self
    }

    /// Show the fraction.
    pub fn with_fraction(mut self, show: bool) -> Self {
        self.show_fraction = show;
        self
    }

    /// Get the progress ratio (0.0 to 1.0).
    pub fn ratio(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.current as f64 / self.total as f64).min(1.0)
        }
    }

    /// Get the percentage (0 to 100).
    pub fn percent(&self) -> u8 {
        (self.ratio() * 100.0).min(100.0) as u8
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render a progress bar into a single ratatui line.
///
/// The bar is colored based on the progress ratio:
/// - Under 70%: accent color
/// - 70–90%: tool/warning color
/// - Over 90%: error color
pub fn render_progress_bar(bar: &ProgressBar, style: &StyleConfig) -> Line<'static> {
    let ratio = bar.ratio();
    let (filled_char, empty_char) = bar.style.chars();
    let filled_count = (ratio * bar.width as f64) as usize;
    let empty_count = bar.width.saturating_sub(filled_count);

    // Color based on progress
    let bar_color = if ratio > 0.9 {
        style.error_color
    } else if ratio > 0.7 {
        style.tool_color
    } else {
        style.accent_color
    };

    let filled: String = filled_char.repeat(filled_count);
    let empty: String = empty_char.repeat(empty_count);

    let mut spans = vec![
        Span::styled(
            format!(" {} ", bar.label),
            Style::default().fg(style.status_fg),
        ),
        Span::styled(filled, Style::default().fg(bar_color)),
        Span::styled(empty, Style::default().fg(style.info_color)),
    ];

    if bar.show_percent {
        spans.push(Span::styled(
            format!(" {}%", bar.percent()),
            Style::default().fg(bar_color),
        ));
    }

    if bar.show_fraction {
        spans.push(Span::styled(
            format!(" {}/{}", bar.current, bar.total),
            Style::default().fg(style.info_color),
        ));
    }

    Line::from(spans)
}

/// Render a multi-line progress display with a label, bar, and detail line.
pub fn render_progress_detail(
    bar: &ProgressBar,
    detail: &str,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let bar_line = render_progress_bar(bar, style);

    let detail_line = Line::from(vec![
        Span::styled(
            format!(" {} ", " ".repeat(bar.label.len())),
            Style::default(),
        ),
        Span::styled(
            detail.to_owned(),
            Style::default()
                .fg(style.info_color)
                .add_modifier(Modifier::DIM),
        ),
    ]);

    vec![bar_line, detail_line]
}

/// Render a spinner-based indeterminate progress indicator.
pub fn render_indeterminate(label: &str, frame: usize, style: &StyleConfig) -> Line<'static> {
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
    let spinner = frames[frame % frames.len()];

    Line::from(vec![
        Span::styled(
            format!(" {spinner} "),
            Style::default()
                .fg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(label.to_owned(), Style::default().fg(style.status_fg)),
        Span::styled(
            "…".to_owned(),
            Style::default()
                .fg(style.info_color)
                .add_modifier(Modifier::DIM),
        ),
    ])
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

    // --- ProgressBar struct tests ---

    #[test]
    fn progress_bar_new() {
        let bar = ProgressBar::new("Loading", 50, 100);
        assert_eq!(bar.label, "Loading");
        assert_eq!(bar.current, 50);
        assert_eq!(bar.total, 100);
        assert_eq!(bar.style, ProgressStyle::Solid);
    }

    #[test]
    fn progress_bar_builder() {
        let bar = ProgressBar::new("Test", 0, 10)
            .with_style(ProgressStyle::Dotted)
            .with_width(20)
            .with_percent(false)
            .with_fraction(true);
        assert_eq!(bar.style, ProgressStyle::Dotted);
        assert_eq!(bar.width, 20);
        assert!(!bar.show_percent);
        assert!(bar.show_fraction);
    }

    #[test]
    fn progress_ratio_half() {
        let bar = ProgressBar::new("T", 5, 10);
        assert!((bar.ratio() - 0.5).abs() < 0.01);
    }

    #[test]
    fn progress_ratio_zero_total() {
        let bar = ProgressBar::new("T", 5, 0);
        assert_eq!(bar.ratio(), 0.0);
    }

    #[test]
    fn progress_ratio_capped_at_one() {
        let bar = ProgressBar::new("T", 200, 100);
        assert_eq!(bar.ratio(), 1.0);
    }

    #[test]
    fn progress_percent() {
        let bar = ProgressBar::new("T", 75, 100);
        assert_eq!(bar.percent(), 75);
    }

    // --- ProgressStyle tests ---

    #[test]
    fn style_solid_chars() {
        let (f, e) = ProgressStyle::Solid.chars();
        assert_eq!(f, "█");
        assert_eq!(e, "░");
    }

    #[test]
    fn style_dotted_chars() {
        let (f, e) = ProgressStyle::Dotted.chars();
        assert_eq!(f, "⣿");
        assert_eq!(e, "⣀");
    }

    #[test]
    fn style_pip_chars() {
        let (f, e) = ProgressStyle::Pip.chars();
        assert_eq!(f, "▸");
        assert_eq!(e, "·");
    }

    #[test]
    fn style_hash_chars() {
        let (f, e) = ProgressStyle::Hash.chars();
        assert_eq!(f, "#");
        assert_eq!(e, "-");
    }

    #[test]
    fn style_minimal_chars() {
        let (f, e) = ProgressStyle::Minimal.chars();
        assert_eq!(f, "●");
        assert_eq!(e, "○");
    }

    // --- Render tests ---

    #[test]
    fn render_bar_basic() {
        let bar = ProgressBar::new("Progress", 50, 100);
        let line = render_progress_bar(&bar, &test_style());
        let text = line.to_string();
        assert!(text.contains("Progress"));
        assert!(text.contains("50%"));
    }

    #[test]
    fn render_bar_zero_progress() {
        let bar = ProgressBar::new("Start", 0, 100);
        let line = render_progress_bar(&bar, &test_style());
        let text = line.to_string();
        assert!(text.contains("0%"));
    }

    #[test]
    fn render_bar_full_progress() {
        let bar = ProgressBar::new("Done", 100, 100);
        let line = render_progress_bar(&bar, &test_style());
        let text = line.to_string();
        assert!(text.contains("100%"));
    }

    #[test]
    fn render_bar_with_fraction() {
        let bar = ProgressBar::new("Items", 3, 7).with_fraction(true);
        let line = render_progress_bar(&bar, &test_style());
        let text = line.to_string();
        assert!(text.contains("3/7"));
    }

    #[test]
    fn render_bar_no_percent() {
        let bar = ProgressBar::new("Hide", 50, 100).with_percent(false);
        let line = render_progress_bar(&bar, &test_style());
        let text = line.to_string();
        assert!(!text.contains('%'));
    }

    #[test]
    fn render_bar_dotted_style() {
        let bar = ProgressBar::new("Dots", 50, 100).with_style(ProgressStyle::Dotted);
        let line = render_progress_bar(&bar, &test_style());
        let text = line.to_string();
        assert!(text.contains('⣿'));
        assert!(text.contains('⣀'));
    }

    #[test]
    fn render_bar_minimal_style() {
        let bar = ProgressBar::new("Mini", 2, 5)
            .with_style(ProgressStyle::Minimal)
            .with_width(5)
            .with_percent(false);
        let line = render_progress_bar(&bar, &test_style());
        let text = line.to_string();
        assert!(text.contains('●'));
        assert!(text.contains('○'));
    }

    #[test]
    fn render_progress_detail_multi_line() {
        let bar = ProgressBar::new("Download", 30, 100);
        let lines = render_progress_detail(&bar, "Fetching data…", &test_style());
        assert_eq!(lines.len(), 2);
        assert!(lines[0].to_string().contains("Download"));
        assert!(lines[1].to_string().contains("Fetching"));
    }

    #[test]
    fn indeterminate_spinner() {
        let line = super::render_indeterminate("Loading", 0, &test_style());
        let text = line.to_string();
        assert!(text.contains("Loading"));
        assert!(text.contains('…'));
    }

    #[test]
    fn indeterminate_frames_differ() {
        let line0 = super::render_indeterminate("T", 0, &test_style()).to_string();
        let line3 = super::render_indeterminate("T", 3, &test_style()).to_string();
        // Different frames should produce different spinner characters
        assert_ne!(line0, line3);
    }
}
