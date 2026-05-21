//! User file sending tool: send_user_file.
//!
//! Provides a tool for sending files to the user (logs, screenshots,
//! exported data, etc.). Supports base64 encoding and file type detection.

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ToolExecutionContext;

/// Detected file type category.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileCategory {
    /// Text file (source code, logs, etc.).
    Text,
    /// Image file (png, jpg, gif, etc.).
    Image,
    /// Binary file (compiled, compressed, etc.).
    Binary,
    /// Document file (pdf, docx, etc.).
    Document,
    /// Data file (csv, json, xml, etc.).
    Data,
}

impl FileCategory {
    /// Detect the file category from a file extension.
    #[must_use]
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "txt" | "rs" | "ts" | "js" | "py" | "go" | "java" | "c" | "cpp" | "h" | "hpp"
            | "toml" | "yaml" | "yml" | "md" | "html" | "css" | "sh" | "bash" | "zsh" | "fish"
            | "log" | "cfg" | "ini" | "conf" => Self::Text,
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "webp" | "ico" | "tiff" | "tif" => {
                Self::Image
            }
            "pdf" | "docx" | "doc" | "odt" | "rtf" | "xlsx" | "xls" | "pptx" | "ppt" => {
                Self::Document
            }
            "csv" | "json" | "xml" | "tsv" | "parquet" | "sql" => Self::Data,
            _ => Self::Binary,
        }
    }

    /// Check if this category should be base64 encoded for transport.
    #[must_use]
    pub fn needs_encoding(self) -> bool {
        matches!(self, Self::Image | Self::Binary | Self::Document)
    }
}

/// File metadata for user file delivery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserFileInfo {
    /// File name.
    pub name: String,
    /// File extension (without dot).
    pub extension: String,
    /// Detected file category.
    pub category: FileCategory,
    /// File size in bytes.
    pub size_bytes: u64,
    /// MIME type guess.
    pub mime_type: String,
    /// Whether the content is base64 encoded.
    pub is_base64: bool,
}

/// Send a file to the user.
///
/// Reads a file from the workspace and prepares it for delivery to the user.
/// Binary files are base64 encoded. Text files are included as-is (up to a
/// size limit).
///
/// # Errors
/// Returns an error if the file path is missing or the file cannot be read.
pub fn send_user_file(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let file_path = input["file_path"]
        .as_str()
        .ok_or_else(|| anyhow!("file_path is required"))?;

    if file_path.trim().is_empty() {
        return Err(anyhow!("file_path cannot be empty"));
    }

    let full_path = context.cwd.join(file_path);

    if !full_path.exists() {
        return Err(anyhow!("File not found: {}", full_path.display()));
    }

    let metadata = std::fs::metadata(&full_path).context("failed to read file metadata")?;
    let size_bytes = metadata.len();

    // Check size limit (default 10MB).
    let max_size = input["max_size_bytes"].as_u64().unwrap_or(10 * 1024 * 1024);

    if size_bytes > max_size {
        return Ok(json!({
            "type": "send_user_file",
            "file_path": file_path,
            "status": "too_large",
            "size_bytes": size_bytes,
            "max_size_bytes": max_size,
            "message": format!(
                "File is too large ({} bytes). Maximum allowed: {} bytes.",
                size_bytes, max_size
            ),
        })
        .to_string());
    }

    let file_name = full_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let extension = full_path
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();

    let category = FileCategory::from_extension(&extension);
    let mime_type = guess_mime_type(&extension);
    let description = input["description"].as_str().unwrap_or("");

    let file_info = UserFileInfo {
        name: file_name.clone(),
        extension: extension.clone(),
        category,
        size_bytes,
        mime_type: mime_type.clone(),
        is_base64: category.needs_encoding(),
    };

    // Read and optionally encode the file content.
    let content = if category.needs_encoding() {
        let bytes = std::fs::read(&full_path).context("failed to read binary file")?;
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        json!(encoded)
    } else {
        let text = std::fs::read_to_string(&full_path).context("failed to read text file")?;
        // Truncate if too large for text content.
        let max_text_chars = input["max_text_chars"].as_u64().unwrap_or(50_000) as usize;
        if text.len() > max_text_chars {
            json!(text[..max_text_chars].to_string() + "\n... (truncated)")
        } else {
            json!(text)
        }
    };

    Ok(json!({
        "type": "send_user_file",
        "file_path": file_path,
        "status": "ready",
        "description": description,
        "file_info": {
            "name": file_info.name,
            "extension": file_info.extension,
            "category": serde_json::to_value(file_info.category).expect("category serializes"),
            "size_bytes": file_info.size_bytes,
            "mime_type": file_info.mime_type,
            "is_base64": file_info.is_base64,
        },
        "content": content,
        "message": format!("File '{file_name}' ({}, {}) ready for delivery.", mime_type, format_size(size_bytes)),
    })
    .to_string())
}

