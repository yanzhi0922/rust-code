//! Shell output rendering component for the TUI.
//!
//! Provides rendering for shell command execution output, including
//! stdout/stderr lines, timing information, and progress indicators.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`ShellOutputLine`] | A single line of shell output |
//! | [`ShellOutputBlock`] | A block of shell output for one command |
//! | [`ShellOutputConfig`] | Display configuration |
//! | [`render_shell_output`] | Render shell output into lines |
//! | [`render_shell_progress`] | Render a running shell command indicator |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// ShellOutputLine
// ---------------------------------------------------------------------------

/// The stream source of a shell output line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputStream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// A single line of shell command output.
#[derive(Debug, Clone)]
pub struct ShellOutputLine {
    /// The text content of this line.
    pub text: String,
    /// Which stream this line came from.
    pub stream: OutputStream,
    /// Line number (1-based).
    pub line_number: usize,
}

// ---------------------------------------------------------------------------
// ShellOutputBlock
// ---------------------------------------------------------------------------

/// Status of a shell command execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellCommandStatus {
    /// Command is currently running.
    Running,
    /// Command completed successfully with exit code.
    Success(i32),
    /// Command failed with exit code.
    Failed(i32),
    /// Command was timed out.
    TimedOut,
    /// Command was interrupted by the user.
    Interrupted,
}

impl ShellCommandStatus {
    /// Returns the display label.
    pub fn label(&self) -> &'static str {
        match self {
            ShellCommandStatus::Running => "running",
            ShellCommandStatus::Success(_) => "done",
            ShellCommandStatus::Failed(_) => "failed",
            ShellCommandStatus::TimedOut => "timed out",
            ShellCommandStatus::Interrupted => "interrupted",
        }
    }

    /// Returns the color for the status indicator.
    pub fn color(&self) -> Color {
        match self {
            ShellCommandStatus::Running => Color::Yellow,
            ShellCommandStatus::Success(_) => Color::Green,
            ShellCommandStatus::Failed(_) => Color::Red,
            ShellCommandStatus::TimedOut => Color::Magenta,
            ShellCommandStatus::Interrupted => Color::Yellow,
        }
    }
}

/// A block of output from a shell command execution.
#[derive(Debug, Clone)]
pub struct ShellOutputBlock {
    /// The command that was executed.
    pub command: String,
    /// Current status.
    pub status: ShellCommandStatus,
    /// Output lines.
    pub lines: Vec<ShellOutputLine>,
    /// Execution duration in milliseconds (if completed).
    pub duration_ms: Option<u64>,
    /// Working directory.
    pub cwd: Option<String>,
    /// Whether output was truncated.
    pub truncated: bool,
    /// Total bytes of output (may differ from displayed lines).
    pub total_bytes: Option<usize>,
}

// ---------------------------------------------------------------------------
// ShellOutputConfig
// ---------------------------------------------------------------------------

/// Configuration for shell output display.
#[derive(Debug, Clone)]
pub struct ShellOutputConfig {
    /// Maximum number of output lines to display.
    pub max_lines: usize,
    /// Whether to show line numbers.
    pub show_line_numbers: bool,
    /// Whether to show timing information.
    pub show_timing: bool,
    /// Whether to show the command header.
    pub show_header: bool,
}

