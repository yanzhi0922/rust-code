//! Tool Use Summary Generator — generates human-readable summaries of tool batches.
//!
//! Produces short, git-commit-style labels describing what a batch of tool calls
//! accomplished. Used by the SDK to provide high-level progress updates.
//!
//! # Architecture
//!
//! - [`ToolUseSummaryGenerator`] — main generator with template-based summarization
//! - [`ToolInfo`] — information about a single tool invocation
//! - [`SummaryTemplate`] — template for generating summaries
//! - [`generate_summary()`] — core summary generation function

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tool Info
// ---------------------------------------------------------------------------

/// Information about a single tool invocation for summary generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    /// Tool name (e.g. "Read", "Edit", "Bash").
    pub name: String,
    /// Tool input as a JSON string.
    pub input: String,
    /// Tool output as a string.
    pub output: String,
    /// Whether the tool call resulted in an error.
    #[serde(default)]
    pub is_error: bool,
}

impl ToolInfo {
    /// Creates a new tool info.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        input: impl Into<String>,
        output: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            input: input.into(),
            output: output.into(),
            is_error: false,
        }
    }

    /// Creates a tool info representing an error.
    #[must_use]
    pub fn with_error(
        name: impl Into<String>,
        input: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            input: input.into(),
            output: error.into(),
            is_error: true,
        }
    }

    /// Returns a truncated version of the input.
    #[must_use]
    pub fn truncated_input(&self, max_len: usize) -> &str {
        truncate_str(&self.input, max_len)
    }

    /// Returns a truncated version of the output.
    #[must_use]
    pub fn truncated_output(&self, max_len: usize) -> &str {
        truncate_str(&self.output, max_len)
    }
}

// ---------------------------------------------------------------------------
// Summary Template
// ---------------------------------------------------------------------------

/// A template for generating tool use summaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryTemplate {
    /// Template pattern (may contain {tool}, {file}, {action} placeholders).
    pub pattern: String,
    /// Tool name this template applies to (empty = wildcard).
    pub tool_name: String,
    /// Priority (lower = higher priority).
    pub priority: u32,
}

impl SummaryTemplate {
    /// Creates a new summary template.
    #[must_use]
    pub fn new(tool_name: impl Into<String>, pattern: impl Into<String>, priority: u32) -> Self {
        Self {
            pattern: pattern.into(),
            tool_name: tool_name.into(),
            priority,
        }
    }

    /// Applies this template to a tool info, returning the formatted summary.
    #[must_use]
    pub fn apply(&self, tool: &ToolInfo) -> String {
        let file = extract_file_from_input(&tool.input);
        let action = infer_action_from_tool(&tool.name);

        self.pattern
            .replace("{tool}", &tool.name)
            .replace("{file}", &file)
            .replace("{action}", &action)
    }
}

// ---------------------------------------------------------------------------
// Built-in Templates
// ---------------------------------------------------------------------------

/// Returns the default summary templates.
fn default_templates() -> Vec<SummaryTemplate> {
    vec![
        SummaryTemplate::new("Read", "Read {file}", 10),
        SummaryTemplate::new("Edit", "Edited {file}", 10),
        SummaryTemplate::new("Write", "Created {file}", 10),
        SummaryTemplate::new("Bash", "Ran command", 20),
        SummaryTemplate::new("Grep", "Searched in codebase", 15),
        SummaryTemplate::new("Glob", "Found files", 15),
        SummaryTemplate::new("NotebookEdit", "Edited notebook {file}", 10),
        SummaryTemplate::new("", "{action} with {tool}", 100),
    ]
}

// ---------------------------------------------------------------------------
// Tool Use Summary Generator
// ---------------------------------------------------------------------------

/// Configuration for the summary generator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryGeneratorConfig {
    /// Maximum input truncation length.
    pub max_input_length: usize,
    /// Maximum output truncation length.
    pub max_output_length: usize,
    /// Maximum summary length.
    pub max_summary_length: usize,
    /// Whether to include error details in summaries.
    pub include_error_details: bool,
}

