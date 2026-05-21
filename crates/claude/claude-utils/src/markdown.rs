//! Markdown rendering and processing utilities.
//!
//! Provides functions for rendering Markdown to terminal output,
//! stripping Markdown formatting, and extracting code blocks.

// ---------------------------------------------------------------------------
// Strip markdown
// ---------------------------------------------------------------------------

/// Strip Markdown formatting from text, returning plain text.
///
/// Removes:
/// - Headers (`#`, `##`, etc.)
/// - Bold (`**text**` or `__text__`)
/// - Italic (`*text*` or `_text_`)
/// - Links (`[text](url)`)
/// - Images (`![alt](url)`)
/// - Inline code (`` `code` ``)
/// - Horizontal rules (`---`, `***`)
///
/// # Arguments
///
/// * `markdown` — The Markdown text to strip.
///
/// # Returns
///
/// Plain text with Markdown formatting removed.
#[must_use]
pub fn strip_markdown(markdown: &str) -> String {
    let mut result = markdown.to_string();

    // Remove code blocks first (preserve content).
    result = strip_code_blocks(&result).0;

    // Remove images: ![alt](url) -> alt
    result = replace_balanced(&result, "![", "]", "(", ")");

    // Remove links: [text](url) -> text
    result = replace_links(&result);

    // Remove bold: **text** or __text__ -> text
    result = replace_delimited(&result, "**");
    result = replace_delimited(&result, "__");

    // Remove italic: *text* or _text_ -> text
    result = replace_delimited(&result, "*");
    result = replace_delimited(&result, "_");

    // Remove inline code: `code` -> code
    result = replace_delimited(&result, "`");

    // Remove heading markers.
    let lines: Vec<String> = result
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let hash_count = trimmed.chars().take_while(|c| *c == '#').count();
            if hash_count > 0 && hash_count <= 6 {
                trimmed[hash_count..].trim_start().to_string()
            } else {
                line.to_string()
            }
        })
        .collect();

    lines.join("\n")
}

/// Replace delimited patterns like **text** -> text.
fn replace_delimited(text: &str, delimiter: &str) -> String {
    let mut result = text.to_string();
    while let Some(start) = result.find(delimiter) {
        let after_start = start + delimiter.len();
        if let Some(end) = result[after_start..].find(delimiter) {
            let end_pos = after_start + end;
            let inner = result[after_start..end_pos].to_string();
            result = format!(
                "{}{}{}",
                &result[..start],
                inner,
                &result[end_pos + delimiter.len()..]
            );
        } else {
            break;
        }
    }
    result
}

/// Replace link patterns [text](url) -> text.
fn replace_links(text: &str) -> String {
    let mut result = text.to_string();
    while let Some(bracket_start) = result.find('[') {
        if let Some(bracket_end) = result[bracket_start..].find(']') {
            let text_start = bracket_start + 1;
            let text_end = bracket_start + bracket_end;
            let link_text = result[text_start..text_end].to_string();
            let after_bracket = text_end + 1;
            if after_bracket < result.len()
                && result.as_bytes()[after_bracket] == b'('
                && let Some(paren_end) = result[after_bracket..].find(')')
            {
                let full_end = after_bracket + paren_end + 1;
                result = format!(
                    "{}{}{}",
                    &result[..bracket_start],
                    link_text,
                    &result[full_end..]
                );
                continue;
            }
            // Not a link, just a bracket.
            break;
        } else {
            break;
        }
    }
    result
}