impl Default for ShellOutputConfig {
    fn default() -> Self {
        Self {
            max_lines: 50,
            show_line_numbers: false,
            show_timing: true,
            show_header: true,
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

fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        format!("{mins}m {secs}s")
    }
}

// ---------------------------------------------------------------------------
// Render functions
// ---------------------------------------------------------------------------

/// Render a shell output block.
pub fn render_shell_output(
    block: &ShellOutputBlock,
    config: &ShellOutputConfig,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Header
    if config.show_header {
        let status_color = block.status.color();
        let _status_label = block.status.label();

        let mut header_spans = vec![
            Span::styled(
                format!(" {} ", block.status.label().to_uppercase()),
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(block.command.clone(), Style::default().fg(style.status_fg)),
        ];

        if let Some(dur) = block.duration_ms
            && config.show_timing
        {
            header_spans.push(dim_span(&format!(" ({})", format_duration(dur))));
        }

        lines.push(Line::from(header_spans));
    }

    // Output lines
    let display_lines = if block.lines.len() > config.max_lines {
        let skip = block.lines.len() - config.max_lines;
        &block.lines[skip..]
    } else {
        &block.lines[..]
    };

    for output_line in display_lines {
        let line_style = match output_line.stream {
            OutputStream::Stdout => Style::default().fg(style.status_fg),
            OutputStream::Stderr => Style::default().fg(Color::Red),
        };

        let mut spans = Vec::new();

        if config.show_line_numbers {
            spans.push(dim_span(&format!("{:>4} │ ", output_line.line_number)));
        } else {
            spans.push(dim_span("   "));
        }

        spans.push(Span::styled(output_line.text.clone(), line_style));
        lines.push(Line::from(spans));
    }

    // Truncation notice
    if block.truncated {
        lines.push(Line::from(dim_span("   … output truncated …")));
        if let Some(bytes) = block.total_bytes {
            lines.push(Line::from(dim_span(&format!("   ({bytes} bytes total)"))));
        }
    }

    lines
}

/// Render a running shell command progress indicator.
pub fn render_shell_progress(
    command: &str,
    elapsed_ms: u64,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let frame = (elapsed_ms / 100) as usize % spinner_chars.len();
    let spinner = spinner_chars[frame];

    vec![Line::from(vec![
        Span::styled(
            format!(" {spinner} "),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(command.to_owned(), Style::default().fg(style.status_fg)),
        dim_span(&format!(" ({} elapsed)", format_duration(elapsed_ms))),
    ])]
}

/// Render a compact shell output summary (for collapsed view).
pub fn render_shell_summary(block: &ShellOutputBlock, _style: &StyleConfig) -> Vec<Line<'static>> {
    let status_color = block.status.color();
    let line_count = block.lines.len();

    let mut spans = vec![
        Span::styled(
            format!(" {} ", block.status.label()),
            Style::default().fg(status_color),
        ),
        Span::styled(
            block.command.clone(),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ];

    if let Some(dur) = block.duration_ms {
        spans.push(dim_span(&format!(
            " ({}, {line_count} lines)",
            format_duration(dur)
        )));
    } else {
        spans.push(dim_span(&format!(" ({line_count} lines)")));
    }

    vec![Line::from(spans)]
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

    fn sample_block(status: ShellCommandStatus) -> ShellOutputBlock {
        ShellOutputBlock {
            command: "cargo test".to_owned(),
            status,
            lines: vec![
                ShellOutputLine {
                    text: "running 42 tests".to_owned(),
                    stream: OutputStream::Stdout,
                    line_number: 1,
                },
                ShellOutputLine {
                    text: "test result: ok".to_owned(),
                    stream: OutputStream::Stdout,
                    line_number: 2,
                },
                ShellOutputLine {
                    text: "error: something".to_owned(),
                    stream: OutputStream::Stderr,
                    line_number: 3,
                },
            ],
            duration_ms: Some(1234),
            cwd: Some("/project".to_owned()),
            truncated: false,
            total_bytes: None,
        }
    }

    // -- ShellCommandStatus --

    #[test]
    fn status_label_running() {
        assert_eq!(ShellCommandStatus::Running.label(), "running");
    }

    #[test]
    fn status_label_success() {
        assert_eq!(ShellCommandStatus::Success(0).label(), "done");
    }

    #[test]
    fn status_label_failed() {
        assert_eq!(ShellCommandStatus::Failed(1).label(), "failed");
    }

    #[test]
    fn status_label_timed_out() {
        assert_eq!(ShellCommandStatus::TimedOut.label(), "timed out");
    }

    #[test]
    fn status_label_interrupted() {
        assert_eq!(ShellCommandStatus::Interrupted.label(), "interrupted");
    }

    #[test]
    fn status_color_running() {
        assert_eq!(ShellCommandStatus::Running.color(), Color::Yellow);
    }

    #[test]
    fn status_color_success() {
        assert_eq!(ShellCommandStatus::Success(0).color(), Color::Green);
    }

    #[test]
    fn status_color_failed() {
        assert_eq!(ShellCommandStatus::Failed(1).color(), Color::Red);
    }

    // -- format_duration --

    #[test]
    fn format_duration_millis() {
        assert_eq!(format_duration(500), "500ms");
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(2500), "2.5s");
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(format_duration(125000), "2m 5s");
    }

    // -- render_shell_output --

    #[test]
    fn render_output_shows_command() {
        let block = sample_block(ShellCommandStatus::Success(0));
        let lines = render_shell_output(&block, &ShellOutputConfig::default(), &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("cargo test"));
    }

    #[test]
    fn render_output_shows_timing() {
        let block = sample_block(ShellCommandStatus::Success(0));
        let lines = render_shell_output(&block, &ShellOutputConfig::default(), &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("1.2s"));
    }

    #[test]
    fn render_output_shows_output_lines() {
        let block = sample_block(ShellCommandStatus::Success(0));
        let lines = render_shell_output(&block, &ShellOutputConfig::default(), &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("running 42 tests"));
        assert!(combined.contains("test result: ok"));
    }

    #[test]
    fn render_output_no_header() {
        let block = sample_block(ShellCommandStatus::Success(0));
        let config = ShellOutputConfig {
            show_header: false,
            ..ShellOutputConfig::default()
        };
        let lines = render_shell_output(&block, &config, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(!combined.contains("DONE"));
    }

    #[test]
    fn render_output_with_line_numbers() {
        let block = sample_block(ShellCommandStatus::Success(0));
        let config = ShellOutputConfig {
            show_line_numbers: true,
            ..ShellOutputConfig::default()
        };
        let lines = render_shell_output(&block, &config, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("│"));
    }

    #[test]
    fn render_output_truncated() {
        let mut block = sample_block(ShellCommandStatus::Success(0));
        block.truncated = true;
        block.total_bytes = Some(99999);
        let lines = render_shell_output(&block, &ShellOutputConfig::default(), &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("output truncated"));
        assert!(combined.contains("99999 bytes"));
    }

    #[test]
    fn render_output_max_lines() {
        let mut block = sample_block(ShellCommandStatus::Success(0));
        block.lines = (0..100)
            .map(|i| ShellOutputLine {
                text: format!("line {i}"),
                stream: OutputStream::Stdout,
                line_number: i + 1,
            })
            .collect();
        let config = ShellOutputConfig {
            max_lines: 5,
            ..ShellOutputConfig::default()
        };
        let lines = render_shell_output(&block, &config, &test_style());
        // Header + 5 output lines = 6 lines
        assert_eq!(lines.len(), 6);
    }

    // -- render_shell_progress --

    #[test]
    fn render_progress_shows_command() {
        let lines = render_shell_progress("npm install", 5000, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("npm install"));
        assert!(combined.contains("elapsed"));
    }

    #[test]
    fn render_progress_shows_spinner() {
        let lines = render_shell_progress("test", 0, &test_style());
        let text = lines[0].to_string();
        // Should contain one of the spinner characters
        let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        assert!(spinner_chars.iter().any(|c| text.contains(*c)));
    }

    // -- render_shell_summary --

    #[test]
    fn render_summary_shows_command() {
        let block = sample_block(ShellCommandStatus::Success(0));
        let lines = render_shell_summary(&block, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("cargo test"));
        assert!(combined.contains("3 lines"));
    }

    #[test]
    fn render_summary_failed() {
        let block = sample_block(ShellCommandStatus::Failed(1));
        let lines = render_shell_summary(&block, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("failed"));
    }
}
