//! Markdown rendering component.
//!
//! Converts Markdown text into ratatui [`Line`] sequences with basic
//! styling for headings, lists, code blocks, bold, italic, and inline code.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;
use crate::syntax::{self, Language, SyntaxColors};

/// Render Markdown content into ratatui Lines.
pub fn render_markdown(content: &str, width: usize, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut code_lang = Language::Plain;
    let mut code_buffer = String::new();
    let syntax_colors = SyntaxColors::dark();

    for raw_line in content.lines() {
        if raw_line.starts_with("```") {
            if in_code_block {
                // End of code block.
                in_code_block = false;
                let code_lines = syntax::highlight_code(&code_buffer, code_lang, &syntax_colors);
                lines.extend(code_lines);
                // Add closing fence indicator.
                lines.push(Line::from(Span::styled(
                    "─".repeat(width.min(40)),
                    Style::default().fg(style.info_color),
                )));
                code_buffer.clear();
            } else {
                // Start of code block.
                in_code_block = true;
                let fence_info = raw_line.trim_start_matches('`').trim();
                code_lang = Language::from_fence(fence_info);
                lines.push(Line::from(Span::styled(
                    format!("─ {} ", fence_info),
                    Style::default().fg(style.info_color),
                )));
            }
            continue;
        }

        if in_code_block {
            code_buffer.push_str(raw_line);
            code_buffer.push('\n');
            continue;
        }

        // Headings.
        if raw_line.starts_with("# ") {
            lines.push(render_heading(raw_line, 1, style));
            continue;
        }
        if raw_line.starts_with("## ") {
            lines.push(render_heading(raw_line, 2, style));
            continue;
        }
        if raw_line.starts_with("### ") {
            lines.push(render_heading(raw_line, 3, style));
            continue;
        }

        // List items.
        if raw_line.starts_with("- ") || raw_line.starts_with("* ") {
            lines.push(render_list_item(raw_line, style));
            continue;
        }

        // Numbered list.
        if let Some(rest) = raw_line.strip_prefix("1. ") {
            lines.push(render_numbered_item(1, rest, style));
            continue;
        }

        // Blockquote.
        if raw_line.starts_with("> ") {
            let text = raw_line.strip_prefix("> ").unwrap_or(raw_line);
            lines.push(Line::from(vec![
                Span::styled(" │ ", Style::default().fg(style.info_color)),
                Span::styled(text.to_owned(), Style::default().fg(style.info_color)),
            ]));
            continue;
        }

        // Horizontal rule.
        if raw_line.trim().starts_with("---") || raw_line.trim().starts_with("***") {
            lines.push(Line::from(Span::styled(
                "─".repeat(width.min(40)),
                Style::default().fg(style.info_color),
            )));
            continue;
        }

        // Regular text with inline formatting.
        lines.push(render_inline_formatting(raw_line, style));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::raw(String::new())));
    }
    lines
}

/// Render a heading line.
fn render_heading(line: &str, level: usize, style: &StyleConfig) -> Line<'static> {
    let text = line.trim_start_matches('#').trim();
    let (color, modifier) = match level {
        1 => (style.accent_color, Modifier::BOLD),
        2 => (style.accent_color, Modifier::BOLD),
        _ => (style.status_fg, Modifier::empty()),
    };
    Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(color).add_modifier(modifier),
    ))
}

/// Render a list item.
fn render_list_item(line: &str, style: &StyleConfig) -> Line<'static> {
    let text = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line);
    Line::from(vec![
        Span::styled("  • ", Style::default().fg(style.accent_color)),
        Span::styled(text.to_owned(), Style::default().fg(style.status_fg)),
    ])
}

/// Render a numbered list item.
fn render_numbered_item(num: usize, text: &str, style: &StyleConfig) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {num}. "),
            Style::default().fg(style.accent_color),
        ),
        Span::styled(text.to_owned(), Style::default().fg(style.status_fg)),
    ])
}

/// Render inline formatting (bold, italic, code).
fn render_inline_formatting(text: &str, style: &StyleConfig) -> Line<'static> {
    let mut spans = Vec::new();
    let mut remaining = text;
    let mut in_bold = false;
    let mut in_italic = false;

    while !remaining.is_empty() {
        // Inline code.
        if let Some(end) = find_closing(remaining, '`') {
            let code_text: String = remaining.chars().take_while(|&c| c != '`').collect();
            if !code_text.is_empty() {
                spans.push(Span::styled(
                    code_text,
                    Style::default().fg(style.code_fg).bg(style.code_bg),
                ));
            }
            remaining = &remaining[end..];
            continue;
        }

        // Bold.
        if remaining.starts_with("**") {
            if in_bold {
                in_bold = false;
                remaining = &remaining[2..];
            } else {
                in_bold = true;
                remaining = &remaining[2..];
            }
            continue;
        }

        // Italic.
        if remaining.starts_with('*') && !remaining.starts_with("**") {
            if in_italic {
                in_italic = false;
                remaining = &remaining[1..];
            } else {
                in_italic = true;
                remaining = &remaining[1..];
            }
            continue;
        }

        // Consume characters until next formatting marker.
        let plain_end = find_next_marker(remaining);
        let plain: String = remaining.chars().take(plain_end).collect();
        remaining = &remaining[plain.len()..];

        let mut s = Style::default().fg(style.status_fg);
        if in_bold {
            s = s.add_modifier(Modifier::BOLD);
        }
        if in_italic {
            s = s.add_modifier(Modifier::ITALIC);
        }
        spans.push(Span::styled(plain, s));
    }

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    Line::from(spans)
}

/// Find the closing delimiter for inline code.
fn find_closing(text: &str, delimiter: char) -> Option<usize> {
    if !text.starts_with(delimiter) {
        return None;
    }
    let after_open = &text[1..];
    let close_pos = after_open.find(delimiter)?;
    Some(close_pos + 2) // opening + content + closing
}

/// Find the index of the next inline formatting marker.
fn find_next_marker(text: &str) -> usize {
    text.char_indices()
        .find(|&(_, c)| c == '`' || c == '*')
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::StyleConfig;

    #[test]
    fn render_plain_text() {
        let style = StyleConfig::dark();
        let lines = render_markdown("hello world", 80, &style);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn render_heading() {
        let style = StyleConfig::dark();
        let lines = render_markdown("# Title", 80, &style);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn render_list_items() {
        let style = StyleConfig::dark();
        let lines = render_markdown("- item 1\n- item 2", 80, &style);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn render_code_block() {
        let style = StyleConfig::dark();
        let md = "```rust\nfn main() {}\n```";
        let lines = render_markdown(md, 80, &style);
        assert!(lines.len() >= 3); // fence + code + fence
    }

    #[test]
    fn render_blockquote() {
        let style = StyleConfig::dark();
        let lines = render_markdown("> quote text", 80, &style);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn render_inline_code() {
        let style = StyleConfig::dark();
        let lines = render_markdown("use `cargo build` to compile", 80, &style);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn render_empty_content() {
        let style = StyleConfig::dark();
        let lines = render_markdown("", 80, &style);
        assert!(!lines.is_empty());
    }

    #[test]
    fn render_horizontal_rule() {
        let style = StyleConfig::dark();
        let lines = render_markdown("---", 80, &style);
        assert_eq!(lines.len(), 1);
    }
}