/// Replace image patterns ![alt](url) -> alt.
fn replace_balanced(text: &str, prefix: &str, _mid: &str, _open: &str, _close: &str) -> String {
    let mut result = text.to_string();
    while let Some(start) = result.find(prefix) {
        let after_prefix = start + prefix.len();
        if let Some(bracket_end) = result[after_prefix..].find(']') {
            let alt_text = result[after_prefix..after_prefix + bracket_end].to_string();
            let after_bracket = after_prefix + bracket_end + 1;
            if after_bracket < result.len()
                && result.as_bytes()[after_bracket] == b'('
                && let Some(paren_end) = result[after_bracket..].find(')')
            {
                let full_end = after_bracket + paren_end + 1;
                result = format!("{}{}{}", &result[..start], alt_text, &result[full_end..]);
                continue;
            }
            break;
        } else {
            break;
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Code block extraction
// ---------------------------------------------------------------------------

/// A code block extracted from Markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock {
    /// The language tag (e.g. `"rust"`, `"python"`), if specified.
    pub language: Option<String>,
    /// The code content.
    pub code: String,
}

/// Extract all fenced code blocks from Markdown text.
///
/// Supports both triple-backtick (```) and triple-tilde (~~~) fences.
///
/// # Arguments
///
/// * `markdown` — The Markdown text to parse.
///
/// # Returns
///
/// A vector of extracted code blocks.
#[must_use]
pub fn extract_code_blocks(markdown: &str) -> Vec<CodeBlock> {
    let mut blocks = Vec::new();
    let mut in_code_block = false;
    let mut current_language = None;
    let mut current_lines: Vec<String> = Vec::new();

    for line in markdown.lines() {
        let trimmed = line.trim();

        if !in_code_block {
            // Look for code block start.
            if let Some(rest) = trimmed.strip_prefix("```") {
                in_code_block = true;
                let lang = rest.trim();
                current_language = if lang.is_empty() {
                    None
                } else {
                    Some(lang.to_string())
                };
                current_lines.clear();
            } else if let Some(rest) = trimmed.strip_prefix("~~~") {
                in_code_block = true;
                let lang = rest.trim();
                current_language = if lang.is_empty() {
                    None
                } else {
                    Some(lang.to_string())
                };
                current_lines.clear();
            }
        } else {
            // Look for code block end.
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_code_block = false;
                blocks.push(CodeBlock {
                    language: current_language.take(),
                    code: current_lines.join("\n"),
                });
                current_lines.clear();
            } else {
                current_lines.push(line.to_string());
            }
        }
    }

    // Handle unclosed code block.
    if in_code_block && !current_lines.is_empty() {
        blocks.push(CodeBlock {
            language: current_language,
            code: current_lines.join("\n"),
        });
    }

    blocks
}

/// Strip code blocks from markdown, returning the text without code blocks.
///
/// # Arguments
///
/// * `markdown` — The Markdown text.
///
/// # Returns
///
/// A tuple of (text without code blocks, removed code blocks).
#[must_use]
pub fn strip_code_blocks(markdown: &str) -> (String, Vec<CodeBlock>) {
    let blocks = extract_code_blocks(markdown);

    let mut result = String::new();
    let mut in_code_block = false;

    for line in markdown.lines() {
        let trimmed = line.trim();
        if !in_code_block && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            in_code_block = true;
            continue;
        }
        if in_code_block && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            in_code_block = false;
            continue;
        }
        if !in_code_block {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(line);
        }
    }

    (result, blocks)
}

// ---------------------------------------------------------------------------
// Terminal rendering
// ---------------------------------------------------------------------------

