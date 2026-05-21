//! Conversation export to various formats.
//!
//! Provides functionality for exporting conversation entries to JSON,
//! Markdown, and HTML formats for sharing, archiving, or documentation.

use claude_core::ConversationEntry;
use std::io::Write;
use std::path::Path;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Supported export formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportFormat {
    /// JSON format (structured, machine-readable).
    Json,
    /// Markdown format (human-readable, suitable for documentation).
    Markdown,
    /// HTML format (styled, suitable for viewing in browsers).
    Html,
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportFormat::Json => write!(f, "json"),
            ExportFormat::Markdown => write!(f, "markdown"),
            ExportFormat::Html => write!(f, "html"),
        }
    }
}

// ---------------------------------------------------------------------------
// Export functions
// ---------------------------------------------------------------------------

/// Export conversation entries to the specified format.
pub fn export_conversation(
    messages: &[ConversationEntry],
    format: ExportFormat,
) -> anyhow::Result<String> {
    match format {
        ExportFormat::Json => export_json(messages),
        ExportFormat::Markdown => export_markdown(messages),
        ExportFormat::Html => export_html(messages),
    }
}

/// Write exported conversation to a file.
pub fn write_export_to_file(
    messages: &[ConversationEntry],
    format: ExportFormat,
    path: &Path,
) -> anyhow::Result<()> {
    let content = export_conversation(messages, format)?;
    let mut file = std::fs::File::create(path)?;
    file.write_all(content.as_bytes())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Format implementations
// ---------------------------------------------------------------------------

/// Export as pretty-printed JSON.
fn export_json(messages: &[ConversationEntry]) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(messages)?)
}

/// Export as Markdown.
fn export_markdown(messages: &[ConversationEntry]) -> anyhow::Result<String> {
    let mut output = String::new();
    output.push_str("# Conversation Export");
    output.push(line_sep());
    output.push(line_sep());

    for entry in messages {
        let role_label = format_role(&entry.role);
        output.push_str(&format!("## {role_label}"));
        output.push(line_sep());
        output.push(line_sep());

        if !entry.text.is_empty() {
            output.push_str(&entry.text);
            output.push(line_sep());
            output.push(line_sep());
        }

        for tc in &entry.tool_calls {
            output.push_str(&format!("**Tool: {}** (`{}`)", tc.name, tc.id));
            output.push(line_sep());
            let args = serde_json::to_string_pretty(&tc.input)?;
            output.push_str(&format!(
                "```json{ls}{args}{ls}```{ls}{ls}",
                ls = line_sep()
            ));
        }

        if let Some(ref tc_id) = entry.tool_call_id {
            output.push_str(&format!("*Tool call ID: {tc_id}*"));
            output.push(line_sep());
            output.push(line_sep());
        }

        if entry.is_error {
            output.push_str("*Warning: Error*");
            output.push(line_sep());
            output.push(line_sep());
        }
    }

    output.push_str("---");
    output.push(line_sep());
    output.push_str(&format!("*Exported {} messages*", messages.len()));
    output.push(line_sep());

    Ok(output)
}

