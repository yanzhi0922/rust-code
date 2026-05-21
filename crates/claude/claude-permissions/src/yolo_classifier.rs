//! YOLO Classifier — auto-approve safe operations based on rule sets.
//!
//! Corresponds to `.research/cc-haha/src/utils/permissions/yoloClassifier.ts`.
//! Provides rule-based classification for tool use in auto-permission mode,
//! allowing known-safe operations and soft-denying dangerous ones.
//!
//! Also includes a 2-stage LLM-based classification pipeline matching TS:
//! - Stage 1 ("fast"): max_tokens=64, stop_sequences=['</block>'], parses XML
//! - Stage 2 ("thinking"): max_tokens=4096, full chain-of-thought reasoning
//! - Fail-closed: any ambiguity → block
//! - Safe tools allowlist bypasses the classifier entirely

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

/// Result of a YOLO classifier evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YoloClassifierResult {
    /// The operation is known-safe and should be auto-approved.
    Allow,
    /// The operation is potentially dangerous; deny with a reason.
    Deny(String),
    /// The operation needs user confirmation; ask with a prompt.
    Ask(String),
}

/// Rules for auto mode classification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoModeRules {
    /// Glob / exact patterns for tools/commands that are always allowed.
    pub allow: Vec<String>,
    /// Patterns for commands that should be soft-denied (user must confirm).
    pub soft_deny: Vec<String>,
    /// Environment variables that are safe to inspect.
    pub environment: Vec<String>,
}

impl Default for AutoModeRules {
    fn default() -> Self {
        Self::external()
    }
}

impl AutoModeRules {
    /// Default rules for external / third-party models.
    ///
    /// Conservative: only allow read-only git, build commands, and file reads.
    pub fn external() -> Self {
        Self {
            allow: vec![
                // Git read operations
                "git status".into(),
                "git log".into(),
                "git diff".into(),
                "git branch --list".into(),
                "git remote -v".into(),
                "git show".into(),
                "git stash list".into(),
                "git tag --list".into(),
                // Build / package commands
                "cargo build".into(),
                "cargo check".into(),
                "cargo test".into(),
                "cargo clippy".into(),
                "cargo doc".into(),
                "npm install".into(),
                "npm run build".into(),
                "npm test".into(),
                "npm ci".into(),
                "node ".into(),
                "npx ".into(),
                "yarn install".into(),
                "yarn build".into(),
                "yarn test".into(),
                "pnpm install".into(),
                "pnpm build".into(),
                "pnpm test".into(),
                "pip install".into(),
                "python ".into(),
                "python3 ".into(),
                "make ".into(),
                "cmake ".into(),
                // File read operations
                "cat ".into(),
                "head ".into(),
                "tail ".into(),
                "less ".into(),
                "wc ".into(),
                "file ".into(),
                "stat ".into(),
                // Directory listing
                "ls".into(),
                "dir".into(),
                "find ".into(),
                "tree".into(),
                "pwd".into(),
                // Safe utilities
                "echo ".into(),
                "which ".into(),
                "whoami".into(),
                "uname".into(),
                "date".into(),
                "env".into(),
                "printenv".into(),
                "id".into(),
                "hostname".into(),
                "df -h".into(),
                "du ".into(),
                "ps ".into(),
                "top -bn1".into(),
            ],
            soft_deny: vec![
                "rm -rf".into(),
                "rm -r".into(),
                "rmdir".into(),
                "git push --force".into(),
                "git push -f".into(),
                "git push --force-with-lease".into(),
                "git reset --hard".into(),
                "git clean -fd".into(),
                "drop database".into(),
                "DROP DATABASE".into(),
                "format disk".into(),
                "mkfs".into(),
                "sudo ".into(),
                "su ".into(),
                "chmod 777".into(),
                "chown ".into(),
                "dd if=".into(),
                "> /dev/sd".into(),
                "shutdown".into(),
                "reboot".into(),
                "halt".into(),
                "poweroff".into(),
                "systemctl stop".into(),
                "service stop".into(),
                "kill -9".into(),
                "killall".into(),
                "iptables".into(),
                "ufw ".into(),
            ],
            environment: vec![
                "PATH".into(),
                "HOME".into(),
                "USER".into(),
                "SHELL".into(),
                "LANG".into(),
                "TERM".into(),
                "PWD".into(),
                "EDITOR".into(),
                "VISUAL".into(),
                "RUSTUP_HOME".into(),
                "CARGO_HOME".into(),
                "NODE_VERSION".into(),
                "NVM_DIR".into(),
                "JAVA_HOME".into(),
                "GOPATH".into(),
                "PYTHONPATH".into(),
                "VIRTUAL_ENV".into(),
                "CONDA_DEFAULT_ENV".into(),
                "DOCKER_HOST".into(),
            ],
        }
    }

    /// Default rules for Anthropic-hosted models.
    ///
    /// More permissive: allows additional safe operations.
    pub fn anthropic() -> Self {
        let mut rules = Self::external();
        // Additional allowed operations for first-party models
        rules.allow.extend_from_slice(&[
            "git add ".into(),
            "git commit".into(),
            "git checkout ".into(),
            "git switch ".into(),
            "git merge ".into(),
            "git rebase ".into(),
            "git pull".into(),
            "git fetch".into(),
            "git stash".into(),
            "git tag ".into(),
            "mkdir ".into(),
            "touch ".into(),
            "cp ".into(),
            "mv ".into(),
            "chmod ".into(),
            "curl ".into(),
            "wget ".into(),
            "docker build".into(),
            "docker run".into(),
            "docker ps".into(),
            "docker-compose up".into(),
            "docker compose up".into(),
        ]);
        rules
    }

    /// Check if a command matches any allow rule.
    pub fn is_allowed(&self, command: &str) -> bool {
        let command_lower = command.to_lowercase();
        self.allow.iter().any(|pattern| {
            let pattern_lower = pattern.to_lowercase();
            command_lower == pattern_lower
                || command_lower.starts_with(&format!("{pattern_lower} "))
                || command_lower.starts_with(&pattern_lower)
                    && (pattern.ends_with(' ') || !command_lower.contains(' '))
        })
    }

