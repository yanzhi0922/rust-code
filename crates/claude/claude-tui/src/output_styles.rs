//! Output style system for the TUI.
//!
//! Provides configurable output formatting styles inspired by
//! Claude Code's output style system. Supports:
//! - Multiple output styles (Default, Concise, Verbose, Technical)
//! - Per-style configuration (inline length, headers, bullets, code blocks)
//! - Output formatting with style-aware rendering

use std::fmt;

// ---------------------------------------------------------------------------
// Output Style
// ---------------------------------------------------------------------------

/// Available output styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputStyle {
    /// Default balanced style.
    Default,
    /// Concise — minimal output, shortened messages.
    Concise,
    /// Verbose — full detail, expanded explanations.
    Verbose,
    /// Technical — code-focused, structured output.
    Technical,
}

impl OutputStyle {
    /// All available styles.
    pub fn all() -> &'static [OutputStyle] {
        &[
            OutputStyle::Default,
            OutputStyle::Concise,
            OutputStyle::Verbose,
            OutputStyle::Technical,
        ]
    }

    /// Parse from string.
    pub fn from_str_lossy(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "concise" => OutputStyle::Concise,
            "verbose" => OutputStyle::Verbose,
            "technical" => OutputStyle::Technical,
            _ => OutputStyle::Default,
        }
    }

    /// Get the default configuration for this style.
    pub fn default_config(self) -> StyleConfig {
        match self {
            OutputStyle::Default => StyleConfig {
                max_inline_length: 120,
                use_headers: true,
                bullet_style: BulletStyle::Dash,
                code_block_style: CodeBlockStyle::Fenced,
                max_list_items: 10,
                indent_size: 2,
                wrap_width: 80,
                show_line_numbers: false,
                truncate_threshold: 500,
            },
            OutputStyle::Concise => StyleConfig {
                max_inline_length: 60,
                use_headers: false,
                bullet_style: BulletStyle::Compact,
                code_block_style: CodeBlockStyle::Inline,
                max_list_items: 5,
                indent_size: 1,
                wrap_width: 60,
                show_line_numbers: false,
                truncate_threshold: 200,
            },
            OutputStyle::Verbose => StyleConfig {
                max_inline_length: 200,
                use_headers: true,
                bullet_style: BulletStyle::Numbered,
                code_block_style: CodeBlockStyle::Fenced,
                max_list_items: 50,
                indent_size: 4,
                wrap_width: 100,
                show_line_numbers: true,
                truncate_threshold: 2000,
            },
            OutputStyle::Technical => StyleConfig {
                max_inline_length: 120,
                use_headers: true,
                bullet_style: BulletStyle::Dash,
                code_block_style: CodeBlockStyle::Fenced,
                max_list_items: 20,
                indent_size: 2,
                wrap_width: 100,
                show_line_numbers: true,
                truncate_threshold: 1000,
            },
        }
    }
}

impl fmt::Display for OutputStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => write!(f, "default"),
            Self::Concise => write!(f, "concise"),
            Self::Verbose => write!(f, "verbose"),
            Self::Technical => write!(f, "technical"),
        }
    }
}

// ---------------------------------------------------------------------------
// Bullet Style
// ---------------------------------------------------------------------------

/// How list items are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BulletStyle {
    /// `- item`
    Dash,
    /// `• item`
    Compact,
    /// `1. item`
    Numbered,
    /// `* item`
    Asterisk,
}

impl BulletStyle {
    /// Get the prefix for a bullet at the given index.
    pub fn prefix(self, index: usize) -> String {
        match self {
            Self::Dash => "- ".to_string(),
            Self::Compact => "• ".to_string(),
            Self::Numbered => format!("{}. ", index + 1),
            Self::Asterisk => "* ".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Code Block Style
// ---------------------------------------------------------------------------

/// How code blocks are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodeBlockStyle {
    /// ```lang\n code \n```
    Fenced,
    /// Inline `code` rendering.
    Inline,
    /// Indented by 4 spaces.
    Indented,
}

// ---------------------------------------------------------------------------
// Style Config
// ---------------------------------------------------------------------------

/// Configuration for an output style.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleConfig {
    /// Maximum length for inline content before truncation.
    pub max_inline_length: usize,
    /// Whether to use markdown headers.
    pub use_headers: bool,
    /// Bullet style for lists.
    pub bullet_style: BulletStyle,
    /// Code block rendering style.
    pub code_block_style: CodeBlockStyle,
    /// Maximum number of list items before "N more...".
    pub max_list_items: usize,
    /// Indentation size in spaces.
    pub indent_size: usize,
    /// Text wrap width.
    pub wrap_width: usize,
    /// Whether to show line numbers in code blocks.
    pub show_line_numbers: bool,
    /// Character threshold for truncation.
    pub truncate_threshold: usize,
}

impl Default for StyleConfig {
    fn default() -> Self {
        OutputStyle::Default.default_config()
    }
}

impl StyleConfig {
    /// Create a new config with custom max_inline_length.
    pub fn with_max_inline_length(mut self, len: usize) -> Self {
        self.max_inline_length = len;
        self
    }