impl Default for SummaryGeneratorConfig {
    fn default() -> Self {
        Self {
            max_input_length: 300,
            max_output_length: 300,
            max_summary_length: 60,
            include_error_details: true,
        }
    }
}

/// Generates human-readable summaries of tool use batches.
pub struct ToolUseSummaryGenerator {
    /// Configuration.
    config: SummaryGeneratorConfig,
    /// Registered templates.
    templates: Vec<SummaryTemplate>,
    /// Statistics.
    stats: GeneratorStats,
}

/// Statistics about summary generation.
#[derive(Debug, Clone, Default)]
pub struct GeneratorStats {
    /// Total summaries generated.
    pub total_generated: u64,
    /// Summaries that were truncated.
    pub truncated_count: u64,
    /// Summaries that included errors.
    pub error_summaries: u64,
    /// Empty batches skipped.
    pub empty_batches_skipped: u64,
}

impl Default for ToolUseSummaryGenerator {
    fn default() -> Self {
        Self::new(SummaryGeneratorConfig::default())
    }
}

impl ToolUseSummaryGenerator {
    /// Creates a new generator with the given configuration.
    #[must_use]
    pub fn new(config: SummaryGeneratorConfig) -> Self {
        Self {
            config,
            templates: default_templates(),
            stats: GeneratorStats::default(),
        }
    }

    /// Adds a custom template.
    pub fn add_template(&mut self, template: SummaryTemplate) {
        self.templates.push(template);
    }

    /// Generates a summary for a batch of tool calls.
    pub fn generate(&mut self, tools: &[ToolInfo]) -> Option<String> {
        if tools.is_empty() {
            self.stats.empty_batches_skipped += 1;
            return None;
        }

        let summary = generate_summary(tools, &self.templates, &self.config);

        if let Some(ref s) = summary {
            self.stats.total_generated += 1;
            if s.len() > self.config.max_summary_length {
                self.stats.truncated_count += 1;
            }
            if tools.iter().any(|t| t.is_error) {
                self.stats.error_summaries += 1;
            }
        }

        summary
    }

    /// Returns the generator statistics.
    #[must_use]
    pub fn stats(&self) -> &GeneratorStats {
        &self.stats
    }

    /// Resets the generator statistics.
    pub fn reset_stats(&mut self) {
        self.stats = GeneratorStats::default();
    }

    /// Returns the registered templates.
    #[must_use]
    pub fn templates(&self) -> &[SummaryTemplate] {
        &self.templates
    }
}

// ---------------------------------------------------------------------------
// Core Generation Logic
// ---------------------------------------------------------------------------

/// Generates a summary from a batch of tool calls.
pub fn generate_summary(
    tools: &[ToolInfo],
    templates: &[SummaryTemplate],
    config: &SummaryGeneratorConfig,
) -> Option<String> {
    if tools.is_empty() {
        return None;
    }

    // Single tool — use template directly
    if tools.len() == 1 {
        let tool = &tools[0];
        let summary = summarize_single_tool(tool, templates);
        return Some(truncate_summary(&summary, config.max_summary_length));
    }

    // Multiple tools — aggregate
    let summary = summarize_batch(tools, templates);
    Some(truncate_summary(&summary, config.max_summary_length))
}

/// Summarizes a single tool call.
fn summarize_single_tool(tool: &ToolInfo, templates: &[SummaryTemplate]) -> String {
    // Find matching template
    let matching: Vec<&SummaryTemplate> = templates
        .iter()
        .filter(|t| t.tool_name == tool.name || t.tool_name.is_empty())
        .collect();

    if let Some(best) = matching.iter().min_by_key(|t| t.priority) {
        return best.apply(tool);
    }

    // Fallback
    format!("{}: {}", tool.name, truncate_str(&tool.input, 50))
}