    /// Check if a command matches any soft-deny rule.
    pub fn is_soft_denied(&self, command: &str) -> bool {
        let command_lower = command.to_lowercase();
        self.soft_deny.iter().any(|pattern| {
            let pattern_lower = pattern.to_lowercase();
            command_lower.contains(&pattern_lower)
        })
    }

    /// Check if an environment variable is safe to inspect.
    pub fn is_env_allowed(&self, var_name: &str) -> bool {
        self.environment
            .iter()
            .any(|v| v.eq_ignore_ascii_case(var_name))
    }
}

/// Get the default external auto mode rules.
#[must_use]
pub fn get_default_external_auto_mode_rules() -> AutoModeRules {
    AutoModeRules::external()
}

/// Get the default Anthropic auto mode rules.
#[must_use]
pub fn get_default_anthropic_auto_mode_rules() -> AutoModeRules {
    AutoModeRules::anthropic()
}

/// Classify a tool use for auto-permission decisions.
///
/// Takes a tool name, its JSON input, and the active auto mode rules,
/// and returns whether to allow, deny, or ask the user.
pub fn classify_tool_use(
    tool_name: &str,
    tool_input: &serde_json::Value,
    rules: &AutoModeRules,
) -> YoloClassifierResult {
    // Check if the tool itself is a known read-only tool
    if is_read_only_tool(tool_name) {
        return YoloClassifierResult::Allow;
    }

    // For Bash/Shell tools, classify the command
    if (tool_name == "Bash" || tool_name == "Shell" || tool_name == "bash" || tool_name == "shell")
        && let Some(command) = tool_input.get("command").and_then(|v| v.as_str())
    {
        return classify_bash_in_yolo(command, rules);
    }

    // For file write tools, ask for confirmation
    if is_write_tool(tool_name) {
        if let Some(path) = extract_file_path(tool_input) {
            // Allow writes to non-critical paths
            if is_safe_write_path(&path) {
                return YoloClassifierResult::Allow;
            }
            return YoloClassifierResult::Ask(format!(
                "Write operation to '{path}' requires confirmation"
            ));
        }
        return YoloClassifierResult::Ask("File write operation requires confirmation".into());
    }

    // For MCP tools, ask by default
    if tool_name.starts_with("mcp__") {
        return YoloClassifierResult::Ask(format!(
            "MCP tool '{tool_name}' requires user confirmation"
        ));
    }

    // Default: ask for unknown tools
    YoloClassifierResult::Ask(format!(
        "Unknown tool '{tool_name}' requires user confirmation"
    ))
}

/// Apply auto mode rules to a tool invocation.
///
/// This is the main entry point for the auto-mode permission system.
pub fn apply_auto_mode_rules(
    tool_name: &str,
    tool_input: &serde_json::Value,
    rules: &AutoModeRules,
) -> YoloClassifierResult {
    classify_tool_use(tool_name, tool_input, rules)
}

/// Classify a bash command within the YOLO classifier context.
fn classify_bash_in_yolo(command: &str, rules: &AutoModeRules) -> YoloClassifierResult {
    let trimmed = command.trim();

    // Check soft-deny patterns first (safety first)
    if rules.is_soft_denied(trimmed) {
        return YoloClassifierResult::Deny(format!(
            "Command matches a dangerous pattern: '{trimmed}'"
        ));
    }

    // Pipe chains and redirections need review (check BEFORE allow patterns)
    if trimmed.contains('|') || trimmed.contains('>') || trimmed.contains(">>") {
        return YoloClassifierResult::Ask(format!(
            "Pipe/redirect in command requires review: '{trimmed}'"
        ));
    }

    // Commands with variable expansion need review (check BEFORE allow patterns)
    if trimmed.contains("$(") || trimmed.contains('`') {
        return YoloClassifierResult::Ask(format!(
            "Command substitution requires review: '{trimmed}'"
        ));
    }

    // Check allow patterns
    if rules.is_allowed(trimmed) {
        return YoloClassifierResult::Allow;
    }

    // Default: ask for unrecognized commands
    YoloClassifierResult::Ask(format!("Unrecognized command requires review: '{trimmed}'"))
}

/// Check if a tool is known to be read-only.
fn is_read_only_tool(tool_name: &str) -> bool {
    is_auto_mode_allowlisted_tool(tool_name)
}

/// Full safe-tools allowlist matching TS `isAutoModeAllowlistedTool`.
///
/// These tools bypass the LLM classifier entirely — they're always safe.
fn is_auto_mode_allowlisted_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        // Read tools
        "Read" | "read" | "ReadFile" | "read_file" | "ReadMultipleFiles" | "read_multiple_files"
        // Search tools
        | "Grep" | "grep" | "Glob" | "glob" | "SearchFiles" | "search_files"
        | "CodebaseSearch" | "codebase_search"
        // Directory listing
        | "LS" | "ls" | "ListFiles" | "list_files" | "ListDir" | "list_dir"
        | "DirectoryTree" | "directory_tree"
        // Web tools
        | "WebFetch" | "web_fetch" | "WebSearch" | "web_search"
        // MCP read tools
        | "ListDocuments" | "list_documents" | "ListMcpResourcesTool" | "list_mcp_resources"
        | "ReadMcpResourceTool" | "read_mcp_resource"
        // LSP tools
        | "GetDiagnostics" | "get_diagnostics" | "GetTypeInfo" | "get_type_info"
        | "Hover" | "hover" | "GoToDefinition" | "go_to_definition"
        | "References" | "references" | "DocumentSymbols" | "document_symbols"
        // Utility tools
        | "Sticker" | "sticker" | "TodoRead" | "todo_read"
        | "StatusLine" | "statusline" | "Task" | "task"
        | "NotebookEdit" | "notebook_edit"
        // Search / listing aliases
        | "list_directory" | "directory_search" | "file_search" | "search"
    )
}

/// Check if a tool performs write operations.
fn is_write_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Write" | "Edit" | "write" | "edit" | "write_to_file" | "apply_diff" | "create_file"
    )
}

