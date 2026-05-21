use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::tool_result_storage::ensure_tool_results_dir;

pub const DEFAULT_MAX_MCP_OUTPUT_TOKENS: usize = 25_000;
pub const BYTES_PER_TOKEN_ESTIMATE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpResultFormat {
    ToolResult,
    StructuredContent,
    ContentArray,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedBinaryContent {
    pub filepath: PathBuf,
    pub size: usize,
    pub extension: String,
}

#[must_use]
pub fn max_mcp_output_tokens() -> usize {
    parse_max_mcp_output_tokens(std::env::var("MAX_MCP_OUTPUT_TOKENS").ok().as_deref())
        .unwrap_or(DEFAULT_MAX_MCP_OUTPUT_TOKENS)
}

#[must_use]
pub fn max_mcp_output_chars() -> usize {
    max_mcp_output_tokens() * BYTES_PER_TOKEN_ESTIMATE
}

#[must_use]
fn parse_max_mcp_output_tokens(value: Option<&str>) -> Option<usize> {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

#[must_use]
pub fn get_format_description(result_type: McpResultFormat, schema: Option<&str>) -> String {
    match result_type {
        McpResultFormat::ToolResult => "Plain text".to_owned(),
        McpResultFormat::StructuredContent => schema
            .map(|value| format!("JSON with schema: {value}"))
            .unwrap_or_else(|| "JSON".to_owned()),
        McpResultFormat::ContentArray => schema
            .map(|value| format!("JSON array with schema: {value}"))
            .unwrap_or_else(|| "JSON array".to_owned()),
    }
}

#[must_use]
pub fn get_large_output_instructions(
    raw_output_path: &Path,
    content_length: usize,
    format_description: &str,
    max_read_length: Option<usize>,
) -> String {
    let mut instructions = format!(
        "Error: result ({} characters) exceeds maximum allowed tokens. Output has been saved to {}.\n\
Format: {}\n\
Use offset and limit parameters to read specific portions of the file, search within it for specific content, and jq to make structured queries.\n\
REQUIREMENTS FOR SUMMARIZATION/ANALYSIS/REVIEW:\n\
- You MUST read the content from the file at {} in sequential chunks until 100% of the content has been read.\n",
        content_length,
        raw_output_path.display(),
        format_description,
        raw_output_path.display(),
    );

    if let Some(max_read_length) = max_read_length {
        instructions.push_str(&format!(
            "- If you receive truncation warnings when reading the file (\"[N lines truncated]\"), reduce the chunk size until you have read 100% of the content without truncation ***DO NOT PROCEED UNTIL YOU HAVE DONE THIS***. Bash output is limited to {} chars.\n",
            max_read_length
        ));
    } else {
        instructions.push_str(
            "- If you receive truncation warnings when reading the file, reduce the chunk size until you have read 100% of the content without truncation.\n",
        );
    }

    instructions.push_str(
        "- Before producing ANY summary or analysis, you MUST explicitly describe what portion of the content you have read. ***If you did not read the entire content, you MUST explicitly state this.***\n",
    );
    instructions
}

#[must_use]
pub fn extension_for_mime_type(mime_type: Option<&str>) -> &'static str {
    let Some(mime_type) = mime_type else {
        return "bin";
    };
    let normalized = mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match normalized.as_str() {
        "application/pdf" => "pdf",
        "application/json" => "json",
        "text/csv" => "csv",
        "text/plain" => "txt",
        "text/html" => "html",
        "text/markdown" => "md",
        "application/zip" => "zip",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "application/msword" => "doc",
        "application/vnd.ms-excel" => "xls",
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        "audio/ogg" => "ogg",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => "bin",
    }
}

#[must_use]
pub fn is_binary_content_type(content_type: &str) -> bool {
    if content_type.trim().is_empty() {
        return false;
    }
    let normalized = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if normalized.starts_with("text/") {
        return false;
    }
    if normalized.ends_with("+json") || normalized == "application/json" {
        return false;
    }
    if normalized.ends_with("+xml") || normalized == "application/xml" {
        return false;
    }
    if normalized.starts_with("application/javascript") {
        return false;
    }
    if normalized == "application/x-www-form-urlencoded" {
        return false;
    }
    true
}

pub fn persist_binary_content(
    bytes: &[u8],
    mime_type: Option<&str>,
    persist_id: &str,
    tool_results_dir: &Path,
) -> Result<PersistedBinaryContent> {
    ensure_tool_results_dir(tool_results_dir)?;
    let extension = extension_for_mime_type(mime_type);
    let filepath = tool_results_dir.join(format!("{persist_id}.{extension}"));
    fs::write(&filepath, bytes)
        .with_context(|| format!("failed to write {}", filepath.display()))?;
    Ok(PersistedBinaryContent {
        filepath,
        size: bytes.len(),
        extension: extension.to_owned(),
    })
}

#[must_use]
pub fn get_binary_blob_saved_message(
    filepath: &Path,
    mime_type: Option<&str>,
    size: usize,
    source_description: &str,
) -> String {
    let mime_type = mime_type.unwrap_or("unknown type");
    format!(
        "{source_description}Binary content ({mime_type}, {}) saved to {}",
        format_file_size(size),
        filepath.display()
    )
}

fn format_file_size(size_in_bytes: usize) -> String {
    let kb = size_in_bytes as f64 / 1024.0;
    if kb < 1.0 {
        return format!("{size_in_bytes} bytes");
    }
    if kb < 1024.0 {
        return trim_trailing_zero(kb, "KB");
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return trim_trailing_zero(mb, "MB");
    }
    trim_trailing_zero(mb / 1024.0, "GB")
}

fn trim_trailing_zero(value: f64, suffix: &str) -> String {
    format!("{value:.1}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
        + suffix
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{
        BYTES_PER_TOKEN_ESTIMATE, DEFAULT_MAX_MCP_OUTPUT_TOKENS, McpResultFormat,
        extension_for_mime_type, get_binary_blob_saved_message, get_format_description,
        get_large_output_instructions, is_binary_content_type, parse_max_mcp_output_tokens,
        persist_binary_content,
    };

    #[test]
    fn extension_for_mime_type_maps_known_types() {
        assert_eq!(extension_for_mime_type(Some("application/pdf")), "pdf");
        assert_eq!(extension_for_mime_type(Some("image/png")), "png");
        assert_eq!(
            extension_for_mime_type(Some("application/json; charset=utf-8")),
            "json"
        );
        assert_eq!(extension_for_mime_type(None), "bin");
    }

    #[test]
    fn is_binary_content_type_keeps_textual_types_out_of_binary_path() {
        assert!(!is_binary_content_type("text/plain"));
        assert!(!is_binary_content_type("application/json"));
        assert!(!is_binary_content_type("application/problem+json"));
        assert!(!is_binary_content_type("application/xml"));
        assert!(is_binary_content_type("application/pdf"));
        assert!(is_binary_content_type("image/png"));
    }

    #[test]
    fn persist_binary_content_uses_mime_extension() {
        let temp = tempdir().expect("tempdir");
        let persisted = persist_binary_content(
            b"%PDF-1.7",
            Some("application/pdf"),
            "mcp-test",
            temp.path(),
        )
        .expect("persist");

        assert!(persisted.filepath.ends_with("mcp-test.pdf"));
        assert_eq!(persisted.size, 8);
        assert!(persisted.filepath.exists());
    }

    #[test]
    fn large_output_instructions_preserve_research_requirements() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("output.json");
        let instructions =
            get_large_output_instructions(&path, 120_000, "JSON array", Some(30_000));

        assert!(instructions.contains("Output has been saved to"));
        assert!(instructions.contains("Use offset and limit parameters"));
        assert!(instructions.contains("until 100% of the content has been read"));
        assert!(instructions.contains("did not read the entire content"));
        assert!(instructions.contains("Bash output is limited to 30000 chars"));
    }

    #[test]
    fn format_description_matches_research_categories() {
        assert_eq!(
            get_format_description(McpResultFormat::ToolResult, None),
            "Plain text"
        );
        assert_eq!(
            get_format_description(McpResultFormat::StructuredContent, Some("{id: number}")),
            "JSON with schema: {id: number}"
        );
    }

    #[test]
    fn max_mcp_output_tokens_parser_matches_research_env_override_rules() {
        assert_eq!(parse_max_mcp_output_tokens(Some("123")), Some(123));
        assert_eq!(parse_max_mcp_output_tokens(Some(" 456 ")), Some(456));
        assert_eq!(parse_max_mcp_output_tokens(Some("0")), None);
        assert_eq!(parse_max_mcp_output_tokens(Some("-1")), None);
        assert_eq!(parse_max_mcp_output_tokens(Some("not-a-number")), None);
        assert_eq!(parse_max_mcp_output_tokens(None), None);
        assert_eq!(
            DEFAULT_MAX_MCP_OUTPUT_TOKENS * BYTES_PER_TOKEN_ESTIMATE,
            100_000
        );
    }

    #[test]
    fn binary_blob_saved_message_mentions_size_and_path() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("artifact.bin");
        let message =
            get_binary_blob_saved_message(&path, Some("application/octet-stream"), 1536, "[MCP] ");
        assert!(message.contains("[MCP] Binary content"));
        assert!(message.contains("1.5KB"));
        assert!(message.contains(&path.display().to_string()));
    }
}