/// Summarizes a batch of tool calls.
fn summarize_batch(tools: &[ToolInfo], _templates: &[SummaryTemplate]) -> String {
    // Count tool types
    let mut tool_counts: HashMap<&str, usize> = HashMap::new();
    let mut files: Vec<&str> = Vec::new();
    let mut has_errors = false;

    for tool in tools {
        *tool_counts.entry(&tool.name).or_insert(0) += 1;
        if tool.is_error {
            has_errors = true;
        }
        let file = extract_file_from_input(&tool.input);
        if !file.is_empty() && !files.contains(&file.as_str()) {
            files.push(Box::leak(file.into_boxed_str()));
        }
    }

    // Sort by frequency
    let mut sorted_tools: Vec<_> = tool_counts.into_iter().collect();
    sorted_tools.sort_by(|a, b| b.1.cmp(&a.1));

    let primary_tool = sorted_tools
        .first()
        .map(|(name, _)| *name)
        .unwrap_or("Unknown");

    // Build summary
    let action = infer_action_from_tool(primary_tool);

    if has_errors {
        if let Some(file) = files.first() {
            return format!("{action} {file} (with errors)");
        }
        return format!("{action} (with errors)");
    }

    match sorted_tools.len() {
        1 => {
            let count = sorted_tools[0].1;
            if count == 1 {
                if let Some(file) = files.first() {
                    format!("{action} {file}")
                } else {
                    action
                }
            } else {
                format!("{action} {count} files")
            }
        }
        2 => {
            let second = sorted_tools.get(1).map(|(n, _)| *n).unwrap_or("");
            let second_action = infer_action_from_tool(second);
            if let Some(file) = files.first() {
                format!("{action} and {second_action} {file}")
            } else {
                format!("{action} and {second_action}")
            }
        }
        _ => {
            if let Some(file) = files.first() {
                format!("{action} and {} more in {file}", sorted_tools.len() - 1)
            } else {
                format!("{action} and {} more", sorted_tools.len() - 1)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extracts a file path from tool input (heuristic).
fn extract_file_from_input(input: &str) -> String {
    // Try JSON parsing first
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(input) {
        if let Some(path) = val.get("file_path").and_then(|v| v.as_str()) {
            return path.to_string();
        }
        if let Some(path) = val.get("path").and_then(|v| v.as_str()) {
            return path.to_string();
        }
        if let Some(path) = val.get("filePath").and_then(|v| v.as_str()) {
            return path.to_string();
        }
    }

    // Fallback: look for file-like patterns
    let extensions = [
        ".rs", ".ts", ".js", ".py", ".toml", ".json", ".yaml", ".yml", ".md", ".css", ".html",
        ".go", ".java", ".c", ".cpp", ".h",
    ];

    for word in input.split_whitespace() {
        let cleaned = word.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ':');
        if extensions.iter().any(|ext| cleaned.ends_with(ext)) {
            return cleaned.to_string();
        }
    }

    String::new()
}

/// Infers a past-tense action verb from a tool name.
fn infer_action_from_tool(tool_name: &str) -> String {
    match tool_name {
        "Read" => "Read".to_string(),
        "Edit" => "Edited".to_string(),
        "Write" => "Created".to_string(),
        "Bash" => "Ran command".to_string(),
        "Grep" => "Searched".to_string(),
        "Glob" => "Found files".to_string(),
        "NotebookEdit" => "Edited notebook".to_string(),
        "LSP" => "Ran LSP".to_string(),
        "ToolSearch" => "Searched tools".to_string(),
        _ => format!("Used {tool_name}"),
    }
}

/// Truncates a string to a maximum length.
fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    let mut end = max_len;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Truncates a summary to the maximum length with ellipsis.
fn truncate_summary(summary: &str, max_len: usize) -> String {
    if summary.len() <= max_len {
        return summary.to_string();
    }

    let mut end = max_len.saturating_sub(3);
    while end > 0 && !summary.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &summary[..end])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- ToolInfo tests ---

    #[test]
    fn test_tool_info_new() {
        let info = ToolInfo::new("Read", r#"{"file_path": "src/main.rs"}"#, "file contents");
        assert_eq!(info.name, "Read");
        assert!(!info.is_error);
    }

    #[test]
    fn test_tool_info_with_error() {
        let info = ToolInfo::with_error("Bash", "cargo build", "compilation failed");
        assert!(info.is_error);
    }

    #[test]
    fn test_tool_info_truncated_input() {
        let long_input = "x".repeat(500);
        let info = ToolInfo::new("Test", long_input.clone(), "output");
        let truncated = info.truncated_input(100);
        assert!(truncated.len() <= 100);
    }

    #[test]
    fn test_tool_info_truncated_output() {
        let long_output = "y".repeat(500);
        let info = ToolInfo::new("Test", "input", long_output);
        let truncated = info.truncated_output(50);
        assert!(truncated.len() <= 50);
    }

    // --- SummaryTemplate tests ---

    #[test]
    fn test_template_new() {
        let tmpl = SummaryTemplate::new("Read", "Read {file}", 10);
        assert_eq!(tmpl.tool_name, "Read");
        assert_eq!(tmpl.priority, 10);
    }

    #[test]
    fn test_template_apply() {
        let tmpl = SummaryTemplate::new("Read", "Read {file}", 10);
        let tool = ToolInfo::new("Read", r#"{"file_path": "src/main.rs"}"#, "contents");
        let result = tmpl.apply(&tool);
        assert!(result.contains("src/main.rs"));
    }

    #[test]
    fn test_template_apply_with_action() {
        let tmpl = SummaryTemplate::new("Edit", "{action} {file}", 10);
        let tool = ToolInfo::new("Edit", r#"{"file_path": "lib.rs"}"#, "ok");
        let result = tmpl.apply(&tool);
        assert!(result.contains("Edited"));
        assert!(result.contains("lib.rs"));
    }

    // --- extract_file_from_input tests ---

    #[test]
    fn test_extract_file_json() {
        let file = extract_file_from_input(r#"{"file_path": "src/main.rs"}"#);
        assert_eq!(file, "src/main.rs");
    }

    #[test]
    fn test_extract_file_path_key() {
        let file = extract_file_from_input(r#"{"path": "config.toml"}"#);
        assert_eq!(file, "config.toml");
    }

    #[test]
    fn test_extract_file_heuristic() {
        let file = extract_file_from_input("check the file src/lib.rs for errors");
        assert_eq!(file, "src/lib.rs");
    }

    #[test]
    fn test_extract_file_empty() {
        let file = extract_file_from_input("no file here");
        assert!(file.is_empty());
    }

    // --- infer_action_from_tool tests ---

    #[test]
    fn test_infer_action_known() {
        assert_eq!(infer_action_from_tool("Read"), "Read");
        assert_eq!(infer_action_from_tool("Edit"), "Edited");
        assert_eq!(infer_action_from_tool("Write"), "Created");
        assert_eq!(infer_action_from_tool("Bash"), "Ran command");
    }

    #[test]
    fn test_infer_action_unknown() {
        assert_eq!(infer_action_from_tool("CustomTool"), "Used CustomTool");
    }

    // --- generate_summary tests ---

    #[test]
    fn test_generate_summary_empty() {
        let config = SummaryGeneratorConfig::default();
        let templates = default_templates();
        let result = generate_summary(&[], &templates, &config);
        assert!(result.is_none());
    }

    #[test]
    fn test_generate_summary_single_read() {
        let config = SummaryGeneratorConfig::default();
        let templates = default_templates();
        let tools = vec![ToolInfo::new(
            "Read",
            r#"{"file_path": "src/main.rs"}"#,
            "contents",
        )];
        let result = generate_summary(&tools, &templates, &config);
        assert!(result.is_some());
        let summary = result.expect("summary");
        assert!(summary.contains("src/main.rs"));
    }

    #[test]
    fn test_generate_summary_single_edit() {
        let config = SummaryGeneratorConfig::default();
        let templates = default_templates();
        let tools = vec![ToolInfo::new("Edit", r#"{"file_path": "lib.rs"}"#, "ok")];
        let result = generate_summary(&tools, &templates, &config);
        assert!(result.is_some());
        assert!(result.expect("summary").contains("lib.rs"));
    }

    #[test]
    fn test_generate_summary_batch() {
        let config = SummaryGeneratorConfig::default();
        let templates = default_templates();
        let tools = vec![
            ToolInfo::new("Read", r#"{"file_path": "a.rs"}"#, "contents"),
            ToolInfo::new("Edit", r#"{"file_path": "b.rs"}"#, "ok"),
        ];
        let result = generate_summary(&tools, &templates, &config);
        assert!(result.is_some());
    }

    #[test]
    fn test_generate_summary_with_errors() {
        let config = SummaryGeneratorConfig::default();
        let templates = default_templates();
        let tools = vec![
            ToolInfo::new("Bash", "cargo build", "ok"),
            ToolInfo::with_error("Bash", "cargo test", "test failed"),
        ];
        let result = generate_summary(&tools, &templates, &config);
        assert!(result.is_some());
        assert!(result.expect("summary").contains("error"));
    }

    // --- ToolUseSummaryGenerator tests ---

    #[test]
    fn test_generator_default() {
        let generator = ToolUseSummaryGenerator::default();
        assert_eq!(generator.stats().total_generated, 0);
        assert!(!generator.templates().is_empty());
    }

    #[test]
    fn test_generator_empty_batch() {
        let mut generator = ToolUseSummaryGenerator::default();
        let result = generator.generate(&[]);
        assert!(result.is_none());
        assert_eq!(generator.stats().empty_batches_skipped, 1);
    }

    #[test]
    fn test_generator_single_tool() {
        let mut generator = ToolUseSummaryGenerator::default();
        let tools = vec![ToolInfo::new(
            "Read",
            r#"{"file_path": "main.rs"}"#,
            "contents",
        )];
        let result = generator.generate(&tools);
        assert!(result.is_some());
        assert_eq!(generator.stats().total_generated, 1);
    }

    #[test]
    fn test_generator_batch_tools() {
        let mut generator = ToolUseSummaryGenerator::default();
        let tools = vec![
            ToolInfo::new("Read", r#"{"file_path": "a.rs"}"#, "ok"),
            ToolInfo::new("Edit", r#"{"file_path": "b.rs"}"#, "ok"),
            ToolInfo::new("Bash", "cargo test", "passed"),
        ];
        let result = generator.generate(&tools);
        assert!(result.is_some());
    }

    #[test]
    fn test_generator_error_tracking() {
        let mut generator = ToolUseSummaryGenerator::default();
        let tools = vec![ToolInfo::with_error("Bash", "build", "failed")];
        let _ = generator.generate(&tools);
        assert_eq!(generator.stats().error_summaries, 1);
    }

    #[test]
    fn test_generator_reset_stats() {
        let mut generator = ToolUseSummaryGenerator::default();
        let tools = vec![ToolInfo::new("Read", "input", "output")];
        let _ = generator.generate(&tools);
        generator.reset_stats();
        assert_eq!(generator.stats().total_generated, 0);
    }

    #[test]
    fn test_generator_custom_template() {
        let mut generator = ToolUseSummaryGenerator::default();
        generator.add_template(SummaryTemplate::new("CustomTool", "Custom {file}", 5));
        let tools = vec![ToolInfo::new(
            "CustomTool",
            r#"{"file_path": "test.rs"}"#,
            "ok",
        )];
        let result = generator.generate(&tools);
        assert!(result.is_some());
        assert!(result.expect("summary").contains("Custom"));
    }

    // --- truncate_summary tests ---

    #[test]
    fn test_truncate_summary_short() {
        let result = truncate_summary("Short summary", 100);
        assert_eq!(result, "Short summary");
    }

    #[test]
    fn test_truncate_summary_long() {
        let long = "This is a very long summary that should be truncated".repeat(5);
        let result = truncate_summary(&long, 30);
        assert!(result.len() <= 33); // 30 + "..."
        assert!(result.ends_with("..."));
    }
}