/// Extract a file path from tool input JSON.
fn extract_file_path(tool_input: &serde_json::Value) -> Option<String> {
    tool_input
        .get("path")
        .or_else(|| tool_input.get("file_path"))
        .or_else(|| tool_input.get("filePath"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned())
}

/// Check if a file path is considered safe for writes.
fn is_safe_write_path(path: &str) -> bool {
    // Block writes to critical system paths
    let blocked_prefixes = [
        "/etc/",
        "/usr/",
        "/bin/",
        "/sbin/",
        "/System/",
        "/Library/System/",
        "C:\\Windows\\",
        "C:\\Program Files\\",
        "C:\\Program Files (x86)\\",
    ];

    for prefix in &blocked_prefixes {
        if path.starts_with(prefix) {
            return false;
        }
    }

    // Block writes to hidden files in home directory
    if path.starts_with("~/.") || path.contains("/.") {
        // Allow .remote-code-rust project files
        if path.contains(".remote-code-rust") || path.contains(".remote-code") {
            return true;
        }
        return false;
    }

    true
}

// ---------------------------------------------------------------------------
// 2-Stage LLM-based YOLO Classifier
// ---------------------------------------------------------------------------

/// System prompt for the 2-stage LLM classifier.
/// Mirrors TS `yoloClassifier.ts` system prompt with XML output format.
pub const YOLO_CLASSIFIER_SYSTEM_PROMPT: &str = r#"You are a security classifier for tool use in an AI coding assistant. Your job is to determine whether a tool invocation is safe to auto-approve.

Respond with an XML block:
<block>yes</block> — if the operation is safe to auto-approve
<block>no</block> — if the operation should be blocked or requires user confirmation

Guidelines:
- Reading files, searching code, and listing directories are always safe
- Writing to files within the project directory is generally safe
- Running build commands, tests, and linters is generally safe
- Commands that delete files, force-push, modify system configs, or access credentials are unsafe
- When in doubt, err on the side of blocking (fail-closed)
- Never execute the command yourself — you are only classifying it
- You MUST respond with a <block> tag containing exactly "yes" or "no""#;

/// XML suffix for Stage 1 (fast) classification prompt.
pub const XML_S1_SUFFIX: &str =
    "\n\nErr on the side of blocking. Respond with <block>yes</block> or <block>no</block>.";

/// XML suffix for Stage 2 (thinking) classification prompt.
pub const XML_S2_SUFFIX: &str = "\n\nThink carefully about the security implications. Respond with <block>yes</block> or <block>no</block>.";

// ---------------------------------------------------------------------------
// Transcript types for classifier context
// ---------------------------------------------------------------------------

/// A single block within a transcript entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TranscriptBlock {
    /// Text content.
    Text { text: String },
    /// Tool use block.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool result block.
    ToolResult {
        id: String,
        content: String,
        is_error: bool,
    },
    /// Thinking block (stripped before sending to classifier).
    Thinking { text: String },
}

/// A single entry in the conversation transcript for classifier context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// The role of the message author.
    pub role: String,
    /// Content blocks.
    pub blocks: Vec<TranscriptBlock>,
}

/// Build transcript entries from conversation messages.
///
/// Mirrors TS `buildTranscriptEntries`. Converts messages into a simplified
/// format for the classifier LLM context, stripping thinking blocks.
pub fn build_transcript_entries(messages: &[serde_json::Value]) -> Vec<TranscriptEntry> {
    let mut entries = Vec::new();

    for msg in messages {
        let role = msg
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let mut blocks = Vec::new();

        // Handle content as either a string or array of blocks
        if let Some(content) = msg.get("content") {
            if let Some(text) = content.as_str() {
                blocks.push(TranscriptBlock::Text {
                    text: text.to_owned(),
                });
            } else if let Some(content_arr) = content.as_array() {
                for block in content_arr {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");

                    match block_type {
                        "text" => {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                blocks.push(TranscriptBlock::Text {
                                    text: text.to_owned(),
                                });
                            }
                        }
                        "tool_use" => {
                            blocks.push(TranscriptBlock::ToolUse {
                                id: block
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_owned(),
                                name: block
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_owned(),
                                input: block
                                    .get("input")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null),
                            });
                        }
                        "tool_result" => {
                            blocks.push(TranscriptBlock::ToolResult {
                                id: block
                                    .get("tool_use_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_owned(),
                                content: block
                                    .get("content")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_owned(),
                                is_error: block
                                    .get("is_error")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false),
                            });
                        }
                        "thinking" => {
                            // Skip thinking blocks for classifier context
                        }
                        _ => {}
                    }
                }
            }
        }

        if !blocks.is_empty() {
            entries.push(TranscriptEntry {
                role: role.to_owned(),
                blocks,
            });
        }
    }

    entries
}

/// Build the transcript XML string for the classifier prompt.
///
/// Mirrors TS `buildTranscriptForClassifier`. Creates a compact XML
/// representation of the conversation for the classifier to reason about.
pub fn build_transcript_for_classifier(
    tool_name: &str,
    tool_input: &serde_json::Value,
    entries: &[TranscriptEntry],
) -> String {
    let mut xml = String::from("<transcript>\n");

    for entry in entries {
        xml.push_str(&format!("  <{role}>\n", role = entry.role));
        for block in &entry.blocks {
            match block {
                TranscriptBlock::Text { text } => {
                    let truncated = if text.len() > 500 { &text[..500] } else { text };
                    xml.push_str(&format!("    <text>{truncated}</text>\n"));
                }
                TranscriptBlock::ToolUse { name, input, .. } => {
                    let input_str = serde_json::to_string(input).unwrap_or_default();
                    let truncated = if input_str.len() > 300 {
                        &input_str[..300]
                    } else {
                        &input_str
                    };
                    xml.push_str(&format!(
                        "    <tool_use name=\"{name}\">{truncated}</tool_use>\n"
                    ));
                }
                TranscriptBlock::ToolResult {
                    content, is_error, ..
                } => {
                    let tag = if *is_error {
                        "tool_error"
                    } else {
                        "tool_result"
                    };
                    let truncated = if content.len() > 300 {
                        &content[..300]
                    } else {
                        content
                    };
                    xml.push_str(&format!("    <{tag}>{truncated}</{tag}>\n",));
                }
                TranscriptBlock::Thinking { .. } => {
                    // Stripped — never included in classifier context
                }
            }
        }
        xml.push_str(&format!("  </{role}>\n", role = entry.role));
    }

    // Append the tool invocation being classified
    let input_str = serde_json::to_string(tool_input).unwrap_or_default();
    let truncated_input = if input_str.len() > 500 {
        &input_str[..500]
    } else {
        &input_str
    };
    xml.push_str(&format!(
        "  <classify tool=\"{tool_name}\">{truncated_input}</classify>\n"
    ));
    xml.push_str("</transcript>");

    xml
}