/// Render Markdown to a terminal-friendly format.
///
/// This is a simplified renderer that produces readable output for
/// terminal display. In a full implementation, this would use ANSI
/// color codes and proper text wrapping.
///
/// # Arguments
///
/// * `markdown` — The Markdown text to render.
///
/// # Returns
///
/// Terminal-rendered text.
#[must_use]
pub fn render_markdown_to_terminal(markdown: &str) -> String {
    let mut output = String::new();

    for line in markdown.lines() {
        let trimmed = line.trim_start();

        // Headers.
        let hash_count = trimmed.chars().take_while(|c| *c == '#').count();
        if hash_count > 0 && hash_count <= 6 {
            let header_text = trimmed[hash_count..].trim_start();
            let separator = "═".repeat(header_text.len().max(1));
            match hash_count {
                1 => {
                    output.push_str(&separator);
                    output.push('\n');
                    output.push_str(header_text);
                    output.push('\n');
                    output.push_str(&separator);
                }
                2 => {
                    output.push_str(header_text);
                    output.push('\n');
                    output.push_str(&separator);
                }
                _ => {
                    output.push_str(header_text);
                }
            }
            output.push('\n');
            continue;
        }

        // Horizontal rules.
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            output.push_str(&"─".repeat(40));
            output.push('\n');
            continue;
        }

        // Code block fences.
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let lang = trimmed
                .trim_start_matches('`')
                .trim_start_matches('~')
                .trim();
            if !lang.is_empty() {
                output.push_str(&format!("┌─ {lang} "));
                output.push_str(&"─".repeat(40usize.saturating_sub(lang.len() + 4)));
            } else {
                output.push('┌');
                output.push_str(&"─".repeat(40));
            }
            output.push('\n');
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    output
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- extract_code_blocks ---

    #[test]
    fn extract_single_code_block() {
        let md = "Some text\n```rust\nfn main() {}\n```\nMore text";
        let blocks = extract_code_blocks(md);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].language.as_ref().expect("language"), "rust");
        assert_eq!(blocks[0].code, "fn main() {}");
    }

    #[test]
    fn extract_code_block_no_language() {
        let md = "```\nhello\n```";
        let blocks = extract_code_blocks(md);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].language.is_none());
        assert_eq!(blocks[0].code, "hello");
    }

    #[test]
    fn extract_multiple_code_blocks() {
        let md = "```rust\nfn a() {}\n```\nText\n```python\ndef b():\n    pass\n```";
        let blocks = extract_code_blocks(md);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].language.as_ref().expect("lang"), "rust");
        assert_eq!(blocks[1].language.as_ref().expect("lang"), "python");
    }

    #[test]
    fn extract_tilde_code_blocks() {
        let md = "~~~javascript\nconsole.log(1)\n~~~";
        let blocks = extract_code_blocks(md);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].language.as_ref().expect("lang"), "javascript");
    }

    #[test]
    fn extract_no_code_blocks() {
        let md = "Just some text\nNo code here";
        let blocks = extract_code_blocks(md);
        assert!(blocks.is_empty());
    }

    #[test]
    fn extract_empty_code_block() {
        let md = "```\n```";
        let blocks = extract_code_blocks(md);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].code.is_empty());
    }

    #[test]
    fn extract_unclosed_code_block() {
        let md = "```python\nprint(1)";
        let blocks = extract_code_blocks(md);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].code, "print(1)");
    }

    #[test]
    fn extract_multiline_code() {
        let md = "```rust\nfn main() {\n    println!(\"hello\");\n}\n```";
        let blocks = extract_code_blocks(md);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].code.contains("fn main()"));
        assert!(blocks[0].code.contains("println"));
    }

    // --- strip_code_blocks ---

    #[test]
    fn strip_code_blocks_removes_code() {
        let md = "Before\n```rust\ncode\n```\nAfter";
        let (text, blocks) = strip_code_blocks(md);
        assert_eq!(blocks.len(), 1);
        assert!(text.contains("Before"));
        assert!(text.contains("After"));
        assert!(!text.contains("code"));
    }

    #[test]
    fn strip_code_blocks_no_code() {
        let md = "Just text\nMore text";
        let (text, blocks) = strip_code_blocks(md);
        assert!(blocks.is_empty());
        assert!(text.contains("Just text"));
    }

    // --- render_markdown_to_terminal ---

    #[test]
    fn render_h1() {
        let md = "# Title";
        let rendered = render_markdown_to_terminal(md);
        assert!(rendered.contains("Title"));
        assert!(rendered.contains('═'));
    }

    #[test]
    fn render_h2() {
        let md = "## Section";
        let rendered = render_markdown_to_terminal(md);
        assert!(rendered.contains("Section"));
        assert!(rendered.contains('═'));
    }

    #[test]
    fn render_h3() {
        let md = "### Subsection";
        let rendered = render_markdown_to_terminal(md);
        assert!(rendered.contains("Subsection"));
    }

    #[test]
    fn render_horizontal_rule() {
        let md = "Above\n---\nBelow";
        let rendered = render_markdown_to_terminal(md);
        assert!(rendered.contains('─'));
        assert!(rendered.contains("Above"));
        assert!(rendered.contains("Below"));
    }

    #[test]
    fn render_code_fence() {
        let md = "```rust\nfn main() {}\n```";
        let rendered = render_markdown_to_terminal(md);
        assert!(rendered.contains("rust"));
        assert!(rendered.contains('┌'));
    }

    #[test]
    fn render_plain_text() {
        let md = "Hello world";
        let rendered = render_markdown_to_terminal(md);
        assert_eq!(rendered.trim(), "Hello world");
    }

    #[test]
    fn render_empty() {
        let rendered = render_markdown_to_terminal("");
        assert!(rendered.is_empty());
    }

    // --- strip_markdown ---

    #[test]
    fn strip_markdown_headers() {
        let md = "# Title\n## Subtitle\nText";
        let stripped = strip_markdown(md);
        assert!(stripped.contains("Title"));
        assert!(stripped.contains("Subtitle"));
        assert!(!stripped.contains('#'));
    }

    #[test]
    fn strip_markdown_inline_code() {
        let md = "Use `cargo test` to run";
        let stripped = strip_markdown(md);
        assert!(stripped.contains("cargo test"));
        assert!(!stripped.contains('`'));
    }

    #[test]
    fn strip_markdown_bold() {
        let md = "This is **bold** text";
        let stripped = strip_markdown(md);
        assert!(stripped.contains("bold"));
        assert!(!stripped.contains("**"));
    }

    #[test]
    fn strip_markdown_italic() {
        let md = "This is *italic* text";
        let stripped = strip_markdown(md);
        assert!(stripped.contains("italic"));
        assert!(!stripped.contains('*'));
    }

    #[test]
    fn strip_markdown_link() {
        let md = "Click [here](https://example.com) for more";
        let stripped = strip_markdown(md);
        assert!(stripped.contains("here"));
        assert!(!stripped.contains("https://"));
    }

    #[test]
    fn strip_markdown_image() {
        let md = "![alt text](image.png)";
        let stripped = strip_markdown(md);
        assert!(stripped.contains("alt text"));
        assert!(!stripped.contains("image.png"));
    }

    #[test]
    fn strip_markdown_plain_text() {
        let md = "Hello world";
        assert_eq!(strip_markdown(md), "Hello world");
    }

    // --- CodeBlock ---

    #[test]
    fn code_block_fields() {
        let block = CodeBlock {
            language: Some("rust".to_string()),
            code: "fn main() {}".to_string(),
        };
        assert_eq!(block.language.as_ref().expect("lang"), "rust");
        assert_eq!(block.code, "fn main() {}");
    }
}