    /// Create a new config with custom wrap_width.
    pub fn with_wrap_width(mut self, width: usize) -> Self {
        self.wrap_width = width;
        self
    }

    /// Create a new config with line numbers enabled.
    pub fn with_line_numbers(mut self, show: bool) -> Self {
        self.show_line_numbers = show;
        self
    }

    /// Truncate text to max_inline_length, adding "..." if truncated.
    pub fn truncate_inline(&self, text: &str) -> String {
        if text.len() <= self.max_inline_length {
            return text.to_string();
        }
        let end = self.max_inline_length.saturating_sub(3);
        // Find a safe char boundary.
        let mut boundary = end;
        while boundary > 0 && !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        format!("{}...", &text[..boundary])
    }

    /// Wrap text to the configured wrap_width.
    pub fn wrap_text(&self, text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        for line in text.lines() {
            if line.len() <= self.wrap_width {
                result.push_str(line);
                result.push('\n');
            } else {
                let mut remaining = line;
                let indent = " ".repeat(self.indent_size);
                let mut first = true;
                while remaining.len() > self.wrap_width {
                    let mut split_at = self.wrap_width;
                    // Try to split at a space boundary.
                    if let Some(space_pos) = remaining[..split_at].rfind(' ') {
                        split_at = space_pos;
                    }
                    // Ensure valid char boundary.
                    while split_at > 0 && !remaining.is_char_boundary(split_at) {
                        split_at -= 1;
                    }
                    if split_at == 0 {
                        break;
                    }
                    if first {
                        result.push_str(&remaining[..split_at]);
                        first = false;
                    } else {
                        result.push_str(&indent);
                        result.push_str(remaining[..split_at].trim_start());
                    }
                    result.push('\n');
                    remaining = remaining[split_at..].trim_start();
                }
                if !remaining.is_empty() {
                    if first {
                        result.push_str(remaining);
                    } else {
                        result.push_str(&indent);
                        result.push_str(remaining.trim_start());
                    }
                    result.push('\n');
                }
            }
        }
        // Remove trailing newline if the input didn't have one.
        if text.ends_with('\n') {
            result
        } else if result.ends_with('\n') {
            result.pop();
            result
        } else {
            result
        }
    }
}

// ---------------------------------------------------------------------------
// Output Formatter
// ---------------------------------------------------------------------------

/// Formats output according to a style configuration.
#[derive(Debug, Clone)]
pub struct OutputFormatter {
    /// The active style.
    style: OutputStyle,
    /// The style configuration.
    config: StyleConfig,
}

impl OutputFormatter {
    /// Create a new formatter with the given style.
    pub fn new(style: OutputStyle) -> Self {
        let config = style.default_config();
        OutputFormatter { style, config }
    }

    /// Create a formatter with custom config.
    pub fn with_config(style: OutputStyle, config: StyleConfig) -> Self {
        OutputFormatter { style, config }
    }

    /// Get the current style.
    pub fn style(&self) -> OutputStyle {
        self.style
    }

    /// Get the current config.
    pub fn config(&self) -> &StyleConfig {
        &self.config
    }

    /// Change the output style.
    pub fn set_style(&mut self, style: OutputStyle) {
        self.style = style;
        self.config = style.default_config();
    }

    /// Format a header.
    pub fn format_header(&self, text: &str, level: usize) -> String {
        if !self.config.use_headers {
            return format!("{}:", text);
        }
        let hashes = "#".repeat(level.clamp(1, 6));
        format!("{hashes} {text}")
    }

    /// Format a list of items.
    pub fn format_list(&self, items: &[&str]) -> String {
        let visible_count = items.len().min(self.config.max_list_items);
        let mut result = String::new();

        for (i, item) in items.iter().take(visible_count).enumerate() {
            let prefix = self.config.bullet_style.prefix(i);
            let wrapped = self.config.wrap_text(item);
            result.push_str(&format!("{prefix}{wrapped}"));
            result.push('\n');
        }

        let remaining = items.len().saturating_sub(visible_count);
        if remaining > 0 {
            result.push_str(&format!("  ... and {remaining} more\n"));
        }

        result
    }