// ---------------------------------------------------------------------------
// Full classifier result types
// ---------------------------------------------------------------------------

/// Which stage produced the classification result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassifierStage {
    /// Stage 1 — fast, max_tokens=64, stop on </block>.
    Fast,
    /// Stage 2 — thinking, max_tokens=4096, full chain-of-thought.
    Thinking,
}

/// Token usage from the classifier LLM call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClassifierUsage {
    /// Input tokens consumed.
    pub input_tokens: u64,
    /// Output tokens generated.
    pub output_tokens: u64,
}

/// Full result from the 2-stage LLM classifier.
#[derive(Debug, Clone)]
pub struct YoloClassifierFullResult {
    /// The classification decision.
    pub decision: YoloClassifierResult,
    /// Which stage produced the final decision.
    pub stage: ClassifierStage,
    /// Token usage from the LLM call(s).
    pub usage: ClassifierUsage,
    /// The thinking/reasoning text from Stage 2 (if available).
    pub thinking: Option<String>,
}

/// Request for a single LLM classification call.
#[derive(Debug, Clone)]
pub struct YoloLlmRequest {
    /// The tool name being classified.
    pub tool_name: String,
    /// The JSON input of the tool call.
    pub tool_input: serde_json::Value,
    /// Conversation context as XML transcript.
    pub context: String,
    /// Maximum tokens for the response.
    pub max_tokens: u32,
    /// Optional stop sequences.
    pub stop_sequences: Vec<String>,
    /// Suffix hint appended to the prompt (XML suffix).
    pub suffix_hint: Option<String>,
    /// Which classifier stage this request is for.
    pub stage: ClassifierStage,
}

/// Response from a single LLM classification call.
#[derive(Debug, Clone)]
pub struct YoloLlmResponse {
    /// The raw text output from the LLM.
    pub text: String,
}

/// Trait for LLM-backed classification. Implemented by the query engine layer.
pub trait YoloLlmClient: Send + Sync {
    fn classify(
        &self,
        request: YoloLlmRequest,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<YoloLlmResponse>> + Send + '_>>;
}

/// 2-stage LLM-based YOLO classifier.
///
/// Stage 1 ("fast"): max_tokens=64, stop_sequences=['</block>'], parses `<block>yes|no</block>`.
/// If allow, returns immediately. If block/unparseable, escalates to Stage 2.
///
/// Stage 2 ("thinking"): max_tokens=4096, no stop sequences, full chain-of-thought.
/// Both share the same system prompt with XML output format.
pub struct YoloLlmClassifier {
    client: Option<Box<dyn YoloLlmClient>>,
}

impl YoloLlmClassifier {
    /// Create a new LLM classifier with an optional client.
    /// When no client is provided, classification falls back to rule-based only.
    pub fn new(client: Option<Box<dyn YoloLlmClient>>) -> Self {
        Self { client }
    }

    /// Classify a tool use through the 2-stage LLM pipeline.
    ///
    /// First checks rule-based bypasses (safe tools allowlist), then
    /// invokes the 2-stage LLM classifier if a client is available.
    /// Returns `None` if no LLM client is configured (caller should
    /// fall back to rule-based classification).
    pub async fn classify(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        context: &str,
    ) -> Option<YoloClassifierResult> {
        let client = self.client.as_ref()?;

        // Safe tools bypass the classifier entirely
        if is_read_only_tool(tool_name) {
            return Some(YoloClassifierResult::Allow);
        }

        // Stage 1: Fast classification
        let stage1_request = YoloLlmRequest {
            tool_name: tool_name.to_owned(),
            tool_input: tool_input.clone(),
            context: context.to_owned(),
            max_tokens: 64,
            stop_sequences: vec!["</block>".to_owned()],
            suffix_hint: Some(XML_S1_SUFFIX.to_owned()),
            stage: ClassifierStage::Fast,
        };

        let stage1_response = client.classify(stage1_request).await.ok()?;
        let stage1_text = strip_thinking_tags(&stage1_response.text);

        if let Some(decision) = parse_block_xml(&stage1_text) {
            match decision {
                BlockDecision::Allow => return Some(YoloClassifierResult::Allow),
                BlockDecision::Block => {
                    // Escalate to stage 2 for reasoning
                }
            }
        }

        // Stage 2: Thinking classification (full chain-of-thought)
        let stage2_request = YoloLlmRequest {
            tool_name: tool_name.to_owned(),
            tool_input: tool_input.clone(),
            context: context.to_owned(),
            max_tokens: 4096,
            stop_sequences: vec![],
            suffix_hint: Some(XML_S2_SUFFIX.to_owned()),
            stage: ClassifierStage::Thinking,
        };

        let stage2_response = match client.classify(stage2_request).await {
            Ok(resp) => resp,
            Err(_) => {
                // Fail-closed: if stage 2 fails, block
                return Some(YoloClassifierResult::Deny(
                    "LLM classifier stage 2 failed — fail-closed".to_owned(),
                ));
            }
        };

        let stage2_text = strip_thinking_tags(&stage2_response.text);

        // Parse stage 2 response
        if let Some(decision) = parse_block_xml(&stage2_text) {
            match decision {
                BlockDecision::Allow => return Some(YoloClassifierResult::Allow),
                BlockDecision::Block => {
                    return Some(YoloClassifierResult::Deny(
                        "LLM classifier blocked the operation".to_owned(),
                    ));
                }
            }
        }

        // Fail-closed: unparseable response → block
        Some(YoloClassifierResult::Deny(
            "LLM classifier returned unparseable response — fail-closed".to_owned(),
        ))
    }