/// Export as HTML.
fn export_html(messages: &[ConversationEntry]) -> anyhow::Result<String> {
    let mut output = String::new();
    let q = html_quote();

    output.push_str("<!DOCTYPE html>");
    output.push(line_sep());
    output.push_str(&format!("<html lang={q}en{q}>"));
    output.push(line_sep());
    output.push_str("<head>");
    output.push(line_sep());
    output.push_str(&format!("<meta charset={q}utf-8{q}>"));
    output.push(line_sep());
    output.push_str(&format!(
        "<meta name={q}viewport{q} content={q}width=device-width, initial-scale=1{q}>"
    ));
    output.push(line_sep());
    output.push_str("<title>Conversation Export</title>");
    output.push(line_sep());
    output.push_str("<style>");
    output.push(line_sep());
    output.push_str("body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; max-width: 800px; margin: 0 auto; padding: 20px; }");
    output.push(line_sep());
    output.push_str(".message { margin: 16px 0; padding: 12px; border-radius: 8px; }");
    output.push(line_sep());
    output.push_str(".system { background: #f0f0f0; }");
    output.push(line_sep());
    output.push_str(".user { background: #e3f2fd; }");
    output.push(line_sep());
    output.push_str(".assistant { background: #e8f5e9; }");
    output.push(line_sep());
    output.push_str(".tool { background: #fff3e0; }");
    output.push(line_sep());
    output.push_str(".role { font-weight: bold; margin-bottom: 8px; }");
    output.push(line_sep());
    output.push_str(".error { color: #d32f2f; font-style: italic; }");
    output.push(line_sep());
    output.push_str("pre { background: #263238; color: #eeffff; padding: 12px; border-radius: 4px; overflow-x: auto; }");
    output.push(line_sep());
    output.push_str("code { font-family: 'Fira Code', monospace; }");
    output.push(line_sep());
    output.push_str("</style>");
    output.push(line_sep());
    output.push_str("</head>");
    output.push(line_sep());
    output.push_str("<body>");
    output.push(line_sep());
    output.push_str("<h1>Conversation Export</h1>");
    output.push(line_sep());

    for entry in messages {
        let role_class = format_role_class(&entry.role);
        let role_label = format_role(&entry.role);

        output.push_str(&format!("<div class={q}message {role_class}{q}>"));
        output.push(line_sep());
        output.push_str(&format!("<div class={q}role{q}>{role_label}</div>"));
        output.push(line_sep());

        if !entry.text.is_empty() {
            let escaped = html_escape(&entry.text);
            output.push_str(&format!("<p>{escaped}</p>"));
            output.push(line_sep());
        }

        for tc in &entry.tool_calls {
            let args_json = serde_json::to_string_pretty(&tc.input)?;
            output.push_str(&format!(
                "<div><strong>Tool: {}</strong> <code>{}</code></div>",
                html_escape(&tc.name),
                html_escape(&tc.id)
            ));
            output.push(line_sep());
            output.push_str(&format!(
                "<pre><code>{}</code></pre>",
                html_escape(&args_json)
            ));
            output.push(line_sep());
        }

        if entry.is_error {
            output.push_str("<p class={q}error{q}>Warning: Error</p>");
            output.push(line_sep());
        }

        output.push_str("</div>");
        output.push(line_sep());
    }

    output.push_str(&format!(
        "<footer><p><em>Exported {} messages</em></p></footer>",
        messages.len()
    ));
    output.push(line_sep());
    output.push_str("</body>");
    output.push(line_sep());
    output.push_str("</html>");
    output.push(line_sep());

    Ok(output)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Line separator character.
fn line_sep() -> char {
    '\n'
}

/// Format a conversation role for display.
fn format_role(role: &claude_core::ConversationRole) -> &'static str {
    match role {
        claude_core::ConversationRole::System => "System",
        claude_core::ConversationRole::User => "User",
        claude_core::ConversationRole::Assistant => "Assistant",
        claude_core::ConversationRole::Tool => "Tool",
    }
}

/// Get CSS class name for a role.
fn format_role_class(role: &claude_core::ConversationRole) -> &'static str {
    match role {
        claude_core::ConversationRole::System => "system",
        claude_core::ConversationRole::User => "user",
        claude_core::ConversationRole::Assistant => "assistant",
        claude_core::ConversationRole::Tool => "tool",
    }
}

/// Return a double-quote character for HTML attribute construction.
fn html_quote() -> char {
    '"'
}

/// Build the HTML entity for ampersand.
fn html_amp() -> String {
    let mut s = String::with_capacity(5);
    s.push('&');
    s.push_str("amp;");
    s
}

/// Build the HTML entity for less-than.
fn html_lt() -> String {
    let mut s = String::with_capacity(4);
    s.push('&');
    s.push_str("lt;");
    s
}

/// Build the HTML entity for greater-than.
fn html_gt() -> String {
    let mut s = String::with_capacity(4);
    s.push('&');
    s.push_str("gt;");
    s
}

/// Build the HTML entity for double-quote.
fn html_quot() -> String {
    let mut s = String::with_capacity(6);
    s.push('&');
    s.push_str("quot;");
    s
}

