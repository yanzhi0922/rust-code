//! Simple syntax highlighting for code blocks.
//!
//! Provides token-based highlighting for common programming languages,
//! converting source code into styled [`Line`] sequences for ratatui rendering.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// A highlighted span with its style.
#[derive(Debug, Clone)]
pub struct HighlightedSpan {
    pub content: String,
    pub style: Style,
}

/// Detect the programming language from a fence info string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    Shell,
    Html,
    Css,
    Json,
    Toml,
    Yaml,
    Markdown,
    Plain,
}

impl Language {
    /// Parse from a fence info string (e.g. `"rust"`, `"python"`).
    pub fn from_fence(fence: &str) -> Self {
        match fence.trim().to_lowercase().as_str() {
            "rust" | "rs" => Self::Rust,
            "python" | "py" => Self::Python,
            "javascript" | "js" | "jsx" => Self::JavaScript,
            "typescript" | "ts" | "tsx" => Self::TypeScript,
            "go" | "golang" => Self::Go,
            "sh" | "bash" | "zsh" | "shell" => Self::Shell,
            "html" | "htm" => Self::Html,
            "css" | "scss" | "sass" => Self::Css,
            "json" => Self::Json,
            "toml" => Self::Toml,
            "yaml" | "yml" => Self::Yaml,
            "md" | "markdown" => Self::Markdown,
            _ => Self::Plain,
        }
    }
}

/// Style configuration for syntax highlighting.
#[derive(Debug, Clone)]
pub struct SyntaxColors {
    pub keyword: Color,
    pub string: Color,
    pub comment: Color,
    pub number: Color,
    pub function: Color,
    pub r#type: Color,
    pub punctuation: Color,
    pub default: Color,
}

impl SyntaxColors {
    /// Default dark-theme syntax colors.
    pub fn dark() -> Self {
        SyntaxColors {
            keyword: Color::Magenta,
            string: Color::Green,
            comment: Color::DarkGray,
            number: Color::Yellow,
            function: Color::Blue,
            r#type: Color::Cyan,
            punctuation: Color::Gray,
            default: Color::White,
        }
    }
}

/// Highlight a single line of code, returning a list of styled spans.
pub fn highlight_line(line: &str, lang: Language, colors: &SyntaxColors) -> Vec<Span<'static>> {
    match lang {
        Language::Rust => highlight_rust(line, colors),
        Language::Shell => highlight_shell(line, colors),
        Language::Json => highlight_json(line, colors),
        _ => {
            // Generic highlighting for unknown languages.
            vec![Span::styled(
                line.to_owned(),
                Style::default().fg(colors.default),
            )]
        }
    }
}

/// Highlight multiple lines of code into ratatui `Line` values.
pub fn highlight_code(code: &str, lang: Language, colors: &SyntaxColors) -> Vec<Line<'static>> {
    code.lines()
        .map(|line| Line::from(highlight_line(line, lang, colors)))
        .collect()
}

/// Check if a word is a Rust keyword.
pub fn is_rust_keyword(word: &str) -> bool {
    matches!(
        word,
        "fn" | "let"
            | "mut"
            | "if"
            | "else"
            | "match"
            | "loop"
            | "while"
            | "for"
            | "in"
            | "return"
            | "break"
            | "continue"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "pub"
            | "use"
            | "mod"
            | "crate"
            | "self"
            | "super"
            | "where"
            | "async"
            | "await"
            | "move"
            | "ref"
            | "type"
            | "const"
            | "static"
            | "unsafe"
            | "extern"
            | "dyn"
            | "as"
            | "true"
            | "false"
    )
}

fn highlight_rust(line: &str, colors: &SyntaxColors) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Line comment
        if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '/' {
            let comment: String = chars[i..].iter().collect();
            spans.push(Span::styled(
                comment,
                Style::default()
                    .fg(colors.comment)
                    .add_modifier(Modifier::ITALIC),
            ));
            break;
        }

        // String literal
        if chars[i] == '"' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    i += 1; // skip escaped char
                }
                i += 1;
            }
            if i < chars.len() {
                i += 1; // closing quote
            }
            let s: String = chars[start..i].iter().collect();
            spans.push(Span::styled(s, Style::default().fg(colors.string)));
            continue;
        }

        // Number
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_digit() || chars[i] == '_' || chars[i] == '.')
            {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            spans.push(Span::styled(s, Style::default().fg(colors.number)));
            continue;
        }

        // Identifier / keyword
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let style = if is_rust_keyword(&word) {
                Style::default()
                    .fg(colors.keyword)
                    .add_modifier(Modifier::BOLD)
            } else if i < chars.len() && chars[i] == '(' {
                Style::default().fg(colors.function)
            } else if word.starts_with(char::is_uppercase) && word.len() > 1 {
                Style::default().fg(colors.r#type)
            } else {
                Style::default().fg(colors.default)
            };
            spans.push(Span::styled(word, style));
            continue;
        }

        // Punctuation
        let ch = chars[i];
        spans.push(Span::styled(
            ch.to_string(),
            Style::default().fg(colors.punctuation),
        ));
        i += 1;
    }

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    spans
}