/// Guess the MIME type from a file extension.
fn guess_mime_type(extension: &str) -> String {
    match extension.to_ascii_lowercase().as_str() {
        "txt" | "log" => "text/plain".to_string(),
        "rs" => "text/rust".to_string(),
        "ts" | "tsx" => "text/typescript".to_string(),
        "js" | "jsx" => "text/javascript".to_string(),
        "py" => "text/x-python".to_string(),
        "json" => "application/json".to_string(),
        "csv" => "text/csv".to_string(),
        "xml" => "application/xml".to_string(),
        "html" => "text/html".to_string(),
        "css" => "text/css".to_string(),
        "md" => "text/markdown".to_string(),
        "pdf" => "application/pdf".to_string(),
        "png" => "image/png".to_string(),
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "gif" => "image/gif".to_string(),
        "svg" => "image/svg+xml".to_string(),
        "zip" => "application/zip".to_string(),
        "tar" | "gz" => "application/gzip".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

/// Format a file size in human-readable form.
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} bytes")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_context_with_cwd(cwd: &Path) -> ToolExecutionContext {
        ToolExecutionContext {
            cwd: cwd.to_path_buf(),
            original_cwd: cwd.to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 30_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Arc::new(parking_lot::Mutex::new(
                claude_core::task_stack::TaskStack::default(),
            )),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    #[test]
    fn file_category_from_extension_text() {
        assert_eq!(FileCategory::from_extension("rs"), FileCategory::Text);
        assert_eq!(FileCategory::from_extension("py"), FileCategory::Text);
        assert_eq!(FileCategory::from_extension("js"), FileCategory::Text);
        assert_eq!(FileCategory::from_extension("log"), FileCategory::Text);
        assert_eq!(FileCategory::from_extension("md"), FileCategory::Text);
    }

    #[test]
    fn file_category_from_extension_image() {
        assert_eq!(FileCategory::from_extension("png"), FileCategory::Image);
        assert_eq!(FileCategory::from_extension("jpg"), FileCategory::Image);
        assert_eq!(FileCategory::from_extension("gif"), FileCategory::Image);
        assert_eq!(FileCategory::from_extension("svg"), FileCategory::Image);
    }

    #[test]
    fn file_category_from_extension_document() {
        assert_eq!(FileCategory::from_extension("pdf"), FileCategory::Document);
        assert_eq!(FileCategory::from_extension("docx"), FileCategory::Document);
        assert_eq!(FileCategory::from_extension("xlsx"), FileCategory::Document);
    }

    #[test]
    fn file_category_from_extension_data() {
        assert_eq!(FileCategory::from_extension("csv"), FileCategory::Data);
        assert_eq!(FileCategory::from_extension("json"), FileCategory::Data);
        assert_eq!(FileCategory::from_extension("xml"), FileCategory::Data);
    }

    #[test]
    fn file_category_from_extension_binary() {
        assert_eq!(FileCategory::from_extension("exe"), FileCategory::Binary);
        assert_eq!(FileCategory::from_extension("zip"), FileCategory::Binary);
        assert_eq!(
            FileCategory::from_extension("unknown"),
            FileCategory::Binary
        );
    }

    #[test]
    fn file_category_needs_encoding() {
        assert!(FileCategory::Image.needs_encoding());
        assert!(FileCategory::Binary.needs_encoding());
        assert!(FileCategory::Document.needs_encoding());
        assert!(!FileCategory::Text.needs_encoding());
        assert!(!FileCategory::Data.needs_encoding());
    }

    #[test]
    fn file_category_case_insensitive() {
        assert_eq!(FileCategory::from_extension("PNG"), FileCategory::Image);
        assert_eq!(FileCategory::from_extension("Rs"), FileCategory::Text);
    }

    #[test]
    fn guess_mime_type_common_types() {
        assert_eq!(guess_mime_type("json"), "application/json");
        assert_eq!(guess_mime_type("png"), "image/png");
        assert_eq!(guess_mime_type("pdf"), "application/pdf");
        assert_eq!(guess_mime_type("html"), "text/html");
        assert_eq!(guess_mime_type("unknown"), "application/octet-stream");
    }

    #[test]
    fn format_size_human_readable() {
        assert_eq!(format_size(0), "0 bytes");
        assert_eq!(format_size(512), "512 bytes");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn send_user_file_requires_file_path() {
        let input = json!({});
        let context = test_context_with_cwd(Path::new("/tmp"));
        let result = send_user_file(&input, &context);
        let error = result.expect_err("missing file_path should fail");
        assert!(error.to_string().contains("file_path"));
    }

    #[test]
    fn send_user_file_rejects_empty_path() {
        let input = json!({"file_path": ""});
        let context = test_context_with_cwd(Path::new("/tmp"));
        let result = send_user_file(&input, &context);
        assert!(result.is_err());
    }

    #[test]
    fn send_user_file_handles_nonexistent_file() {
        let input = json!({"file_path": "nonexistent.txt"});
        let context = test_context_with_cwd(Path::new("/tmp"));
        let result = send_user_file(&input, &context);
        let error = result.expect_err("nonexistent file should fail");
        assert!(error.to_string().contains("not found"));
    }

    #[test]
    fn send_user_file_reads_text_file() {
        let temp = TempDir::new().expect("temp dir");
        std::fs::write(temp.path().join("test.txt"), "Hello, world!").expect("write file");

        let input = json!({"file_path": "test.txt", "description": "A test file"});
        let context = test_context_with_cwd(temp.path());
        let result = send_user_file(&input, &context).expect("text file should be read");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["status"], "ready");
        assert_eq!(parsed["content"], "Hello, world!");
        assert_eq!(parsed["file_info"]["category"], "text");
        assert_eq!(parsed["file_info"]["is_base64"], false);
        assert_eq!(parsed["description"], "A test file");
    }

    #[test]
    fn send_user_file_encodes_binary_file() {
        let temp = TempDir::new().expect("temp dir");
        let binary_content = vec![0u8, 1, 2, 3, 255, 254, 253];
        std::fs::write(temp.path().join("test.bin"), &binary_content).expect("write file");

        let input = json!({"file_path": "test.bin"});
        let context = test_context_with_cwd(temp.path());
        let result = send_user_file(&input, &context).expect("binary file should be encoded");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["status"], "ready");
        assert_eq!(parsed["file_info"]["category"], "binary");
        assert_eq!(parsed["file_info"]["is_base64"], true);
        assert!(parsed["content"].is_string());
    }

    #[test]
    fn send_user_file_handles_large_file() {
        let temp = TempDir::new().expect("temp dir");
        let large_content = "x".repeat(200);
        std::fs::write(temp.path().join("large.txt"), &large_content).expect("write file");

        let input = json!({"file_path": "large.txt", "max_size_bytes": 100});
        let context = test_context_with_cwd(temp.path());
        let result = send_user_file(&input, &context).expect("oversized file should be handled");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["status"], "too_large");
    }

    #[test]
    fn send_user_file_truncates_large_text() {
        let temp = TempDir::new().expect("temp dir");
        let content = "A".repeat(1000);
        std::fs::write(temp.path().join("big.txt"), &content).expect("write file");

        let input = json!({"file_path": "big.txt", "max_text_chars": 100});
        let context = test_context_with_cwd(temp.path());
        let result = send_user_file(&input, &context).expect("large text file should be handled");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        let text = parsed["content"].as_str().expect("content string");
        assert!(text.contains("truncated"));
        assert!(text.len() < 200);
    }

    #[test]
    fn user_file_info_round_trips_json() {
        let info = UserFileInfo {
            name: "test.rs".to_string(),
            extension: "rs".to_string(),
            category: FileCategory::Text,
            size_bytes: 1024,
            mime_type: "text/rust".to_string(),
            is_base64: false,
        };
        let json = serde_json::to_string(&info).expect("serialize");
        let parsed: UserFileInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, info);
    }

    #[test]
    fn send_user_file_detects_image_category() {
        let temp = TempDir::new().expect("temp dir");
        std::fs::write(temp.path().join("screenshot.png"), b"\x89PNG\r\n").expect("write file");

        let input = json!({"file_path": "screenshot.png"});
        let context = test_context_with_cwd(temp.path());
        let result = send_user_file(&input, &context).expect("image file should be detected");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["file_info"]["category"], "image");
        assert_eq!(parsed["file_info"]["is_base64"], true);
    }

    #[test]
    fn send_user_file_detects_data_category() {
        let temp = TempDir::new().expect("temp dir");
        std::fs::write(temp.path().join("data.csv"), "a,b,c\n1,2,3").expect("write file");

        let input = json!({"file_path": "data.csv"});
        let context = test_context_with_cwd(temp.path());
        let result = send_user_file(&input, &context).expect("data file should be detected");

        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["file_info"]["category"], "data");
        assert_eq!(parsed["file_info"]["is_base64"], false);
    }
}