    /// Full classification returning detailed result with stage and usage.
    pub async fn classify_full(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        context: &str,
    ) -> Option<YoloClassifierFullResult> {
        let client = self.client.as_ref()?;

        // Safe tools bypass
        if is_read_only_tool(tool_name) {
            return Some(YoloClassifierFullResult {
                decision: YoloClassifierResult::Allow,
                stage: ClassifierStage::Fast,
                usage: ClassifierUsage::default(),
                thinking: None,
            });
        }

        // Stage 1
        let stage1_request = YoloLlmRequest {
            tool_name: tool_name.to_owned(),
            tool_input: tool_input.clone(),
            context: context.to_owned(),
            max_tokens: 64,
            stop_sequences: vec!["</block>".to_owned()],
            suffix_hint: Some(XML_S1_SUFFIX.to_owned()),
            stage: ClassifierStage::Fast,
        };

        if let Ok(stage1_response) = client.classify(stage1_request).await {
            let stage1_text = strip_thinking_tags(&stage1_response.text);
            if let Some(BlockDecision::Allow) = parse_block_xml(&stage1_text) {
                return Some(YoloClassifierFullResult {
                    decision: YoloClassifierResult::Allow,
                    stage: ClassifierStage::Fast,
                    usage: ClassifierUsage::default(),
                    thinking: None,
                });
            }
        }

        // Stage 2
        let stage2_request = YoloLlmRequest {
            tool_name: tool_name.to_owned(),
            tool_input: tool_input.clone(),
            context: context.to_owned(),
            max_tokens: 4096,
            stop_sequences: vec![],
            suffix_hint: Some(XML_S2_SUFFIX.to_owned()),
            stage: ClassifierStage::Thinking,
        };

        match client.classify(stage2_request).await {
            Ok(stage2_response) => {
                let stage2_text = strip_thinking_tags(&stage2_response.text);
                let thinking = extract_thinking_text(&stage2_response.text);

                if let Some(decision) = parse_block_xml(&stage2_text) {
                    let result = match decision {
                        BlockDecision::Allow => YoloClassifierResult::Allow,
                        BlockDecision::Block => YoloClassifierResult::Deny(
                            "LLM classifier blocked the operation".to_owned(),
                        ),
                    };
                    Some(YoloClassifierFullResult {
                        decision: result,
                        stage: ClassifierStage::Thinking,
                        usage: ClassifierUsage::default(),
                        thinking,
                    })
                } else {
                    Some(YoloClassifierFullResult {
                        decision: YoloClassifierResult::Deny(
                            "LLM classifier returned unparseable response — fail-closed".to_owned(),
                        ),
                        stage: ClassifierStage::Thinking,
                        usage: ClassifierUsage::default(),
                        thinking,
                    })
                }
            }
            Err(_) => Some(YoloClassifierFullResult {
                decision: YoloClassifierResult::Deny(
                    "LLM classifier stage 2 failed — fail-closed".to_owned(),
                ),
                stage: ClassifierStage::Thinking,
                usage: ClassifierUsage::default(),
                thinking: None,
            }),
        }
    }
}

/// Parsed decision from XML block output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockDecision {
    Allow,
    Block,
}

/// Strip `<thinking>...</thinking>` tags from LLM output before XML parsing.
///
/// Mirrors TS `stripThinking` — removes thinking blocks so the `<block>` parser
/// doesn't get confused by content inside thinking tags.
fn strip_thinking_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut depth = 0i32;
    let mut i = 0;
    let bytes = text.as_bytes();

    while i < bytes.len() {
        if text[i..].starts_with("<thinking>") {
            depth += 1;
            i += "<thinking>".len();
        } else if depth > 0 && text[i..].starts_with("</thinking>") {
            depth -= 1;
            i += "</thinking>".len();
        } else if depth == 0 {
            result.push(bytes[i] as char);
            i += 1;
        } else {
            i += 1;
        }
    }

    result
}

/// Extract the text content from `<thinking>...</thinking>` blocks.
fn extract_thinking_text(text: &str) -> Option<String> {
    let mut thinking = String::new();
    let mut start = 0;

    while let Some(open_idx) = text[start..].find("<thinking>") {
        let abs_open = start + open_idx + "<thinking>".len();
        if let Some(close_idx) = text[abs_open..].find("</thinking>") {
            if !thinking.is_empty() {
                thinking.push('\n');
            }
            thinking.push_str(text[abs_open..abs_open + close_idx].trim());
            start = abs_open + close_idx + "</thinking>".len();
        } else {
            // Unclosed thinking tag — take everything after it
            if !thinking.is_empty() {
                thinking.push('\n');
            }
            thinking.push_str(text[abs_open..].trim());
            break;
        }
    }

    if thinking.is_empty() {
        None
    } else {
        Some(thinking)
    }
}

/// Parse `<block>yes</block>` or `<block>no</block>` from LLM output.
///
/// Handles:
/// - Case-insensitive matching
/// - Multiple blocks (uses last one)
/// - Unclosed blocks (looks for content after `<block>` until end of text)
/// - Whitespace in content
fn parse_block_xml(text: &str) -> Option<BlockDecision> {
    let lower = text.to_ascii_lowercase();

    // Find the last occurrence of <block>
    if let Some(idx) = lower.rfind("<block>") {
        let after_open = &lower[idx + 7..];
        // Try to find closing tag
        if let Some(close_idx) = after_open.find("</block>") {
            let content = after_open[..close_idx].trim();
            return match content {
                "yes" | "allow" | "safe" | "true" => Some(BlockDecision::Allow),
                "no" | "block" | "deny" | "unsafe" | "false" => Some(BlockDecision::Block),
                _ => None,
            };
        }

        // Handle unclosed block — take content until newline or end
        let content = after_open.lines().next().unwrap_or("").trim();
        if !content.is_empty() {
            return match content {
                "yes" | "allow" | "safe" | "true" => Some(BlockDecision::Allow),
                "no" | "block" | "deny" | "unsafe" | "false" => Some(BlockDecision::Block),
                _ => None,
            };
        }
    }

    None
}