fn highlight_shell(line: &str, colors: &SyntaxColors) -> Vec<Span<'static>> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return vec![Span::styled(
            line.to_owned(),
            Style::default().fg(colors.comment),
        )];
    }

    // Simple approach: highlight common commands and strings.
    let mut spans = Vec::new();
    let mut in_string = false;
    let mut current = String::new();

    for ch in line.chars() {
        if ch == '"' || ch == '\'' {
            if !current.is_empty() {
                let style = if is_shell_keyword(&current) {
                    Style::default().fg(colors.keyword)
                } else {
                    Style::default().fg(colors.default)
                };
                spans.push(Span::styled(current.clone(), style));
                current.clear();
            }
            in_string = !in_string;
            current.push(ch);
        } else if in_string {
            current.push(ch);
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                let style = if is_shell_keyword(&current) {
                    Style::default().fg(colors.keyword)
                } else {
                    Style::default().fg(colors.default)
                };
                spans.push(Span::styled(current.clone(), style));
                current.clear();
            }
            spans.push(Span::raw(ch.to_string()));
        } else {
            current.push(ch);
        }
    }

    if !current.is_empty() {
        let style = if in_string {
            Style::default().fg(colors.string)
        } else if is_shell_keyword(&current) {
            Style::default().fg(colors.keyword)
        } else {
            Style::default().fg(colors.default)
        };
        spans.push(Span::styled(current, style));
    }

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    spans
}

fn is_shell_keyword(word: &str) -> bool {
    matches!(
        word,
        "if" | "then"
            | "else"
            | "fi"
            | "for"
            | "do"
            | "done"
            | "while"
            | "case"
            | "esac"
            | "function"
            | "return"
            | "export"
            | "source"
            | "cd"
            | "echo"
            | "exit"
            | "set"
            | "local"
            | "readonly"
            | "sudo"
            | "apt"
            | "yum"
            | "cargo"
            | "rustup"
    )
}

fn highlight_json(line: &str, colors: &SyntaxColors) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // String
        if chars[i] == '"' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            // Key if followed by ':'
            let is_key = i < chars.len() && chars[i..].contains(&':');
            let style = if is_key {
                Style::default().fg(colors.function)
            } else {
                Style::default().fg(colors.string)
            };
            spans.push(Span::styled(s, style));
            continue;
        }

        // Number
        if chars[i].is_ascii_digit()
            || (chars[i] == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
            let start = i;
            if chars[i] == '-' {
                i += 1;
            }
            while i < chars.len()
                && (chars[i].is_ascii_digit()
                    || chars[i] == '.'
                    || chars[i] == 'e'
                    || chars[i] == 'E'
                    || chars[i] == '+'
                    || chars[i] == '-')
            {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            spans.push(Span::styled(s, Style::default().fg(colors.number)));
            continue;
        }

        // Keywords: true, false, null
        if chars[i].is_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i].is_alphabetic() {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let style = match word.as_str() {
                "true" | "false" | "null" => Style::default().fg(colors.keyword),
                _ => Style::default().fg(colors.default),
            };
            spans.push(Span::styled(word, style));
            continue;
        }

        // Punctuation
        spans.push(Span::styled(
            chars[i].to_string(),
            Style::default().fg(colors.punctuation),
        ));
        i += 1;
    }

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_from_fence_rust() {
        assert_eq!(Language::from_fence("rust"), Language::Rust);
        assert_eq!(Language::from_fence("rs"), Language::Rust);
        assert_eq!(Language::from_fence("RUST"), Language::Rust);
    }

    #[test]
    fn language_from_fence_unknown() {
        assert_eq!(Language::from_fence("brainfuck"), Language::Plain);
    }

    #[test]
    fn highlight_rust_keyword() {
        let colors = SyntaxColors::dark();
        let spans = highlight_rust("fn main() {", &colors);
        assert!(!spans.is_empty());
        // First span should be "fn" in keyword color.
        assert_eq!(spans[0].content, "fn");
    }

    #[test]
    fn highlight_rust_comment() {
        let colors = SyntaxColors::dark();
        let spans = highlight_rust("// hello", &colors);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "// hello");
    }

    #[test]
    fn highlight_rust_string() {
        let colors = SyntaxColors::dark();
        let spans = highlight_rust(r#"let s = "hello";"#, &colors);
        assert!(spans.iter().any(|s| s.content.contains("hello")));
    }

    #[test]
    fn highlight_shell_comment() {
        let colors = SyntaxColors::dark();
        let spans = highlight_shell("# this is a comment", &colors);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "# this is a comment");
    }

    #[test]
    fn highlight_json_string() {
        let colors = SyntaxColors::dark();
        let spans = highlight_json(r#""key": "value""#, &colors);
        assert!(spans.len() >= 2);
    }

    #[test]
    fn highlight_code_multiline() {
        let colors = SyntaxColors::dark();
        let lines = highlight_code("fn foo() {\n    // comment\n}", Language::Rust, &colors);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn is_rust_keyword_true() {
        assert!(is_rust_keyword("fn"));
        assert!(is_rust_keyword("let"));
        assert!(is_rust_keyword("async"));
    }

    #[test]
    fn is_rust_keyword_false() {
        assert!(!is_rust_keyword("foo"));
        assert!(!is_rust_keyword("bar"));
    }

    #[test]
    fn highlight_plain_falls_through() {
        let colors = SyntaxColors::dark();
        let spans = highlight_line("hello world", Language::Plain, &colors);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "hello world");
    }
}