/// Escape text for safe inclusion in HTML.
fn html_escape(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => result.push_str(&html_amp()),
            '<' => result.push_str(&html_lt()),
            '>' => result.push_str(&html_gt()),
            '"' => result.push_str(&html_quot()),
            '\n' => {
                result.push_str("<br>");
                result.push('\n');
            }
            _ => result.push(ch),
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::{ConversationRole, ToolCall};
    use serde_json::Value;
    use uuid::Uuid;

    fn make_entry(role: ConversationRole, text: &str) -> ConversationEntry {
        ConversationEntry {
            uuid: Uuid::new_v4(),
            role,
            text: text.to_string(),
            history_text: None,
            content_blocks: vec![],
            tool_calls: vec![],
            attachments: vec![],
            tool_call_id: None,
            name: None,
            is_error: false,
        }
    }

    fn make_error_entry(role: ConversationRole, text: &str) -> ConversationEntry {
        ConversationEntry {
            uuid: Uuid::new_v4(),
            role,
            text: text.to_string(),
            history_text: None,
            content_blocks: vec![],
            tool_calls: vec![],
            attachments: vec![],
            tool_call_id: None,
            name: None,
            is_error: true,
        }
    }

    fn make_tool_entry(text: &str, tool_call_id: &str) -> ConversationEntry {
        ConversationEntry {
            uuid: Uuid::new_v4(),
            role: ConversationRole::Tool,
            text: text.to_string(),
            history_text: None,
            content_blocks: vec![],
            tool_calls: vec![],
            attachments: vec![],
            tool_call_id: Some(tool_call_id.to_string()),
            name: Some("read_file".to_string()),
            is_error: false,
        }
    }

    fn make_assistant_with_tools(text: &str) -> ConversationEntry {
        ConversationEntry {
            uuid: Uuid::new_v4(),
            role: ConversationRole::Assistant,
            text: text.to_string(),
            history_text: None,
            content_blocks: vec![],
            tool_calls: vec![ToolCall {
                id: "tc-1".to_string(),
                name: "read_file".to_string(),
                input: Value::Object(serde_json::Map::new()),
            }],
            attachments: vec![],
            tool_call_id: None,
            name: None,
            is_error: false,
        }
    }

    #[test]
    fn export_format_display() {
        assert_eq!(ExportFormat::Json.to_string(), "json");
        assert_eq!(ExportFormat::Markdown.to_string(), "markdown");
        assert_eq!(ExportFormat::Html.to_string(), "html");
    }

    #[test]
    fn export_json_empty() {
        let result = export_conversation(&[], ExportFormat::Json);
        assert!(result.is_ok());
        assert_eq!(
            result.expect("empty json export should succeed").trim(),
            "[]"
        );
    }

    #[test]
    fn export_json_with_messages() {
        let messages = vec![
            make_entry(ConversationRole::User, "Hello"),
            make_entry(ConversationRole::Assistant, "Hi there!"),
        ];
        let result = export_conversation(&messages, ExportFormat::Json);
        assert!(result.is_ok());
        let json = result.expect("json export with messages should succeed");
        assert!(json.contains("Hello"));
        assert!(json.contains("Hi there!"));
    }

    #[test]
    fn export_markdown_empty() {
        let result = export_conversation(&[], ExportFormat::Markdown);
        assert!(result.is_ok());
        let md = result.expect("empty markdown export should succeed");
        assert!(md.contains("Conversation Export"));
        assert!(md.contains("Exported 0 messages"));
    }

    #[test]
    fn export_markdown_with_messages() {
        let messages = vec![
            make_entry(ConversationRole::User, "What is Rust?"),
            make_entry(
                ConversationRole::Assistant,
                "Rust is a systems programming language.",
            ),
        ];
        let result = export_conversation(&messages, ExportFormat::Markdown);
        assert!(result.is_ok());
        let md = result.expect("markdown export with messages should succeed");
        assert!(md.contains("User"));
        assert!(md.contains("Assistant"));
        assert!(md.contains("What is Rust?"));
    }

    #[test]
    fn export_markdown_with_tool_calls() {
        let messages = vec![make_assistant_with_tools("Let me read that file.")];
        let result = export_conversation(&messages, ExportFormat::Markdown);
        assert!(result.is_ok());
        let md = result.expect("markdown export with tool calls should succeed");
        assert!(md.contains("Tool: read_file"));
    }

    #[test]
    fn export_markdown_with_error() {
        let messages = vec![make_error_entry(ConversationRole::Tool, "file not found")];
        let result = export_conversation(&messages, ExportFormat::Markdown);
        assert!(result.is_ok());
        let md = result.expect("markdown export with error should succeed");
        assert!(md.contains("Error"));
    }

    #[test]
    fn export_html_empty() {
        let result = export_conversation(&[], ExportFormat::Html);
        assert!(result.is_ok());
        let html = result.expect("empty html export should succeed");
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Conversation Export"));
    }

    #[test]
    fn export_html_with_messages() {
        let messages = vec![
            make_entry(ConversationRole::User, "Hello <world>"),
            make_entry(ConversationRole::Assistant, "Hi & welcome!"),
        ];
        let result = export_conversation(&messages, ExportFormat::Html);
        assert!(result.is_ok());
        let html = result.expect("html export with messages should succeed");
        // Check HTML escaping worked
        let expected_lt_world = format!("{}lt;world{}gt;", '&', '&');
        let expected_amp_welcome = format!("{}amp; welcome!", '&');
        assert!(html.contains(&expected_lt_world));
        assert!(html.contains(&expected_amp_welcome));
        let q = html_quote();
        let expected_class = format!("class={q}message user{q}");
        assert!(html.contains(&expected_class));
    }

    #[test]
    fn export_html_escapes_special_chars() {
        let messages = vec![make_entry(
            ConversationRole::User,
            "<script>alert('xss')</script>",
        )];
        let result = export_conversation(&messages, ExportFormat::Html);
        assert!(result.is_ok());
        let html = result.expect("html export with special chars should succeed");
        assert!(!html.contains("<script>"));
        let expected = format!("{}lt;script{}gt;", '&', '&');
        assert!(html.contains(&expected));
    }

    #[test]
    fn write_export_to_file_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("export.json");
        let messages = vec![make_entry(ConversationRole::User, "test")];

        write_export_to_file(&messages, ExportFormat::Json, &path).expect("write");
        assert!(path.exists());

        let content = std::fs::read_to_string(&path).expect("read");
        assert!(content.contains("test"));
    }

    #[test]
    fn write_export_to_file_markdown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("export.md");
        let messages = vec![make_entry(ConversationRole::User, "markdown test")];

        write_export_to_file(&messages, ExportFormat::Markdown, &path).expect("write");
        assert!(path.exists());
    }

    #[test]
    fn write_export_to_file_html() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("export.html");
        let messages = vec![make_entry(ConversationRole::User, "html test")];

        write_export_to_file(&messages, ExportFormat::Html, &path).expect("write");
        assert!(path.exists());
    }

    #[test]
    fn export_json_is_valid_json() {
        let messages = vec![
            make_entry(ConversationRole::System, "system prompt"),
            make_entry(ConversationRole::User, "user message"),
            make_entry(ConversationRole::Assistant, "assistant response"),
            make_tool_entry("tool output", "tc-1"),
        ];
        let result = export_conversation(&messages, ExportFormat::Json).expect("export");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid JSON");
        assert!(parsed.is_array());
        assert_eq!(
            parsed
                .as_array()
                .expect("parsed JSON should be an array")
                .len(),
            4
        );
    }

    #[test]
    fn export_markdown_with_tool_call_id() {
        let messages = vec![make_tool_entry("file contents here", "tc-abc")];
        let result = export_conversation(&messages, ExportFormat::Markdown);
        assert!(result.is_ok());
        let md = result.expect("markdown export with tool call id should succeed");
        assert!(md.contains("tc-abc"));
    }

    #[test]
    fn export_all_roles() {
        let messages = vec![
            make_entry(ConversationRole::System, "sys"),
            make_entry(ConversationRole::User, "usr"),
            make_entry(ConversationRole::Assistant, "ast"),
            make_tool_entry("tool", "tc-1"),
        ];

        for fmt in [
            ExportFormat::Json,
            ExportFormat::Markdown,
            ExportFormat::Html,
        ] {
            let result = export_conversation(&messages, fmt);
            assert!(result.is_ok(), "Failed for format {fmt}");
            let content = result.expect("export should succeed for all formats");
            assert!(!content.is_empty(), "Empty output for format {fmt}");
        }
    }
}