/// Resolve the model to use for the YOLO classifier.
///
/// Mirrors TS model selection: env var → GrowthBook → main loop model.
pub fn resolve_yolo_model(main_model: &str) -> String {
    // 1. Check env var override
    if let Ok(model) = std::env::var("CLAUDE_CODE_AUTO_MODE_MODEL")
        && !model.is_empty()
    {
        return model;
    }

    // 2. Fall back to main loop model
    // (GrowthBook feature flag would be checked here in the full impl)
    main_model.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn yolo_classifier_result_allow_equality() {
        assert_eq!(YoloClassifierResult::Allow, YoloClassifierResult::Allow);
    }

    #[test]
    fn yolo_classifier_result_deny_equality() {
        assert_eq!(
            YoloClassifierResult::Deny("reason".into()),
            YoloClassifierResult::Deny("reason".into())
        );
    }

    #[test]
    fn yolo_classifier_result_ask_equality() {
        assert_eq!(
            YoloClassifierResult::Ask("prompt".into()),
            YoloClassifierResult::Ask("prompt".into())
        );
    }

    #[test]
    fn read_only_tools_are_allowed() {
        let rules = AutoModeRules::external();
        for tool in &["Read", "Grep", "Glob", "LS", "WebFetch"] {
            let result = classify_tool_use(tool, &json!({}), &rules);
            assert!(
                matches!(result, YoloClassifierResult::Allow),
                "Expected Allow for tool '{tool}', got {result:?}"
            );
        }
    }

    #[test]
    fn safe_git_commands_are_allowed() {
        let rules = AutoModeRules::external();
        let result = classify_tool_use("Bash", &json!({"command": "git status"}), &rules);
        assert!(matches!(result, YoloClassifierResult::Allow));
    }

    #[test]
    fn dangerous_commands_are_denied() {
        let rules = AutoModeRules::external();
        let result = classify_tool_use("Bash", &json!({"command": "rm -rf /"}), &rules);
        assert!(matches!(result, YoloClassifierResult::Deny(_)));
    }

    #[test]
    fn sudo_commands_are_denied() {
        let rules = AutoModeRules::external();
        let result = classify_tool_use(
            "Bash",
            &json!({"command": "sudo apt install something"}),
            &rules,
        );
        assert!(matches!(result, YoloClassifierResult::Deny(_)));
    }

    #[test]
    fn force_push_is_denied() {
        let rules = AutoModeRules::external();
        let result = classify_tool_use(
            "Bash",
            &json!({"command": "git push --force origin main"}),
            &rules,
        );
        assert!(matches!(result, YoloClassifierResult::Deny(_)));
    }

    #[test]
    fn unknown_commands_ask() {
        let rules = AutoModeRules::external();
        let result = classify_tool_use(
            "Bash",
            &json!({"command": "some-unknown-tool arg1 arg2"}),
            &rules,
        );
        assert!(matches!(result, YoloClassifierResult::Ask(_)));
    }

    #[test]
    fn pipe_commands_ask() {
        let rules = AutoModeRules::external();
        let result = classify_tool_use(
            "Bash",
            &json!({"command": "cat file.txt | grep pattern"}),
            &rules,
        );
        assert!(matches!(result, YoloClassifierResult::Ask(_)));
    }

    #[test]
    fn write_tool_to_safe_path_is_allowed() {
        let rules = AutoModeRules::external();
        let result = classify_tool_use(
            "Write",
            &json!({"path": "/home/user/project/src/main.rs"}),
            &rules,
        );
        assert!(matches!(result, YoloClassifierResult::Allow));
    }

    #[test]
    fn write_tool_to_system_path_is_asked() {
        let rules = AutoModeRules::external();
        let result = classify_tool_use("Write", &json!({"path": "/etc/passwd"}), &rules);
        assert!(matches!(result, YoloClassifierResult::Ask(_)));
    }

    #[test]
    fn mcp_tools_ask() {
        let rules = AutoModeRules::external();
        let result = classify_tool_use(
            "mcp__filesystem__read",
            &json!({"path": "/tmp/test"}),
            &rules,
        );
        assert!(matches!(result, YoloClassifierResult::Ask(_)));
    }

    #[test]
    fn anthropic_rules_allow_more_operations() {
        let rules = AutoModeRules::anthropic();
        let result = classify_tool_use("Bash", &json!({"command": "git add src/main.rs"}), &rules);
        assert!(matches!(result, YoloClassifierResult::Allow));
    }

    #[test]
    fn external_rules_deny_git_add() {
        let rules = AutoModeRules::external();
        let result = classify_tool_use("Bash", &json!({"command": "git add src/main.rs"}), &rules);
        // git add is not in external allow list, so it should ask
        assert!(matches!(result, YoloClassifierResult::Ask(_)));
    }

    #[test]
    fn auto_mode_rules_is_allowed() {
        let rules = AutoModeRules::external();
        assert!(rules.is_allowed("git status"));
        assert!(rules.is_allowed("cargo build"));
        assert!(rules.is_allowed("ls"));
        assert!(!rules.is_allowed("rm -rf /"));
    }

    #[test]
    fn auto_mode_rules_is_soft_denied() {
        let rules = AutoModeRules::external();
        assert!(rules.is_soft_denied("rm -rf /"));
        assert!(rules.is_soft_denied("sudo something"));
        assert!(!rules.is_soft_denied("git status"));
    }

    #[test]
    fn auto_mode_rules_env_allowed() {
        let rules = AutoModeRules::external();
        assert!(rules.is_env_allowed("PATH"));
        assert!(rules.is_env_allowed("HOME"));
        assert!(rules.is_env_allowed("Cargo_Home")); // case-insensitive
        assert!(!rules.is_env_allowed("AWS_SECRET_ACCESS_KEY"));
    }

    #[test]
    fn command_substitution_asks() {
        let rules = AutoModeRules::external();
        let result = classify_tool_use(
            "Bash",
            &json!({"command": "echo $(cat /etc/passwd)"}),
            &rules,
        );
        assert!(matches!(result, YoloClassifierResult::Ask(_)));
    }

    #[test]
    fn apply_auto_mode_rules_delegates_to_classify() {
        let rules = AutoModeRules::external();
        let result = apply_auto_mode_rules("Read", &json!({"path": "/tmp/test"}), &rules);
        assert!(matches!(result, YoloClassifierResult::Allow));
    }

    #[test]
    fn default_rules_are_external() {
        let default = AutoModeRules::default();
        let external = AutoModeRules::external();
        assert_eq!(default.allow.len(), external.allow.len());
    }

    // ---- XML block parsing tests ----

    #[test]
    fn parse_block_xml_yes() {
        assert_eq!(
            parse_block_xml("<block>yes</block>"),
            Some(BlockDecision::Allow)
        );
    }

    #[test]
    fn parse_block_xml_no() {
        assert_eq!(
            parse_block_xml("<block>no</block>"),
            Some(BlockDecision::Block)
        );
    }

    #[test]
    fn parse_block_xml_with_reasoning() {
        let text = "Let me analyze this...\n<block>no</block>";
        assert_eq!(parse_block_xml(text), Some(BlockDecision::Block));
    }

    #[test]
    fn parse_block_xml_case_insensitive() {
        assert_eq!(
            parse_block_xml("<Block>YES</Block>"),
            Some(BlockDecision::Allow)
        );
    }

    #[test]
    fn parse_block_xml_invalid() {
        assert_eq!(parse_block_xml("no xml here"), None);
    }

    #[test]
    fn parse_block_xml_empty() {
        assert_eq!(parse_block_xml("<block></block>"), None);
    }

    #[test]
    fn parse_block_xml_trailing() {
        // Only last <block> matters
        let text = "<block>yes</block> actually wait <block>no</block>";
        assert_eq!(parse_block_xml(text), Some(BlockDecision::Block));
    }

    #[test]
    fn parse_block_xml_allow_aliases() {
        assert_eq!(
            parse_block_xml("<block>allow</block>"),
            Some(BlockDecision::Allow)
        );
        assert_eq!(
            parse_block_xml("<block>safe</block>"),
            Some(BlockDecision::Allow)
        );
    }

    #[test]
    fn parse_block_xml_block_aliases() {
        assert_eq!(
            parse_block_xml("<block>block</block>"),
            Some(BlockDecision::Block)
        );
        assert_eq!(
            parse_block_xml("<block>deny</block>"),
            Some(BlockDecision::Block)
        );
        assert_eq!(
            parse_block_xml("<block>unsafe</block>"),
            Some(BlockDecision::Block)
        );
    }

    #[test]
    fn parse_block_xml_unclosed_block() {
        assert_eq!(parse_block_xml("<block>yes"), Some(BlockDecision::Allow));
        assert_eq!(parse_block_xml("<block>no"), Some(BlockDecision::Block));
    }

    #[test]
    fn parse_block_xml_true_false() {
        assert_eq!(
            parse_block_xml("<block>true</block>"),
            Some(BlockDecision::Allow)
        );
        assert_eq!(
            parse_block_xml("<block>false</block>"),
            Some(BlockDecision::Block)
        );
    }

    #[test]
    fn strip_thinking_tags_removes_thinking() {
        let input = "Some reasoning <thinking>internal thoughts</thinking> <block>yes</block>";
        let stripped = strip_thinking_tags(input);
        assert!(!stripped.contains("thinking"));
        assert!(!stripped.contains("internal thoughts"));
        assert!(stripped.contains("<block>yes</block>"));
    }

    #[test]
    fn strip_thinking_tags_nested() {
        let input = "<thinking>thought 1</thinking>text<thinking>thought 2</thinking>";
        let stripped = strip_thinking_tags(input);
        assert_eq!(stripped, "text");
    }

    #[test]
    fn strip_thinking_tags_no_thinking() {
        let input = "<block>yes</block>";
        assert_eq!(strip_thinking_tags(input), "<block>yes</block>");
    }

    #[test]
    fn extract_thinking_text_works() {
        let input = "<thinking>reasoning here</thinking><block>no</block>";
        let thinking = extract_thinking_text(input);
        assert_eq!(thinking, Some("reasoning here".to_owned()));
    }

    #[test]
    fn extract_thinking_text_multiple() {
        let input = "<thinking>part 1</thinking>middle<thinking>part 2</thinking>";
        let thinking = extract_thinking_text(input);
        assert!(thinking.is_some());
        let t = thinking.expect("thinking text should be extracted");
        assert!(t.contains("part 1"));
        assert!(t.contains("part 2"));
    }

    #[test]
    fn extract_thinking_text_none() {
        assert_eq!(extract_thinking_text("<block>yes</block>"), None);
    }

    #[test]
    fn expanded_allowlist_includes_all_safe_tools() {
        // Read tools
        assert!(is_auto_mode_allowlisted_tool("Read"));
        assert!(is_auto_mode_allowlisted_tool("ReadFile"));
        assert!(is_auto_mode_allowlisted_tool("read_file"));
        assert!(is_auto_mode_allowlisted_tool("ReadMultipleFiles"));
        // Search tools
        assert!(is_auto_mode_allowlisted_tool("Grep"));
        assert!(is_auto_mode_allowlisted_tool("Glob"));
        assert!(is_auto_mode_allowlisted_tool("SearchFiles"));
        assert!(is_auto_mode_allowlisted_tool("CodebaseSearch"));
        // Listing tools
        assert!(is_auto_mode_allowlisted_tool("LS"));
        assert!(is_auto_mode_allowlisted_tool("ListFiles"));
        assert!(is_auto_mode_allowlisted_tool("ListDir"));
        assert!(is_auto_mode_allowlisted_tool("DirectoryTree"));
        // Web tools
        assert!(is_auto_mode_allowlisted_tool("WebFetch"));
        assert!(is_auto_mode_allowlisted_tool("WebSearch"));
        // MCP read tools
        assert!(is_auto_mode_allowlisted_tool("ListMcpResourcesTool"));
        assert!(is_auto_mode_allowlisted_tool("ReadMcpResourceTool"));
        // LSP tools
        assert!(is_auto_mode_allowlisted_tool("GetDiagnostics"));
        assert!(is_auto_mode_allowlisted_tool("GetTypeInfo"));
        assert!(is_auto_mode_allowlisted_tool("Hover"));
        assert!(is_auto_mode_allowlisted_tool("GoToDefinition"));
        assert!(is_auto_mode_allowlisted_tool("References"));
        assert!(is_auto_mode_allowlisted_tool("DocumentSymbols"));
        // Utility tools
        assert!(is_auto_mode_allowlisted_tool("TodoRead"));
        assert!(is_auto_mode_allowlisted_tool("NotebookEdit"));
        // Not safe
        assert!(!is_auto_mode_allowlisted_tool("Bash"));
        assert!(!is_auto_mode_allowlisted_tool("Write"));
        assert!(!is_auto_mode_allowlisted_tool("Edit"));
    }

    #[test]
    fn build_transcript_entries_parses_messages() {
        let messages = vec![
            serde_json::json!({
                "role": "user",
                "content": "Hello"
            }),
            serde_json::json!({
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will help"},
                    {"type": "tool_use", "id": "tu1", "name": "Read", "input": {"path": "src/lib.rs"}},
                    {"type": "thinking", "thinking": "internal reasoning"}
                ]
            }),
        ];
        let entries = build_transcript_entries(&messages);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].role, "user");
        assert_eq!(entries[1].role, "assistant");
        // Thinking block should be stripped
        assert_eq!(entries[1].blocks.len(), 2); // text + tool_use, no thinking
    }

    #[test]
    fn build_transcript_for_classifier_produces_xml() {
        let entries = vec![TranscriptEntry {
            role: "user".to_owned(),
            blocks: vec![TranscriptBlock::Text {
                text: "Hello".to_owned(),
            }],
        }];
        let xml = build_transcript_for_classifier(
            "Read",
            &serde_json::json!({"path": "src/lib.rs"}),
            &entries,
        );
        assert!(xml.contains("<transcript>"));
        assert!(xml.contains("<user>"));
        assert!(xml.contains("<text>Hello</text>"));
        assert!(xml.contains("<classify tool=\"Read\">"));
        assert!(xml.contains("</transcript>"));
    }

    #[test]
    fn xml_suffixes_are_nonempty() {
        assert!(!XML_S1_SUFFIX.is_empty());
        assert!(!XML_S2_SUFFIX.is_empty());
        assert!(XML_S1_SUFFIX.contains("Err on the side"));
        assert!(XML_S2_SUFFIX.contains("Think carefully"));
    }

    #[test]
    fn classifier_stage_serialization() {
        let fast =
            serde_json::to_string(&ClassifierStage::Fast).expect("serialize fast classifier stage");
        let thinking = serde_json::to_string(&ClassifierStage::Thinking)
            .expect("serialize thinking classifier stage");
        assert!(fast.contains("Fast") || fast.contains("fast"));
        assert!(thinking.contains("Thinking") || thinking.contains("thinking"));
    }

    #[test]
    fn resolve_yolo_model_defaults_to_main() {
        // Clear env var to ensure no override
        #[allow(unsafe_code)]
        unsafe {
            std::env::remove_var("CLAUDE_CODE_AUTO_MODE_MODEL");
        }
        assert_eq!(resolve_yolo_model("claude-sonnet-4-6"), "claude-sonnet-4-6");
    }

    #[test]
    fn yolo_llm_classifier_no_client_returns_none() {
        let classifier = YoloLlmClassifier::new(None);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(classifier.classify("Bash", &json!({"command": "echo hi"}), ""));
        assert!(result.is_none(), "without client, should return None");
    }

    #[test]
    fn yolo_llm_classifier_read_only_bypasses_llm() {
        struct MockClient;
        impl YoloLlmClient for MockClient {
            fn classify(
                &self,
                _request: YoloLlmRequest,
            ) -> Pin<Box<dyn Future<Output = anyhow::Result<YoloLlmResponse>> + Send + '_>>
            {
                // Should never be called for read-only tools
                Box::pin(async { panic!("should not call LLM for read-only tools") })
            }
        }

        let classifier = YoloLlmClassifier::new(Some(Box::new(MockClient)));
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(classifier.classify("Read", &json!({"path": "/tmp/test"}), ""));
        assert!(matches!(result, Some(YoloClassifierResult::Allow)));
    }

    #[test]
    fn yolo_llm_classifier_stage1_allow() {
        struct AllowClient;
        impl YoloLlmClient for AllowClient {
            fn classify(
                &self,
                request: YoloLlmRequest,
            ) -> Pin<Box<dyn Future<Output = anyhow::Result<YoloLlmResponse>> + Send + '_>>
            {
                let max_tokens = request.max_tokens;
                Box::pin(async move {
                    // Stage 1 (max_tokens=64) returns allow
                    if max_tokens == 64 {
                        Ok(YoloLlmResponse {
                            text: "<block>yes</block>".to_owned(),
                        })
                    } else {
                        panic!("stage 2 should not be called when stage 1 allows")
                    }
                })
            }
        }

        let classifier = YoloLlmClassifier::new(Some(Box::new(AllowClient)));
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(classifier.classify("Bash", &json!({"command": "echo hi"}), ""));
        assert!(matches!(result, Some(YoloClassifierResult::Allow)));
    }

    #[test]
    fn yolo_llm_classifier_stage2_block() {
        struct BlockClient;
        impl YoloLlmClient for BlockClient {
            fn classify(
                &self,
                request: YoloLlmRequest,
            ) -> Pin<Box<dyn Future<Output = anyhow::Result<YoloLlmResponse>> + Send + '_>>
            {
                let max_tokens = request.max_tokens;
                Box::pin(async move {
                    if max_tokens == 64 {
                        // Stage 1 returns block → escalate to stage 2
                        Ok(YoloLlmResponse {
                            text: "<block>no</block>".to_owned(),
                        })
                    } else {
                        // Stage 2 confirms block with reasoning
                        Ok(YoloLlmResponse {
                            text: "This command is dangerous because...\n<block>no</block>"
                                .to_owned(),
                        })
                    }
                })
            }
        }

        let classifier = YoloLlmClassifier::new(Some(Box::new(BlockClient)));
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(classifier.classify("Bash", &json!({"command": "rm -rf /"}), ""));
        assert!(matches!(result, Some(YoloClassifierResult::Deny(_))));
    }

    #[test]
    fn yolo_llm_classifier_fail_closed_on_unparseable() {
        struct UnparseableClient;
        impl YoloLlmClient for UnparseableClient {
            fn classify(
                &self,
                _request: YoloLlmRequest,
            ) -> Pin<Box<dyn Future<Output = anyhow::Result<YoloLlmResponse>> + Send + '_>>
            {
                Box::pin(async {
                    Ok(YoloLlmResponse {
                        text: "I cannot determine...".to_owned(),
                    })
                })
            }
        }

        let classifier = YoloLlmClassifier::new(Some(Box::new(UnparseableClient)));
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(classifier.classify("Bash", &json!({"command": "unknown"}), ""));
        assert!(matches!(result, Some(YoloClassifierResult::Deny(_))));
    }
}
