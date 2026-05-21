use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow};
use claude_core::{ConversationEntry, ConversationRole};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ToolResultSizePolicy;

pub const DEFAULT_MAX_RESULT_SIZE_CHARS: usize = 50_000;
pub const MAX_TOOL_RESULT_TOKENS: usize = 100_000;
pub const BYTES_PER_TOKEN: usize = 4;
pub const MAX_TOOL_RESULT_BYTES: usize = MAX_TOOL_RESULT_TOKENS * BYTES_PER_TOKEN;
pub const MAX_TOOL_RESULTS_PER_MESSAGE_CHARS: usize = 200_000;
pub const TOOL_RESULT_CLEARED_MESSAGE: &str = "[Old tool result content cleared]";
pub const TOOL_RESULTS_SUBDIR: &str = "tool-results";
pub const PERSISTED_OUTPUT_TAG: &str = "<persisted-output>";
pub const PERSISTED_OUTPUT_CLOSING_TAG: &str = "</persisted-output>";
pub const PREVIEW_SIZE_BYTES: usize = 2_000;

static NON_TEXT_CONTENT_ERROR: Lazy<String> =
    Lazy::new(|| "Cannot persist tool results containing non-text content".to_owned());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedToolResult {
    pub filepath: PathBuf,
    pub original_size: usize,
    pub is_json: bool,
    pub preview: String,
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentReplacementState {
    pub seen_ids: std::collections::HashSet<String>,
    pub replacements: std::collections::HashMap<String, String>,
}

impl ContentReplacementState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ContentReplacementKind {
    ToolResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContentReplacementRecord {
    pub kind: ContentReplacementKind,
    #[serde(rename = "toolUseId")]
    pub tool_use_id: String,
    pub replacement: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolResultBudgetOutcome {
    pub newly_replaced: Vec<ContentReplacementRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessedToolResultContent {
    pub content: String,
    pub content_blocks: Vec<Value>,
}

#[derive(Debug, Clone)]
struct ToolResultCandidate {
    tool_use_id: String,
    tool_name: String,
    content: CandidateContent,
    size: usize,
}

#[derive(Debug, Clone)]
enum CandidateContent {
    Text(String),
    Blocks(Vec<Value>),
}

impl CandidateContent {
    fn persist(&self, tool_use_id: &str, tool_results_dir: &Path) -> Result<PersistedToolResult> {
        match self {
            Self::Text(content) => persist_tool_result_text(content, tool_use_id, tool_results_dir),
            Self::Blocks(content_blocks) => {
                persist_tool_result_blocks(content_blocks, tool_use_id, tool_results_dir)
            }
        }
    }
}

pub fn session_tool_results_dir(session_dir: &Path) -> PathBuf {
    session_dir.join(TOOL_RESULTS_SUBDIR)
}

pub fn get_tool_result_path(tool_results_dir: &Path, id: &str, is_json: bool) -> PathBuf {
    tool_results_dir.join(format!("{id}.{}", if is_json { "json" } else { "txt" }))
}

pub fn ensure_tool_results_dir(tool_results_dir: &Path) -> Result<()> {
    fs::create_dir_all(tool_results_dir)
        .with_context(|| format!("failed to create {}", tool_results_dir.display()))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolResultsCleanupSummary {
    pub removed_files: usize,
    pub errors: usize,
}

pub fn cleanup_tool_results_for_session(
    session_dir: &Path,
    cutoff: SystemTime,
) -> ToolResultsCleanupSummary {
    let tool_results_dir = session_tool_results_dir(session_dir);
    cleanup_tool_results_dir(&tool_results_dir, cutoff)
}

pub fn cleanup_tool_results_dir(
    tool_results_dir: &Path,
    cutoff: SystemTime,
) -> ToolResultsCleanupSummary {
    let mut summary = ToolResultsCleanupSummary::default();
    let Ok(entries) = fs::read_dir(tool_results_dir) else {
        let _ = fs::remove_dir(session_parent(tool_results_dir));
        return summary;
    };

    for entry in entries {
        let Ok(entry) = entry else {
            summary.errors += 1;
            continue;
        };
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            summary.errors += 1;
            continue;
        };
        if file_type.is_file() {
            unlink_if_old(&path, cutoff, &mut summary);
        } else if file_type.is_dir() {
            cleanup_one_level_tool_dir(&path, cutoff, &mut summary);
            let _ = fs::remove_dir(&path);
        }
    }

    let _ = fs::remove_dir(tool_results_dir);
    let _ = fs::remove_dir(session_parent(tool_results_dir));
    summary
}

pub fn process_tool_result_text(
    content: &str,
    tool_use_id: &str,
    tool_results_dir: Option<&Path>,
    result_size_policy: ToolResultSizePolicy,
) -> Result<String> {
    process_tool_result_text_with_empty_message(
        content,
        tool_use_id,
        tool_results_dir,
        result_size_policy,
        "(tool completed with no output)",
    )
}

pub fn process_tool_result_content(
    content: &str,
    content_blocks: &[Value],
    tool_use_id: &str,
    tool_name: &str,
    tool_results_dir: Option<&Path>,
    result_size_policy: ToolResultSizePolicy,
) -> Result<ProcessedToolResultContent> {
    if content_blocks.is_empty() {
        return Ok(ProcessedToolResultContent {
            content: process_tool_result_text_with_empty_message(
                content,
                tool_use_id,
                tool_results_dir,
                result_size_policy,
                &empty_tool_result_message(tool_name),
            )?,
            content_blocks: Vec::new(),
        });
    }

    if content_blocks_are_empty(content_blocks) {
        return Ok(ProcessedToolResultContent {
            content: empty_tool_result_message(tool_name),
            content_blocks: Vec::new(),
        });
    }

    if !content_blocks_are_text_only(content_blocks) || content_blocks_have_image(content_blocks) {
        return Ok(ProcessedToolResultContent {
            content: content.to_owned(),
            content_blocks: content_blocks.to_vec(),
        });
    }

    let Some(limit) = persistence_threshold(result_size_policy) else {
        return Ok(ProcessedToolResultContent {
            content: content.to_owned(),
            content_blocks: content_blocks.to_vec(),
        });
    };
    if content_blocks_size(content_blocks) <= limit {
        return Ok(ProcessedToolResultContent {
            content: content.to_owned(),
            content_blocks: content_blocks.to_vec(),
        });
    }

    let Some(tool_results_dir) = tool_results_dir else {
        return Ok(ProcessedToolResultContent {
            content: content.to_owned(),
            content_blocks: content_blocks.to_vec(),
        });
    };

    let persisted = persist_tool_result_blocks(content_blocks, tool_use_id, tool_results_dir)?;
    Ok(ProcessedToolResultContent {
        content: build_large_tool_result_message(&persisted),
        content_blocks: Vec::new(),
    })
}

fn process_tool_result_text_with_empty_message(
    content: &str,
    tool_use_id: &str,
    tool_results_dir: Option<&Path>,
    result_size_policy: ToolResultSizePolicy,
    empty_message: &str,
) -> Result<String> {
    if is_tool_result_text_empty(content) {
        return Ok(empty_message.to_owned());
    }
    let Some(limit) = persistence_threshold(result_size_policy) else {
        return Ok(content.to_owned());
    };
    if content.chars().count() <= limit {
        return Ok(content.to_owned());
    }
    let Some(tool_results_dir) = tool_results_dir else {
        return Ok(content.to_owned());
    };

    let persisted = persist_tool_result_text(content, tool_use_id, tool_results_dir)?;
    Ok(build_large_tool_result_message(&persisted))
}

fn empty_tool_result_message(tool_name: &str) -> String {
    format!("({tool_name} completed with no output)")
}

#[must_use]
pub fn persistence_threshold(result_size_policy: ToolResultSizePolicy) -> Option<usize> {
    match result_size_policy {
        ToolResultSizePolicy::NeverPersist => None,
        ToolResultSizePolicy::Finite(declared_max_result_size_chars) => {
            Some(declared_max_result_size_chars.min(DEFAULT_MAX_RESULT_SIZE_CHARS))
        }
    }
}

fn cleanup_one_level_tool_dir(
    tool_dir: &Path,
    cutoff: SystemTime,
    summary: &mut ToolResultsCleanupSummary,
) {
    let Ok(entries) = fs::read_dir(tool_dir) else {
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            summary.errors += 1;
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            summary.errors += 1;
            continue;
        };
        if file_type.is_file() {
            unlink_if_old(&entry.path(), cutoff, summary);
        }
    }
}

fn unlink_if_old(path: &Path, cutoff: SystemTime, summary: &mut ToolResultsCleanupSummary) {
    match fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(|modified| modified <= cutoff)
    {
        Ok(true) => match fs::remove_file(path) {
            Ok(()) => summary.removed_files += 1,
            Err(_) => summary.errors += 1,
        },
        Ok(false) => {}
        Err(_) => summary.errors += 1,
    }
}

fn session_parent(tool_results_dir: &Path) -> &Path {
    tool_results_dir.parent().unwrap_or(tool_results_dir)
}

pub fn persist_tool_result_text(
    content: &str,
    tool_use_id: &str,
    tool_results_dir: &Path,
) -> Result<PersistedToolResult> {
    ensure_tool_results_dir(tool_results_dir)?;
    let filepath = get_tool_result_path(tool_results_dir, tool_use_id, false);

    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&filepath)
    {
        Ok(mut file) => {
            file.write_all(content.as_bytes())
                .with_context(|| format!("failed to write {}", filepath.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(anyhow!(
                "{}",
                filesystem_error_message(&filepath, error.kind(), &error.to_string())
            ));
        }
    }

    let (preview, has_more) = generate_preview(content, PREVIEW_SIZE_BYTES);
    Ok(PersistedToolResult {
        filepath,
        original_size: content.len(),
        is_json: false,
        preview,
        has_more,
    })
}

pub fn persist_tool_result_blocks(
    content_blocks: &[Value],
    tool_use_id: &str,
    tool_results_dir: &Path,
) -> Result<PersistedToolResult> {
    if content_blocks.iter().any(|block| {
        block.get("type").and_then(Value::as_str) != Some("text")
            || block.get("text").and_then(Value::as_str).is_none()
    }) {
        return Err(anyhow!(NON_TEXT_CONTENT_ERROR.clone()));
    }

    ensure_tool_results_dir(tool_results_dir)?;
    let filepath = get_tool_result_path(tool_results_dir, tool_use_id, true);
    let content = serde_json::to_string_pretty(content_blocks).context("serialize tool content")?;

    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&filepath)
    {
        Ok(mut file) => {
            file.write_all(content.as_bytes())
                .with_context(|| format!("failed to write {}", filepath.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(anyhow!(
                "{}",
                filesystem_error_message(&filepath, error.kind(), &error.to_string())
            ));
        }
    }

    let (preview, has_more) = generate_preview(&content, PREVIEW_SIZE_BYTES);
    Ok(PersistedToolResult {
        filepath,
        original_size: content.len(),
        is_json: true,
        preview,
        has_more,
    })
}

pub fn build_large_tool_result_message(result: &PersistedToolResult) -> String {
    let mut message = String::new();
    message.push_str(PERSISTED_OUTPUT_TAG);
    message.push('\n');
    message.push_str(&format!(
        "Output too large ({}). Full output saved to: {}\n\n",
        format_file_size(result.original_size),
        result.filepath.display()
    ));
    message.push_str(&format!(
        "Preview (first {}):\n",
        format_file_size(PREVIEW_SIZE_BYTES)
    ));
    message.push_str(&result.preview);
    if result.has_more {
        message.push_str("\n...\n");
    } else {
        message.push('\n');
    }
    message.push_str(PERSISTED_OUTPUT_CLOSING_TAG);
    message
}

#[must_use]
pub fn reconstruct_content_replacement_state(
    conversation: &[ConversationEntry],
    records: &[ContentReplacementRecord],
    inherited_replacements: Option<&std::collections::HashMap<String, String>>,
) -> ContentReplacementState {
    let mut state = ContentReplacementState::new();
    let candidate_ids = collect_candidates_by_message(conversation)
        .into_iter()
        .flatten()
        .map(|candidate| candidate.tool_use_id)
        .collect::<std::collections::HashSet<_>>();

    state.seen_ids.extend(candidate_ids.iter().cloned());
    for record in records {
        if record.kind == ContentReplacementKind::ToolResult
            && candidate_ids.contains(&record.tool_use_id)
        {
            state
                .replacements
                .insert(record.tool_use_id.clone(), record.replacement.clone());
        }
    }
    if let Some(inherited_replacements) = inherited_replacements {
        for (id, replacement) in inherited_replacements {
            if candidate_ids.contains(id) && !state.replacements.contains_key(id) {
                state.replacements.insert(id.clone(), replacement.clone());
            }
        }
    }
    state
}

pub fn apply_tool_result_budget_to_conversation(
    conversation: &mut [ConversationEntry],
    state: &mut ContentReplacementState,
    tool_results_dir: &Path,
    skip_tool_names: &std::collections::HashSet<String>,
) -> Result<ToolResultBudgetOutcome> {
    let candidates_by_message = collect_candidates_by_message(conversation);
    let mut replacement_map = std::collections::HashMap::<String, String>::new();
    let mut to_persist = Vec::new();

    for candidates in candidates_by_message {
        let mut fresh = Vec::new();
        let mut frozen_size = 0usize;

        for candidate in candidates {
            if let Some(replacement) = state.replacements.get(&candidate.tool_use_id) {
                replacement_map.insert(candidate.tool_use_id.clone(), replacement.clone());
            } else if state.seen_ids.contains(&candidate.tool_use_id) {
                frozen_size = frozen_size.saturating_add(candidate.size);
            } else {
                fresh.push(candidate);
            }
        }

        if fresh.is_empty() {
            continue;
        }

        let mut eligible = Vec::new();
        let mut skipped = Vec::new();
        for candidate in fresh {
            if skip_tool_names.contains(&candidate.tool_name) {
                skipped.push(candidate);
            } else {
                eligible.push(candidate);
            }
        }
        state
            .seen_ids
            .extend(skipped.into_iter().map(|candidate| candidate.tool_use_id));

        let fresh_size = eligible
            .iter()
            .fold(0usize, |sum, candidate| sum.saturating_add(candidate.size));
        let selected = if frozen_size.saturating_add(fresh_size) > per_message_budget_limit() {
            select_fresh_to_replace(eligible.as_slice(), frozen_size)
        } else {
            Vec::new()
        };
        let selected_ids = selected
            .iter()
            .map(|candidate| candidate.tool_use_id.clone())
            .collect::<std::collections::HashSet<_>>();

        state.seen_ids.extend(
            eligible
                .into_iter()
                .filter(|candidate| !selected_ids.contains(&candidate.tool_use_id))
                .map(|candidate| candidate.tool_use_id),
        );
        to_persist.extend(selected);
    }

    if replacement_map.is_empty() && to_persist.is_empty() {
        return Ok(ToolResultBudgetOutcome::default());
    }

    let mut newly_replaced = Vec::new();
    for candidate in to_persist {
        state.seen_ids.insert(candidate.tool_use_id.clone());
        let persisted = match candidate
            .content
            .persist(&candidate.tool_use_id, tool_results_dir)
        {
            Ok(persisted) => persisted,
            Err(_) => continue,
        };
        let replacement = build_large_tool_result_message(&persisted);
        replacement_map.insert(candidate.tool_use_id.clone(), replacement.clone());
        state
            .replacements
            .insert(candidate.tool_use_id.clone(), replacement.clone());
        newly_replaced.push(ContentReplacementRecord {
            kind: ContentReplacementKind::ToolResult,
            tool_use_id: candidate.tool_use_id,
            replacement,
        });
    }

    if !replacement_map.is_empty() {
        replace_tool_result_contents(conversation, &replacement_map);
    }

    Ok(ToolResultBudgetOutcome { newly_replaced })
}

fn collect_candidates_by_message(
    conversation: &[ConversationEntry],
) -> Vec<Vec<ToolResultCandidate>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();

    let flush = |groups: &mut Vec<Vec<ToolResultCandidate>>,
                 current: &mut Vec<ToolResultCandidate>| {
        if !current.is_empty() {
            groups.push(std::mem::take(current));
        }
    };

    for entry in conversation {
        match entry.role {
            ConversationRole::Assistant => flush(&mut groups, &mut current),
            ConversationRole::Tool | ConversationRole::User => {
                current.extend(collect_candidates_from_entry(entry));
            }
            ConversationRole::System => {}
        }
    }
    flush(&mut groups, &mut current);
    groups
}

fn collect_candidates_from_entry(entry: &ConversationEntry) -> Vec<ToolResultCandidate> {
    if entry.role != ConversationRole::Tool {
        return Vec::new();
    }
    let Some(tool_use_id) = entry.tool_call_id.clone() else {
        return Vec::new();
    };
    let tool_name = entry.name.clone().unwrap_or_default();

    if !entry.content_blocks.is_empty() {
        if content_blocks_have_image(&entry.content_blocks)
            || content_blocks_are_empty(&entry.content_blocks)
        {
            return Vec::new();
        }
        let size = content_blocks_size(&entry.content_blocks);
        return vec![ToolResultCandidate {
            tool_use_id,
            tool_name,
            content: CandidateContent::Blocks(entry.content_blocks.clone()),
            size,
        }];
    }

    if is_tool_result_text_empty(&entry.text) || is_content_already_compacted(&entry.text) {
        return Vec::new();
    }
    vec![ToolResultCandidate {
        tool_use_id,
        tool_name,
        size: entry.text.len(),
        content: CandidateContent::Text(entry.text.clone()),
    }]
}

fn select_fresh_to_replace(
    fresh: &[ToolResultCandidate],
    frozen_size: usize,
) -> Vec<ToolResultCandidate> {
    let mut sorted = fresh.to_vec();
    sorted.sort_by(|a, b| b.size.cmp(&a.size));
    let mut selected = Vec::new();
    let mut remaining = frozen_size.saturating_add(
        fresh
            .iter()
            .fold(0usize, |sum, candidate| sum.saturating_add(candidate.size)),
    );
    for candidate in sorted {
        if remaining <= per_message_budget_limit() {
            break;
        }
        remaining = remaining.saturating_sub(candidate.size);
        selected.push(candidate);
    }
    selected
}

#[must_use]
pub const fn per_message_budget_limit() -> usize {
    MAX_TOOL_RESULTS_PER_MESSAGE_CHARS
}

fn replace_tool_result_contents(
    conversation: &mut [ConversationEntry],
    replacement_map: &std::collections::HashMap<String, String>,
) {
    for entry in conversation {
        if entry.role != ConversationRole::Tool {
            continue;
        }
        let Some(tool_use_id) = entry.tool_call_id.as_deref() else {
            continue;
        };
        let Some(replacement) = replacement_map.get(tool_use_id) else {
            continue;
        };
        entry.text.clone_from(replacement);
        entry.content_blocks.clear();
        entry.history_text = None;
    }
}

fn is_content_already_compacted(content: &str) -> bool {
    content.starts_with(PERSISTED_OUTPUT_TAG)
}

fn content_blocks_have_image(content_blocks: &[Value]) -> bool {
    content_blocks
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("image"))
}

fn content_blocks_are_text_only(content_blocks: &[Value]) -> bool {
    content_blocks.iter().all(|block| {
        block.get("type").and_then(Value::as_str) == Some("text")
            && block.get("text").and_then(Value::as_str).is_some()
    })
}

fn content_blocks_are_empty(content_blocks: &[Value]) -> bool {
    content_blocks.is_empty()
        || content_blocks.iter().all(|block| {
            block.get("type").and_then(Value::as_str) == Some("text")
                && block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_none_or(|text| text.trim().is_empty())
        })
}

fn content_blocks_size(content_blocks: &[Value]) -> usize {
    content_blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .map(str::len)
        .sum()
}

fn is_tool_result_text_empty(content: &str) -> bool {
    content.trim().is_empty()
}

pub fn generate_preview(content: &str, max_bytes: usize) -> (String, bool) {
    if content.len() <= max_bytes {
        return (content.to_owned(), false);
    }
    let truncated = char_boundary_prefix(content, max_bytes);
    let last_newline = truncated.rfind('\n');
    let cut_point = last_newline
        .filter(|idx| *idx > max_bytes / 2)
        .unwrap_or(max_bytes);
    let preview = char_boundary_prefix(content, cut_point);
    (preview.to_owned(), true)
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

fn filesystem_error_message(path: &Path, kind: std::io::ErrorKind, fallback: &str) -> String {
    match kind {
        std::io::ErrorKind::NotFound => format!("Directory not found: {}", path.display()),
        std::io::ErrorKind::PermissionDenied => format!("Permission denied: {}", path.display()),
        _ => fallback.to_owned(),
    }
}

fn char_boundary_prefix(content: &str, max_bytes: usize) -> &str {
    if content.len() <= max_bytes {
        return content;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !content.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &content[..boundary]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::{Duration, SystemTime};

    use claude_core::ConversationEntry;
    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        BYTES_PER_TOKEN, DEFAULT_MAX_RESULT_SIZE_CHARS, MAX_TOOL_RESULT_BYTES,
        PERSISTED_OUTPUT_CLOSING_TAG, PERSISTED_OUTPUT_TAG, PREVIEW_SIZE_BYTES,
        TOOL_RESULT_CLEARED_MESSAGE, apply_tool_result_budget_to_conversation,
        build_large_tool_result_message, cleanup_tool_results_for_session, generate_preview,
        persist_tool_result_blocks, persist_tool_result_text, persistence_threshold,
        process_tool_result_content, process_tool_result_text,
        reconstruct_content_replacement_state, session_tool_results_dir,
    };
    use crate::{
        DEFAULT_TOOL_MAX_RESULT_SIZE_CHARS, ToolResultSizePolicy, builtin_tool_specs,
        runtime_tool_result_persistence_skip_names,
    };

    #[test]
    fn process_tool_result_text_persists_large_output() {
        let temp = tempdir().expect("tempdir");
        let tool_results_dir = session_tool_results_dir(temp.path());
        let content = "x".repeat(60_000);
        let processed = process_tool_result_text(
            &content,
            "call-1",
            Some(&tool_results_dir),
            ToolResultSizePolicy::default(),
        )
        .expect("process");
        assert!(processed.starts_with(PERSISTED_OUTPUT_TAG));
        assert!(processed.ends_with(PERSISTED_OUTPUT_CLOSING_TAG));
        assert!(tool_results_dir.join("call-1.txt").exists());
    }

    #[test]
    fn persistence_threshold_clamps_large_declared_sizes_to_default_limit() {
        assert_eq!(
            persistence_threshold(ToolResultSizePolicy::Finite(MAX_TOOL_RESULT_BYTES)),
            Some(DEFAULT_MAX_RESULT_SIZE_CHARS)
        );
        assert_eq!(
            persistence_threshold(ToolResultSizePolicy::Finite(4_000)),
            Some(4_000)
        );
        assert_eq!(
            persistence_threshold(ToolResultSizePolicy::default()),
            Some(DEFAULT_MAX_RESULT_SIZE_CHARS),
            "default declared size still clamps to the research persistence cap"
        );
        assert_eq!(
            DEFAULT_TOOL_MAX_RESULT_SIZE_CHARS, 100_000,
            "default per-tool declaration should mirror research tools"
        );
        assert_eq!(
            persistence_threshold(ToolResultSizePolicy::NeverPersist),
            None
        );
        assert_eq!(MAX_TOOL_RESULT_BYTES, 100_000 * BYTES_PER_TOKEN);
        assert_eq!(
            TOOL_RESULT_CLEARED_MESSAGE,
            "[Old tool result content cleared]"
        );
    }

    #[test]
    fn process_tool_result_text_normalizes_empty_output() {
        let processed =
            process_tool_result_text("   \n", "call-empty", None, ToolResultSizePolicy::default())
                .expect("process");
        assert_eq!(processed, "(tool completed with no output)");
    }

    #[test]
    fn process_tool_result_content_persists_large_text_blocks() {
        let temp = tempdir().expect("tempdir");
        let tool_results_dir = session_tool_results_dir(temp.path());
        let blocks = vec![json!({"type":"text","text":"x".repeat(60_000)})];

        let processed = process_tool_result_content(
            "",
            &blocks,
            "call-blocks",
            "bash_command",
            Some(&tool_results_dir),
            ToolResultSizePolicy::default(),
        )
        .expect("process");

        assert!(processed.content.starts_with(PERSISTED_OUTPUT_TAG));
        assert!(processed.content_blocks.is_empty());
        assert!(tool_results_dir.join("call-blocks.json").exists());
    }

    #[test]
    fn process_tool_result_content_keeps_small_text_blocks() {
        let blocks = vec![json!({"type":"text","text":"small"})];

        let processed = process_tool_result_content(
            "",
            &blocks,
            "call-small",
            "bash_command",
            None,
            ToolResultSizePolicy::default(),
        )
        .expect("process");

        assert_eq!(processed.content, "");
        assert_eq!(processed.content_blocks, blocks);
    }

    #[test]
    fn process_tool_result_content_normalizes_empty_text_blocks() {
        let blocks = vec![json!({"type":"text","text":"   "})];

        let processed = process_tool_result_content(
            "",
            &blocks,
            "call-empty",
            "bash_command",
            None,
            ToolResultSizePolicy::default(),
        )
        .expect("process");

        assert_eq!(processed.content, "(bash_command completed with no output)");
        assert!(processed.content_blocks.is_empty());
    }

    #[test]
    fn process_tool_result_content_preserves_non_text_blocks() {
        let blocks = vec![json!({"type":"image","source":"x"})];

        let processed = process_tool_result_content(
            "image content",
            &blocks,
            "call-image",
            "web_fetch",
            None,
            ToolResultSizePolicy::default(),
        )
        .expect("process");

        assert_eq!(processed.content, "image content");
        assert_eq!(processed.content_blocks, blocks);
    }

    #[test]
    fn process_tool_result_content_never_persist_keeps_large_read_results_inline() {
        let temp = tempdir().expect("tempdir");
        let tool_results_dir = session_tool_results_dir(temp.path());
        let blocks = vec![json!({"type":"text","text":"x".repeat(60_000)})];

        let processed = process_tool_result_content(
            "",
            &blocks,
            "call-read-large",
            "read_file",
            Some(&tool_results_dir),
            ToolResultSizePolicy::NeverPersist,
        )
        .expect("process");

        assert_eq!(processed.content, "");
        assert_eq!(processed.content_blocks, blocks);
        assert!(!tool_results_dir.join("call-read-large.json").exists());
        assert!(!tool_results_dir.join("call-read-large.txt").exists());
    }

    #[test]
    fn process_tool_result_content_respects_bash_threshold() {
        let temp = tempdir().expect("tempdir");
        let tool_results_dir = session_tool_results_dir(temp.path());
        let blocks = vec![json!({"type":"text","text":"x".repeat(30_001)})];

        let processed = process_tool_result_content(
            "",
            &blocks,
            "call-bash-large",
            "bash_command",
            Some(&tool_results_dir),
            ToolResultSizePolicy::Finite(30_000),
        )
        .expect("process");

        assert!(processed.content.starts_with(PERSISTED_OUTPUT_TAG));
        assert!(tool_results_dir.join("call-bash-large.json").exists());
    }

    #[test]
    fn process_tool_result_content_respects_grep_threshold() {
        let temp = tempdir().expect("tempdir");
        let tool_results_dir = session_tool_results_dir(temp.path());
        let blocks = vec![json!({"type":"text","text":"x".repeat(20_001)})];

        let processed = process_tool_result_content(
            "",
            &blocks,
            "call-grep-large",
            "grep",
            Some(&tool_results_dir),
            ToolResultSizePolicy::Finite(20_000),
        )
        .expect("process");

        assert!(processed.content.starts_with(PERSISTED_OUTPUT_TAG));
        assert!(tool_results_dir.join("call-grep-large.json").exists());
    }

    #[test]
    fn process_tool_result_content_default_declared_size_still_clamps_to_global_cap() {
        let temp = tempdir().expect("tempdir");
        let tool_results_dir = session_tool_results_dir(temp.path());
        let blocks = vec![json!({"type":"text","text":"x".repeat(60_000)})];

        let processed = process_tool_result_content(
            "",
            &blocks,
            "call-default-large",
            "web_search",
            Some(&tool_results_dir),
            ToolResultSizePolicy::Finite(DEFAULT_TOOL_MAX_RESULT_SIZE_CHARS),
        )
        .expect("process");

        assert!(processed.content.starts_with(PERSISTED_OUTPUT_TAG));
        assert!(tool_results_dir.join("call-default-large.json").exists());
    }

    #[test]
    fn generate_preview_prefers_newline_boundary() {
        let content = "line1\nline2\nline3";
        let (preview, has_more) = generate_preview(content, 8);
        assert!(has_more);
        assert_eq!(preview, "line1");
    }

    #[test]
    fn persist_tool_result_blocks_rejects_non_text_blocks() {
        let temp = tempdir().expect("tempdir");
        let result = persist_tool_result_blocks(
            &[serde_json::json!({"type":"image","source":"x"})],
            "call-1",
            temp.path(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn build_large_tool_result_message_includes_preview() {
        let temp = tempdir().expect("tempdir");
        let persisted =
            persist_tool_result_text(&"a".repeat(PREVIEW_SIZE_BYTES + 10), "call-2", temp.path())
                .expect("persist");
        let message = build_large_tool_result_message(&persisted);
        assert!(message.contains("Full output saved to:"));
        assert!(message.contains("Preview (first"));
    }

    #[test]
    fn per_message_budget_persists_largest_fresh_tool_result() {
        let temp = tempdir().expect("tempdir");
        let mut conversation = vec![
            ConversationEntry::assistant(""),
            ConversationEntry::tool("small", "bash_command", "s".repeat(10_000), false),
            ConversationEntry::tool("large", "bash_command", "x".repeat(210_000), false),
        ];
        let mut state = super::ContentReplacementState::new();

        let outcome = apply_tool_result_budget_to_conversation(
            &mut conversation,
            &mut state,
            temp.path(),
            &HashSet::new(),
        )
        .expect("budget");

        assert_eq!(outcome.newly_replaced.len(), 1);
        assert_eq!(outcome.newly_replaced[0].tool_use_id, "large");
        assert!(conversation[2].text.starts_with(PERSISTED_OUTPUT_TAG));
        assert!(temp.path().join("large.txt").exists());
        assert!(state.seen_ids.contains("small"));
        assert!(state.seen_ids.contains("large"));
        assert!(state.replacements.contains_key("large"));
    }

    #[test]
    fn per_message_budget_reapplies_stored_replacement_without_repersisting() {
        let temp = tempdir().expect("tempdir");
        let mut conversation = vec![
            ConversationEntry::assistant(""),
            ConversationEntry::tool("large", "bash_command", "x".repeat(210_000), false),
        ];
        let mut state = super::ContentReplacementState::new();
        let first = apply_tool_result_budget_to_conversation(
            &mut conversation,
            &mut state,
            temp.path(),
            &HashSet::new(),
        )
        .expect("first budget");
        let replacement = first.newly_replaced[0].replacement.clone();

        conversation[1].text = "x".repeat(210_000);
        let second = apply_tool_result_budget_to_conversation(
            &mut conversation,
            &mut state,
            temp.path(),
            &HashSet::new(),
        )
        .expect("second budget");

        assert!(second.newly_replaced.is_empty());
        assert_eq!(conversation[1].text, replacement);
    }

    #[test]
    fn per_message_budget_freezes_seen_unreplaced_results() {
        let temp = tempdir().expect("tempdir");
        let mut conversation = vec![
            ConversationEntry::assistant(""),
            ConversationEntry::tool("medium", "bash_command", "m".repeat(150_000), false),
        ];
        let mut state = super::ContentReplacementState::new();
        let first = apply_tool_result_budget_to_conversation(
            &mut conversation,
            &mut state,
            temp.path(),
            &HashSet::new(),
        )
        .expect("first budget");
        assert!(first.newly_replaced.is_empty());
        assert!(state.seen_ids.contains("medium"));

        conversation.push(ConversationEntry::tool(
            "fresh",
            "bash_command",
            "f".repeat(80_000),
            false,
        ));
        let second = apply_tool_result_budget_to_conversation(
            &mut conversation,
            &mut state,
            temp.path(),
            &HashSet::new(),
        )
        .expect("second budget");

        assert_eq!(second.newly_replaced.len(), 1);
        assert_eq!(second.newly_replaced[0].tool_use_id, "fresh");
        assert_eq!(conversation[1].text, "m".repeat(150_000));
    }

    #[test]
    fn per_message_budget_skips_read_file_candidates_from_runtime_specs() {
        let temp = tempdir().expect("tempdir");
        let mut conversation = vec![
            ConversationEntry::assistant(""),
            ConversationEntry::tool("read-large", "read_file", "r".repeat(210_000), false),
            ConversationEntry::tool("bash-large", "bash_command", "b".repeat(210_000), false),
        ];
        let mut state = super::ContentReplacementState::new();
        let specs = builtin_tool_specs();
        let skip_tool_names = runtime_tool_result_persistence_skip_names();
        assert!(
            specs.iter().any(|spec| spec.name == "read_file"),
            "builtin read_file spec should exist"
        );
        assert!(skip_tool_names.contains("read_file"));

        let outcome = apply_tool_result_budget_to_conversation(
            &mut conversation,
            &mut state,
            temp.path(),
            &skip_tool_names,
        )
        .expect("budget");

        assert_eq!(outcome.newly_replaced.len(), 1);
        assert_eq!(outcome.newly_replaced[0].tool_use_id, "bash-large");
        assert_eq!(conversation[1].text, "r".repeat(210_000));
        assert!(conversation[2].text.starts_with(PERSISTED_OUTPUT_TAG));
        assert!(!temp.path().join("read-large.txt").exists());
        assert!(temp.path().join("bash-large.txt").exists());
    }

    #[test]
    fn reconstruct_state_freezes_candidates_and_restores_records() {
        let mut conversation = vec![
            ConversationEntry::assistant(""),
            ConversationEntry::tool("replaced", "bash_command", "x".repeat(10), false),
            ConversationEntry::tool("plain", "bash_command", "y".repeat(10), false),
        ];
        conversation[1].content_blocks = vec![json!({"type": "text", "text": "block text"})];
        let records = vec![super::ContentReplacementRecord {
            kind: super::ContentReplacementKind::ToolResult,
            tool_use_id: "replaced".to_owned(),
            replacement: "cached replacement".to_owned(),
        }];

        let state = reconstruct_content_replacement_state(&conversation, &records, None);

        assert!(state.seen_ids.contains("replaced"));
        assert!(state.seen_ids.contains("plain"));
        assert_eq!(
            state.replacements.get("replaced").map(String::as_str),
            Some("cached replacement")
        );
        assert!(!state.replacements.contains_key("plain"));
    }

    #[test]
    fn cleanup_tool_results_removes_old_files_in_flat_and_nested_layouts() {
        let temp = tempdir().expect("tempdir");
        let session_dir = temp.path().join("session");
        let tool_results_dir = session_tool_results_dir(&session_dir);
        std::fs::create_dir_all(tool_results_dir.join("nested")).expect("tool results");
        std::fs::write(tool_results_dir.join("flat.txt"), "old").expect("flat");
        std::fs::write(tool_results_dir.join("nested").join("inner.txt"), "old").expect("inner");

        let cutoff = SystemTime::now() + Duration::from_secs(1);
        let summary = cleanup_tool_results_for_session(&session_dir, cutoff);

        assert_eq!(summary.removed_files, 2);
        assert!(summary.errors == 0, "cleanup errors: {}", summary.errors);
        assert!(!tool_results_dir.exists());
        assert!(!session_dir.exists());
    }
}
