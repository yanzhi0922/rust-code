//! Progress indicator component (spinner / progress bar).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

/// Spinner characters for animation frames.
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// Get the spinner character for a given frame index.
pub fn spinner_char(frame: usize) -> &'static str {
    SPINNER_FRAMES[frame % SPINNER_FRAMES.len()]
}

/// Render a spinner line for streaming/processing state.
pub fn render_spinner(message: &str, frame: usize, style: &StyleConfig) -> Line<'static> {
    let spinner = spinner_char(frame);
    Line::from(vec![
        Span::styled(
            format!(" {spinner} "),
            Style::default()
                .fg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(message.to_owned(), Style::default().fg(style.status_fg)),
    ])
}

/// Render a progress bar line.
pub fn render_progress_bar(
    label: &str,
    current: usize,
    total: usize,
    width: usize,
    style: &StyleConfig,
) -> Line<'static> {
    let ratio = if total > 0 {
        current as f64 / total as f64
    } else {
        0.0
    };
    let bar_width = width.saturating_sub(label.len() + 10);
    let filled = (ratio * bar_width as f64) as usize;

    let bar_filled: String = "█".repeat(filled);
    let bar_empty: String = "░".repeat(bar_width.saturating_sub(filled));

    Line::from(vec![
        Span::styled(format!(" {label} "), Style::default().fg(style.status_fg)),
        Span::styled(bar_filled, Style::default().fg(style.accent_color)),
        Span::styled(bar_empty, Style::default().fg(style.info_color)),
        Span::styled(
            format!(" {current}/{total} "),
            Style::default().fg(style.info_color),
        ),
    ])
}

/// Render a token usage progress bar.
pub fn render_token_usage(
    used: usize,
    max: usize,
    width: usize,
    style: &StyleConfig,
) -> Line<'static> {
    let ratio = if max > 0 {
        used as f64 / max as f64
    } else {
        0.0
    };
    let percentage = ratio * 100.0;

    let color = if ratio > 0.9 {
        style.error_color
    } else if ratio > 0.7 {
        style.tool_color
    } else {
        style.accent_color
    };

    let bar_width = width.saturating_sub(20);
    let filled = (ratio * bar_width as f64) as usize;
    let bar_filled: String = "█".repeat(filled);
    let bar_empty: String = "░".repeat(bar_width.saturating_sub(filled));

    Line::from(vec![
        Span::styled(" Context: ", Style::default().fg(style.info_color)),
        Span::styled(bar_filled, Style::default().fg(color)),
        Span::styled(bar_empty, Style::default().fg(style.info_color)),
        Span::styled(format!(" {percentage:.0}% "), Style::default().fg(color)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::StyleConfig;

    #[test]
    fn spinner_char_cycles() {
        assert_eq!(spinner_char(0), "⠋");
        assert_eq!(spinner_char(1), "⠙");
        assert_eq!(spinner_char(8), "⠋"); // wraps around
    }

    #[test]
    fn render_spinner_line() {
        let style = StyleConfig::dark();
        let line = render_spinner("Thinking...", 0, &style);
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn render_progress_bar_half() {
        let style = StyleConfig::dark();
        let line = render_progress_bar("Progress", 50, 100, 40, &style);
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn render_progress_bar_zero_total() {
        let style = StyleConfig::dark();
        let line = render_progress_bar("Empty", 0, 0, 40, &style);
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn render_token_usage_high() {
        let style = StyleConfig::dark();
        let line = render_token_usage(9000, 10000, 40, &style);
        assert!(!line.spans.is_empty());
    }
}