    /// Format a code block.
    pub fn format_code(&self, code: &str, language: &str) -> String {
        match self.config.code_block_style {
            CodeBlockStyle::Fenced => {
                let mut result = format!("```{language}\n");
                if self.config.show_line_numbers {
                    for (i, line) in code.lines().enumerate() {
                        result.push_str(&format!("{:>4} | {line}\n", i + 1));
                    }
                } else {
                    result.push_str(code);
                    if !code.ends_with('\n') {
                        result.push('\n');
                    }
                }
                result.push_str("```");
                result
            }
            CodeBlockStyle::Inline => {
                let single_line: String = code.lines().collect::<Vec<_>>().join("; ");
                self.config.truncate_inline(&single_line)
            }
            CodeBlockStyle::Indented => {
                let indent = " ".repeat(4);
                let mut result = String::new();
                for line in code.lines() {
                    result.push_str(&format!("{indent}{line}\n"));
                }
                result
            }
        }
    }

    /// Format inline text (truncated if needed).
    pub fn format_inline(&self, text: &str) -> String {
        self.config.truncate_inline(text)
    }

    /// Format a section with header and body.
    pub fn format_section(&self, title: &str, body: &str) -> String {
        let mut result = self.format_header(title, 2);
        result.push('\n');
        result.push('\n');
        let wrapped = self.config.wrap_text(body);
        result.push_str(&wrapped);
        result
    }

    /// Format a truncated message body.
    pub fn format_body(&self, text: &str) -> String {
        if text.len() <= self.config.truncate_threshold {
            return self.config.wrap_text(text);
        }
        let truncated = self.config.truncate_inline(text);
        let remaining = text.len().saturating_sub(self.config.max_inline_length);
        format!(
            "{}\n\n[... truncated, {remaining} more characters ...]",
            truncated
        )
    }
}

impl Default for OutputFormatter {
    fn default() -> Self {
        Self::new(OutputStyle::Default)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_style_from_str() {
        assert_eq!(OutputStyle::from_str_lossy("concise"), OutputStyle::Concise);
        assert_eq!(OutputStyle::from_str_lossy("VERBOSE"), OutputStyle::Verbose);
        assert_eq!(
            OutputStyle::from_str_lossy("technical"),
            OutputStyle::Technical
        );
        assert_eq!(OutputStyle::from_str_lossy("unknown"), OutputStyle::Default);
        assert_eq!(OutputStyle::from_str_lossy("default"), OutputStyle::Default);
    }

    #[test]
    fn test_output_style_display() {
        assert_eq!(OutputStyle::Default.to_string(), "default");
        assert_eq!(OutputStyle::Concise.to_string(), "concise");
        assert_eq!(OutputStyle::Verbose.to_string(), "verbose");
        assert_eq!(OutputStyle::Technical.to_string(), "technical");
    }

    #[test]
    fn test_output_style_all() {
        assert_eq!(OutputStyle::all().len(), 4);
    }

    #[test]
    fn test_default_config() {
        let config = OutputStyle::Default.default_config();
        assert_eq!(config.max_inline_length, 120);
        assert!(config.use_headers);
        assert_eq!(config.bullet_style, BulletStyle::Dash);
        assert_eq!(config.code_block_style, CodeBlockStyle::Fenced);
        assert!(!config.show_line_numbers);
    }

    #[test]
    fn test_concise_config() {
        let config = OutputStyle::Concise.default_config();
        assert_eq!(config.max_inline_length, 60);
        assert!(!config.use_headers);
        assert_eq!(config.bullet_style, BulletStyle::Compact);
        assert_eq!(config.code_block_style, CodeBlockStyle::Inline);
    }

    #[test]
    fn test_verbose_config() {
        let config = OutputStyle::Verbose.default_config();
        assert_eq!(config.max_inline_length, 200);
        assert!(config.show_line_numbers);
        assert_eq!(config.bullet_style, BulletStyle::Numbered);
    }

    #[test]
    fn test_technical_config() {
        let config = OutputStyle::Technical.default_config();
        assert!(config.show_line_numbers);
        assert_eq!(config.wrap_width, 100);
    }

    #[test]
    fn test_bullet_style_prefix() {
        assert_eq!(BulletStyle::Dash.prefix(0), "- ");
        assert_eq!(BulletStyle::Compact.prefix(0), "• ");
        assert_eq!(BulletStyle::Numbered.prefix(0), "1. ");
        assert_eq!(BulletStyle::Numbered.prefix(4), "5. ");
        assert_eq!(BulletStyle::Asterisk.prefix(0), "* ");
    }

    #[test]
    fn test_truncate_inline_short() {
        let config = StyleConfig::default();
        assert_eq!(config.truncate_inline("hello"), "hello");
    }

    #[test]
    fn test_truncate_inline_long() {
        let config = StyleConfig::default().with_max_inline_length(10);
        let long_text = "abcdefghijkmno";
        let truncated = config.truncate_inline(long_text);
        assert!(truncated.len() <= 13); // 10 chars + "..."
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_wrap_text_short() {
        let config = StyleConfig::default();
        let result = config.wrap_text("hello world");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_wrap_text_long_line() {
        let config = StyleConfig::default().with_wrap_width(20);
        let long_line = "this is a very long line that should be wrapped";
        let wrapped = config.wrap_text(long_line);
        for line in wrapped.lines() {
            // Each line should be at most wrap_width + indent.
            assert!(
                line.len() <= 30,
                "line too long: '{}' ({} chars)",
                line,
                line.len()
            );
        }
    }

    #[test]
    fn test_formatter_header() {
        let fmt = OutputFormatter::new(OutputStyle::Default);
        assert_eq!(fmt.format_header("Title", 1), "# Title");
        assert_eq!(fmt.format_header("Title", 2), "## Title");
        assert_eq!(fmt.format_header("Title", 3), "### Title");
    }

    #[test]
    fn test_formatter_header_no_headers() {
        let config = OutputStyle::Concise.default_config();
        let fmt = OutputFormatter::with_config(OutputStyle::Concise, config);
        assert_eq!(fmt.format_header("Title", 1), "Title:");
    }

    #[test]
    fn test_formatter_list() {
        let fmt = OutputFormatter::new(OutputStyle::Default);
        let items = vec!["item 1", "item 2", "item 3"];
        let result = fmt.format_list(&items);
        assert!(result.contains("- item 1"));
        assert!(result.contains("- item 2"));
        assert!(result.contains("- item 3"));
    }

    #[test]
    fn test_formatter_list_truncation() {
        let config = OutputStyle::Default.default_config();
        let fmt = OutputFormatter::with_config(
            OutputStyle::Default,
            StyleConfig {
                max_list_items: 2,
                ..config
            },
        );
        let items: Vec<&str> = (0..5)
            .map(|i| {
                static ITEMS: [&str; 5] = ["a", "b", "c", "d", "e"];
                ITEMS[i]
            })
            .collect();
        let result = fmt.format_list(&items);
        assert!(result.contains("... and 3 more"));
    }

    #[test]
    fn test_formatter_code_fenced() {
        let fmt = OutputFormatter::new(OutputStyle::Default);
        let code = "fn main() {\n    println!(\"hello\");\n}";
        let result = fmt.format_code(code, "rust");
        assert!(result.starts_with("```rust\n"));
        assert!(result.ends_with("```"));
    }

    #[test]
    fn test_formatter_code_inline() {
        let fmt = OutputFormatter::new(OutputStyle::Concise);
        let code = "fn main() { println!(\"hello\"); }";
        let result = fmt.format_code(code, "rust");
        assert!(!result.contains("```"));
    }

    #[test]
    fn test_formatter_code_with_line_numbers() {
        let fmt = OutputFormatter::new(OutputStyle::Verbose);
        let code = "line1\nline2\nline3";
        let result = fmt.format_code(code, "rust");
        assert!(result.contains("1 |"));
        assert!(result.contains("2 |"));
        assert!(result.contains("3 |"));
    }

    #[test]
    fn test_formatter_set_style() {
        let mut fmt = OutputFormatter::new(OutputStyle::Default);
        assert_eq!(fmt.style(), OutputStyle::Default);
        fmt.set_style(OutputStyle::Concise);
        assert_eq!(fmt.style(), OutputStyle::Concise);
        assert_eq!(fmt.config().max_inline_length, 60);
    }

    #[test]
    fn test_formatter_format_section() {
        let fmt = OutputFormatter::new(OutputStyle::Default);
        let result = fmt.format_section("Results", "All tests passed.");
        assert!(result.contains("## Results"));
        assert!(result.contains("All tests passed."));
    }

    #[test]
    fn test_style_config_builder() {
        let config = StyleConfig::default()
            .with_max_inline_length(50)
            .with_wrap_width(40)
            .with_line_numbers(true);
        assert_eq!(config.max_inline_length, 50);
        assert_eq!(config.wrap_width, 40);
        assert!(config.show_line_numbers);
    }
}
