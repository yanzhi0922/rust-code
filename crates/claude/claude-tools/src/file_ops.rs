//! File operation tools: list_directory, read_file, search_text, write_file,
//! replace_in_file, edit_file, glob_files, grep_files.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, anyhow};
use claude_core::ToolResult;
use claude_permissions::{FilesystemOperation, assess_filesystem_access};
use globset::GlobBuilder;
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::{Value, json};
use walkdir::WalkDir;

use super::{FileState, IGNORED_DIRS, ToolExecutionContext};

const FILE_UNCHANGED_STUB: &str = "File unchanged since last read. The content from the earlier Read tool_result in this conversation is still current — refer to that instead of re-reading.";
const FILE_UNEXPECTEDLY_MODIFIED_ERROR: &str =
    "File has been unexpectedly modified. Read it again before attempting to write it.";

/// Maximum dimension (pixels) for the longest side when resizing images.
/// Matches the TS reference constant `MAX_IMAGE_DIMENSION = 1600`.
const MAX_IMAGE_DIMENSION: u32 = 1600;

/// Maximum characters per line in grep output before truncation.
/// Matches the TS reference `max-columns 500` behaviour.
const MAX_COLUMNS: usize = 500;

/// Maximum total characters for grep results before truncation.
const MAX_RESULT_SIZE_CHARS: usize = 20_000;

/// VCS directories excluded from grep walks (matches TS ripgrep defaults).
const VCS_DIRS: &[&str] = &[".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

/// Image extensions that can be sent as multimodal base64 content blocks.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

const BLOCKED_DEVICE_PATHS: &[&str] = &[
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/full",
    "/dev/stdin",
    "/dev/tty",
    "/dev/console",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/fd/0",
    "/dev/fd/1",
    "/dev/fd/2",
];

fn is_blocked_device_path(path: &str) -> bool {
    if BLOCKED_DEVICE_PATHS.contains(&path) {
        return true;
    }
    // /proc/self/fd/0-2 and /proc/<pid>/fd/0-2 are Linux aliases for stdio
    if path.starts_with("/proc/")
        && (path.ends_with("/fd/0") || path.ends_with("/fd/1") || path.ends_with("/fd/2"))
    {
        return true;
    }
    false
}

tokio::task_local! {
    static TOOL_FILESYSTEM_PERMISSION_CONFIRMED: bool;
}

pub(crate) async fn with_filesystem_permission_confirmed<F, T>(confirmed: bool, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    TOOL_FILESYSTEM_PERMISSION_CONFIRMED
        .scope(confirmed, future)
        .await
}

fn filesystem_permission_confirmed_for_dispatch() -> bool {
    TOOL_FILESYSTEM_PERMISSION_CONFIRMED
        .try_with(|confirmed| *confirmed)
        .unwrap_or(false)
}

/// Map common language type names to file extensions (matching ripgrep `--type`).
fn type_to_extensions(type_name: &str) -> Option<&'static [&'static str]> {
    match type_name {
        "rust" => Some(&["*.rs"]),
        "js" | "javascript" => Some(&["*.js", "*.jsx", "*.mjs", "*.cjs"]),
        "ts" | "typescript" => Some(&["*.ts", "*.tsx", "*.mts", "*.cts"]),
        "py" | "python" => Some(&["*.py", "*.pyi"]),
        "java" => Some(&["*.java"]),
        "go" => Some(&["*.go"]),
        "c" => Some(&["*.c", "*.h"]),
        "cpp" | "c++" => Some(&["*.cpp", "*.cc", "*.cxx", "*.hpp", "*.hh", "*.hxx"]),
        "ruby" | "rb" => Some(&["*.rb"]),
        "swift" => Some(&["*.swift"]),
        "kotlin" | "kt" => Some(&["*.kt", "*.kts"]),
        "css" => Some(&["*.css", "*.scss", "*.sass", "*.less"]),
        "html" => Some(&["*.html", "*.htm"]),
        "json" => Some(&["*.json"]),
        "yaml" | "yml" => Some(&["*.yaml", "*.yml"]),
        "markdown" | "md" => Some(&["*.md", "*.markdown"]),
        "shell" | "sh" => Some(&["*.sh", "*.bash", "*.zsh"]),
        "sql" => Some(&["*.sql"]),
        _ => None,
    }
}

fn normalize_quotes(s: &str) -> String {
    s.replace(['\u{201C}', '\u{201D}'], "\"")
        .replace(['\u{2018}', '\u{2019}'], "'")
}

/// Detect whether the byte slice starts with a UTF-16LE BOM (0xFF 0xFE).
fn has_utf16le_bom(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE
}

/// Line ending style detected in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEnding {
    Lf,
    Crlf,
}

/// Detect the dominant line ending style in `content`.
fn detect_line_ending(content: &str) -> LineEnding {
    if content.contains("\r\n") {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    }
}

/// Normalize line endings in `text` to match `line_ending`.
/// Converts any mix of CRLF/LF to the specified style.
fn normalize_line_endings(text: &str, line_ending: LineEnding) -> String {
    match line_ending {
        LineEnding::Crlf => {
            // First normalize all to LF, then convert to CRLF
            text.replace("\r\n", "\n").replace('\n', "\r\n")
        }
        LineEnding::Lf => text.replace("\r\n", "\n"),
    }
}

/// Maximum number of diff output lines before truncation.
const MAX_DIFF_LINES: usize = 200;

/// Compute a unified diff between `old` and `new` content for the given `path`.
///
/// Uses a simple LCS (Longest Common Subsequence) algorithm to identify added/removed
/// lines and emits hunks in standard unified diff format. Output is capped at
/// [`MAX_DIFF_LINES`] lines.
fn compute_unified_diff(old: &str, new: &str, path: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let lcs = compute_lcs(&old_lines, &new_lines);
    let ops = diff_ops_from_lcs(&old_lines, &new_lines, &lcs);

    // Gather hunks: groups of changes with 3 lines of context on each side.
    let hunks = gather_hunks(&ops, &old_lines, &new_lines, 3);

    if hunks.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str(&format!("--- {path}\n+++ {path}\n"));

    let mut line_count = 2; // header lines

    for hunk in &hunks {
        if line_count >= MAX_DIFF_LINES {
            break;
        }
        let header = format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
        );
        out.push_str(&header);
        line_count += 1;

        for diff_line in &hunk.lines {
            if line_count >= MAX_DIFF_LINES {
                break;
            }
            out.push_str(diff_line);
            out.push('\n');
            line_count += 1;
        }

        if line_count >= MAX_DIFF_LINES {
            break;
        }
    }

    if line_count >= MAX_DIFF_LINES {
        out.push_str("[diff truncated]\n");
    }

    out
}

/// A single diff hunk in unified diff format.
struct DiffHunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<String>,
}

/// Gather contiguous hunks from the diff operations with `context` lines of context.
fn gather_hunks(
    ops: &[DiffOp],
    old_lines: &[&str],
    new_lines: &[&str],
    context: usize,
) -> Vec<DiffHunk> {
    // Find indices of non-equal ops (changes)
    let change_indices: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| !matches!(op, DiffOp::Equal { .. }))
        .map(|(i, _)| i)
        .collect();

    if change_indices.is_empty() {
        return Vec::new();
    }

    // Group change indices into hunks separated by more than 2*context equal lines
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut current_group = vec![change_indices[0]];

    for &idx in &change_indices[1..] {
        let prev = *current_group
            .last()
            .expect("current diff group is initialized before grouping");
        // If the gap between changes is more than 2*context, start a new group
        if idx > prev + 2 * context + 1 {
            groups.push(std::mem::take(&mut current_group));
        }
        current_group.push(idx);
    }
    groups.push(current_group);

    let mut hunks = Vec::new();

    for group in &groups {
        let first_change = group[0];
        let last_change = *group
            .last()
            .expect("diff groups are never empty after grouping");

        // Expand to include context lines
        let start = first_change.saturating_sub(context);
        let end = (last_change + context + 1).min(ops.len());

        let mut hunk_lines = Vec::new();
        let mut old_start = 0;
        let mut new_start = 0;
        let mut old_count = 0;
        let mut new_count = 0;

        // Compute the line offsets up to `start`
        for op in &ops[..start] {
            match op {
                DiffOp::Equal { .. } | DiffOp::Remove { .. } => old_start += 1,
                _ => {}
            }
            match op {
                DiffOp::Equal { .. } | DiffOp::Insert { .. } => new_start += 1,
                _ => {}
            }
        }
        // Convert to 1-based
        old_start += 1;
        new_start += 1;

        for op in &ops[start..end] {
            match op {
                DiffOp::Equal {
                    old_idx,
                    new_idx: _,
                } => {
                    hunk_lines.push(format!(" {}", old_lines[*old_idx]));
                    old_count += 1;
                    new_count += 1;
                }
                DiffOp::Remove { old_idx } => {
                    hunk_lines.push(format!("-{}", old_lines[*old_idx]));
                    old_count += 1;
                }
                DiffOp::Insert { new_idx } => {
                    hunk_lines.push(format!("+{}", new_lines[*new_idx]));
                    new_count += 1;
                }
            }
        }

        hunks.push(DiffHunk {
            old_start,
            old_count,
            new_start,
            new_count,
            lines: hunk_lines,
        });
    }

    hunks
}

/// Diff operation types.
#[derive(Debug)]
#[allow(dead_code)]
enum DiffOp {
    Equal { old_idx: usize, new_idx: usize },
    Remove { old_idx: usize },
    Insert { new_idx: usize },
}

/// Convert LCS result into diff operations.
fn diff_ops_from_lcs(old: &[&str], new: &[&str], lcs: &[(usize, usize)]) -> Vec<DiffOp> {
    let mut ops = Vec::new();
    let mut oi = 0;
    let mut ni = 0;

    for &(lcs_oi, lcs_ni) in lcs {
        // Emit removals for old lines before this LCS match
        while oi < lcs_oi {
            ops.push(DiffOp::Remove { old_idx: oi });
            oi += 1;
        }
        // Emit insertions for new lines before this LCS match
        while ni < lcs_ni {
            ops.push(DiffOp::Insert { new_idx: ni });
            ni += 1;
        }
        // Emit the equal line
        ops.push(DiffOp::Equal {
            old_idx: oi,
            new_idx: ni,
        });
        oi += 1;
        ni += 1;
    }
    // Remaining lines after last LCS match
    while oi < old.len() {
        ops.push(DiffOp::Remove { old_idx: oi });
        oi += 1;
    }
    while ni < new.len() {
        ops.push(DiffOp::Insert { new_idx: ni });
        ni += 1;
    }

    ops
}

/// Compute the LCS (Longest Common Subsequence) between two slices of lines.
/// Returns pairs of (old_index, new_index) for matching lines.
fn compute_lcs(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let m = old.len();
    let n = new.len();

    if m == 0 || n == 0 {
        return Vec::new();
    }

    // For very large files, use a simplified approach to avoid excessive memory use.
    // Cap the DP table at 10000x10000.
    if m > 10000 || n > 10000 {
        return compute_lcs_simple(old, new);
    }

    // Standard LCS dynamic programming
    let mut dp = vec![vec![0usize; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if old[i - 1] == new[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to recover the LCS
    let mut result = Vec::new();
    let mut i = m;
    let mut j = n;
    while i > 0 && j > 0 {
        if old[i - 1] == new[j - 1] {
            result.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] > dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    result.reverse();
    result
}

/// Simplified LCS for large files: matches by line equality scanning forward.
/// Produces a subset of the true LCS by greedily matching lines from left to right.
fn compute_lcs_simple(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let mut last_new = 0;

    for (oi, old_line) in old.iter().enumerate() {
        for (ni, new_line) in new.iter().enumerate().skip(last_new) {
            if old_line == new_line {
                result.push((oi, ni));
                last_new = ni + 1;
                break;
            }
        }
    }

    result
}

fn find_actual_string(file_content: &str, search_string: &str) -> Option<String> {
    // First try exact match
    if file_content.contains(search_string) {
        return Some(search_string.to_owned());
    }
    // Try with normalized quotes — both sides get curly→straight normalization.
    // Use character-level indexing to avoid slicing on non-character boundaries
    // when the normalized string differs in length from the original.
    let normalized_search = normalize_quotes(search_string);
    let normalized_file = normalize_quotes(file_content);
    if let Some(byte_index) = normalized_file.find(&normalized_search) {
        // Count characters up to the match position in the normalized string,
        // then use that character count to index into the original string.
        let char_offset = normalized_file[..byte_index].chars().count();
        let char_len = normalized_search.chars().count();
        let start_ok = file_content.char_indices().nth(char_offset).map(|(b, _)| b);
        let end_ok = file_content
            .char_indices()
            .nth(char_offset + char_len)
            .map(|(b, _)| b)
            .unwrap_or(file_content.len());
        if let Some(start) = start_ok {
            return Some(file_content[start..end_ok].to_owned());
        }
    }
    // Try with de-sanitized tag names — maps abbreviated forms back to real ones.
    // Matches TS desanitizeMatchString / DESANITIZATIONS.
    let desanitized = desanitize_match_string(search_string);
    if desanitized != search_string && file_content.contains(&desanitized) {
        return Some(desanitized);
    }
    None
}

/// Map of sanitized tag abbreviations to their real forms, matching TS DESANITIZATIONS.
fn desanitize_match_string(s: &str) -> String {
    let mut result = s.to_owned();
    for (sanitized, real) in DESANITIZATIONS {
        result = result.replace(sanitized, real);
    }
    result
}

const DESANITIZATIONS: &[(&str, &str)] = &[
    ("<fnr>", "<function_results>"),
    ("</fnr>", "</function_results>"),
    ("<n>", "<name>"),
    ("</n>", "</name>"),
    ("<o>", "<output>"),
    ("</o>", "</output>"),
    ("<e>", "<error>"),
    ("</e>", "</error>"),
    ("<s>", "<system>"),
    ("</s>", "</system>"),
    ("<r>", "<result>"),
    ("</r>", "</result>"),
    ("< META_START >", "<META_START>"),
    ("\n\nH:", "\n\nHuman:"),
    ("\n\nA:", "\n\nAssistant:"),
];

/// Preserves curly quote style from the file content in the replacement string.
///
/// When the file uses curly quotes (e.g. `\u{201c}...\u{201d}`) but the user
/// typed straight quotes in `old_string`, this function maps the straight quotes in
/// `new_string` back to curly style so the replaced text blends in. Matches TS
/// `preserveQuoteStyle`.
fn preserve_quote_style(user_old: &str, actual_old: &str, new_str: &str) -> String {
    // Only transform when the file uses a curly quote that the user typed as straight.
    let has_curly_double = actual_old.contains('\u{201C}') || actual_old.contains('\u{201D}');
    let has_curly_single = actual_old.contains('\u{2018}') || actual_old.contains('\u{2019}');
    let user_has_straight_double = user_old.contains('"');
    let user_has_straight_single = user_old.contains('\'');

    let mut result = new_str.to_owned();
    if has_curly_double && user_has_straight_double {
        // Replace each straight double quote with the curly variant found in the file.
        // Prefer \u{201C} for opening, \u{201D} for closing.
        result = result.replace('"', "\u{201D}");
        // Crude heuristic: first occurrence is opening quote if file has both.
        if let Some(pos) = result.find('\u{201D}') {
            result.replace_range(pos..pos + '\u{201D}'.len_utf8(), "\u{201C}");
        }
    }
    if has_curly_single && user_has_straight_single {
        result = result.replace('\'', "\u{2019}");
        if let Some(pos) = result.find('\u{2019}') {
            result.replace_range(pos..pos + '\u{2019}'.len_utf8(), "\u{2018}");
        }
    }
    result
}

fn normalize_for_comparison(path: PathBuf) -> PathBuf {
    let rendered = path.to_string_lossy();
    if let Some(stripped) = rendered.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path
    }
}

fn file_mtime_ms(path: &Path) -> Result<u128> {
    let modified = std::fs::metadata(path)?.modified()?;
    Ok(modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis())
}

fn ensure_current_read_state(
    context: &ToolExecutionContext,
    target: &Path,
    current_content: &str,
) -> Result<()> {
    let Some(read_state) = context.read_file_state.get(target) else {
        return Err(anyhow!(
            "File has not been read yet. Read it first before writing to it."
        ));
    };
    if read_state.is_partial_view {
        return Err(anyhow!(
            "File has not been read yet. Read it first before writing to it."
        ));
    }

    let last_write_time = file_mtime_ms(target)?;
    if last_write_time > read_state.timestamp {
        let content_unchanged = read_state.offset.is_none()
            && read_state.limit.is_none()
            && current_content == read_state.content;
        if !content_unchanged {
            return Err(anyhow!(
                "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it."
            ));
        }
    }
    Ok(())
}

fn ensure_current_read_state_before_atomic_write(
    context: &ToolExecutionContext,
    target: &Path,
    current_content: &str,
) -> Result<()> {
    let Some(read_state) = context.read_file_state.get(target) else {
        return Err(anyhow!(FILE_UNEXPECTEDLY_MODIFIED_ERROR));
    };
    let last_write_time = file_mtime_ms(target)?;
    if last_write_time > read_state.timestamp {
        let content_unchanged = read_state.offset.is_none()
            && read_state.limit.is_none()
            && current_content == read_state.content;
        if !content_unchanged {
            return Err(anyhow!(FILE_UNEXPECTEDLY_MODIFIED_ERROR));
        }
    }
    Ok(())
}

fn update_post_write_state(context: &ToolExecutionContext, target: &Path, content: String) {
    if let Ok(timestamp) = file_mtime_ms(target) {
        context
            .read_file_state
            .set(target, FileState::post_write(content, timestamp));
    }
}

pub(crate) fn file_path_input(input: &Value) -> Option<&str> {
    input
        .get("file_path")
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
}

pub(crate) fn resolve_workspace_path_for_operation(
    context: &ToolExecutionContext,
    maybe_relative: Option<&str>,
    operation: FilesystemOperation,
) -> Result<PathBuf> {
    let raw_path = maybe_relative.unwrap_or(".");
    let options = crate::filesystem_access_options();
    let check = assess_filesystem_access(raw_path, &context.cwd, &options, operation);

    if check.allowed
        || (check.requires_confirmation && filesystem_permission_confirmed_for_dispatch())
    {
        return Ok(check.normalized_path);
    }

    Err(anyhow!(
        "{}: {}",
        check
            .reason
            .unwrap_or_else(|| "Path is not allowed".to_owned()),
        check.normalized_path.display()
    ))
}

fn canonical_plan_file_path(plan_file_path: &Path) -> PathBuf {
    if plan_file_path.exists() {
        normalize_for_comparison(
            plan_file_path
                .canonicalize()
                .unwrap_or_else(|_| plan_file_path.to_path_buf()),
        )
    } else {
        let parent = plan_file_path.parent().unwrap_or_else(|| Path::new("."));
        let file_name = plan_file_path.file_name().unwrap_or_default();
        let canonical_parent = normalize_for_comparison(
            parent
                .canonicalize()
                .unwrap_or_else(|_| parent.to_path_buf()),
        );
        canonical_parent.join(file_name)
    }
}

fn maybe_persist_plan_snapshot(target: &Path) {
    let target_path = canonical_plan_file_path(target);
    let is_active_plan_file = current_plan_file_path()
        .as_ref()
        .map(|plan_path| canonical_plan_file_path(plan_path))
        .is_some_and(|plan_path| plan_path == target_path);
    if is_active_plan_file {
        let _ = crate::plan_mode::persist_plan_snapshot_if_active();
    }
}

pub(crate) fn list_directory(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let target = resolve_workspace_path_for_operation(
        context,
        input.get("path").and_then(Value::as_str),
        FilesystemOperation::Read,
    )?;
    let recursive = input
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_entries = input
        .get("max_entries")
        .and_then(Value::as_u64)
        .unwrap_or(200) as usize;
    let mut builder = WalkBuilder::new(&target);
    builder.hidden(false);
    if !recursive {
        builder.max_depth(Some(1));
    }
    let mut lines = Vec::new();
    for entry in builder.build().take(max_entries) {
        let entry = entry?;
        let path = entry.path();
        if path == target {
            continue;
        }
        if path.components().any(|component| {
            IGNORED_DIRS.contains(&component.as_os_str().to_string_lossy().as_ref())
        }) {
            continue;
        }
        let relative = path.strip_prefix(&context.cwd).unwrap_or(path);
        let marker = if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_dir())
        {
            "dir"
        } else {
            "file"
        };
        lines.push(format!("[{marker}] {}", relative.display()));
    }
    if lines.is_empty() {
        Ok("No files matched.".to_owned())
    } else {
        Ok(lines.join("\n"))
    }
}

pub fn read_file(input: &Value, context: &ToolExecutionContext) -> Result<ToolResult> {
    let path = file_path_input(input).ok_or_else(|| anyhow!("read_file requires file_path"))?;
    if is_blocked_device_path(path) {
        return Err(anyhow!(
            "Cannot read '{}': this device file would block or produce infinite output.",
            path
        ));
    }
    let target =
        resolve_workspace_path_for_operation(context, Some(path), FilesystemOperation::Read)?;
    let start_line = input
        .get("offset")
        .or_else(|| input.get("start_line"))
        .and_then(Value::as_u64)
        .unwrap_or(1) as usize;
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let end_line = limit
        .map(|limit| start_line.saturating_add(limit).saturating_sub(1))
        .or_else(|| {
            input
                .get("end_line")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
        })
        .unwrap_or(usize::MAX);
    let max_chars = input
        .get("max_chars")
        .and_then(Value::as_u64)
        .unwrap_or(100_000) as usize; // ~25K tokens at 4 chars/token

    // File size pre-check: refuse to read files larger than 256 KB without
    // offset/limit, to avoid excessive token consumption. Matches TS MAX_OUTPUT_SIZE.
    if let Ok(metadata) = std::fs::metadata(&target)
        && metadata.len() > 262_144
        && limit.is_none()
    {
        return Err(anyhow!(
            "File too large ({} bytes). Use offset/limit to read portions.",
            metadata.len()
        ));
    }

    if let Some(read_state) = context.read_file_state.get(&target)
        && !read_state.is_partial_view
        && read_state.offset == Some(start_line)
        && read_state.limit == limit
        && let Ok(mtime) = file_mtime_ms(&target)
        && mtime <= read_state.timestamp
    {
        return Ok(ToolResult {
            content: FILE_UNCHANGED_STUB.to_owned(),
            ..ToolResult::default()
        });
    }

    let ext = target
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // ── Image files: return as multimodal base64 image blocks ──────────
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return read_image_file(&target, &ext);
    }

    // ── SVG files: read as text (they are XML-based) ───────────────────
    if ext == "svg" {
        let contents = std::fs::read_to_string(&target)
            .with_context(|| format!("failed to read {}", target.display()))?;
        if let Ok(timestamp) = file_mtime_ms(&target) {
            context.read_file_state.set(
                &target,
                FileState::read(contents.clone(), timestamp, start_line, limit),
            );
        }
        return Ok(ToolResult {
            content: contents,
            ..ToolResult::default()
        });
    }

    // ── PDF files ──────────────────────────────────────────────────────
    if ext == "pdf" {
        let file_size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        let pages_param = input.get("pages").and_then(Value::as_str);

        // Try pdftotext (poppler-utils) for text extraction
        if let Ok(text) = extract_pdf_via_pdftotext(&target, pages_param, file_size) {
            if let Ok(timestamp) = file_mtime_ms(&target) {
                context.read_file_state.set(
                    &target,
                    FileState::read(text.clone(), timestamp, start_line, limit),
                );
            }
            return Ok(ToolResult {
                content: text,
                ..ToolResult::default()
            });
        }

        // Try pdftotext without page range as fallback
        if pages_param.is_some()
            && let Ok(text) = extract_pdf_via_pdftotext(&target, None, file_size)
        {
            if let Ok(timestamp) = file_mtime_ms(&target) {
                context.read_file_state.set(
                    &target,
                    FileState::read(text.clone(), timestamp, start_line, limit),
                );
            }
            return Ok(ToolResult {
                content: text,
                ..ToolResult::default()
            });
        }

        return Err(anyhow!(
            "PDF reading requires `pdftotext` from poppler-utils. \
             Install with: `brew install poppler` (macOS) or `apt-get install poppler-utils` (Debian/Ubuntu).\n\
             File: {} ({} bytes)",
            target.display(),
            file_size
        ));
    }

    // ── Notebook (.ipynb) files ────────────────────────────────────────
    if ext == "ipynb" {
        let contents = std::fs::read_to_string(&target)
            .with_context(|| format!("failed to read {}", target.display()))?;
        let rendered = render_notebook_cells(&contents, start_line, end_line, max_chars)?;
        if let Ok(timestamp) = file_mtime_ms(&target) {
            let raw_cells =
                render_notebook_cells(&contents, 1, usize::MAX, usize::MAX).unwrap_or_default();
            context.read_file_state.set(
                &target,
                FileState::read(raw_cells, timestamp, start_line, limit),
            );
        }
        return Ok(ToolResult {
            content: rendered,
            ..ToolResult::default()
        });
    }

    // ── Binary file detection ──────────────────────────────────────────
    // Try reading as text first. If that fails (likely binary), check if
    // the file appears to be binary and return a helpful error.
    let contents = match std::fs::read_to_string(&target) {
        Ok(c) => c,
        Err(err) if err.kind() == std::io::ErrorKind::InvalidData => {
            // The file is likely binary. Read a small sample to confirm.
            return Err(anyhow!(
                "File appears to be a binary file that cannot be displayed as text. \
                 Only text files, images (PNG, JPG, GIF, WebP), and PDFs are supported.\n\
                 File: {}",
                target.display()
            ));
        }
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", target.display()));
        }
    };

    let raw_selected = contents
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_number = index + 1;
            if line_number < start_line || line_number > end_line {
                None
            } else {
                Some(line.to_owned())
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let selected = contents
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_number = index + 1;
            if line_number < start_line || line_number > end_line {
                None
            } else {
                Some(format!("{line_number:>4} {line}"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let selected = selected.chars().take(max_chars).collect::<String>();
    if let Ok(timestamp) = file_mtime_ms(&target) {
        context.read_file_state.set(
            &target,
            FileState::read(raw_selected, timestamp, start_line, limit),
        );
    }
    Ok(ToolResult {
        content: selected,
        ..ToolResult::default()
    })
}

/// Read an image file, optionally resize it, and return it as a multimodal
/// Anthropic API image content block embedded in a [`ToolResult`].
///
/// The image is resized when its longest side exceeds `MAX_IMAGE_DIMENSION`
/// (1600 px). Resizing decodes the image, scales it down with Lanczos3
/// filtering, and re-encodes it as PNG for consistent API submission.
fn read_image_file(target: &Path, ext: &str) -> Result<ToolResult> {
    let data = std::fs::read(target)
        .with_context(|| format!("failed to read image {}", target.display()))?;

    let mime = match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };

    // Try to decode the image and resize if it exceeds the max dimension.
    let (base64_data, dimensions) = match image::load_from_memory(&data) {
        Ok(img) => {
            let (orig_w, orig_h) = (img.width(), img.height());
            let max_dim = orig_w.max(orig_h);

            let (final_img, (w, h)) = if max_dim > MAX_IMAGE_DIMENSION {
                let resized = img.resize(
                    MAX_IMAGE_DIMENSION,
                    MAX_IMAGE_DIMENSION,
                    image::imageops::FilterType::Lanczos3,
                );
                let dims = (resized.width(), resized.height());
                (resized, dims)
            } else {
                (img, (orig_w, orig_h))
            };

            // Re-encode as PNG for consistent base64 output
            let mut buf = Vec::new();
            final_img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)?;
            let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &buf);
            (encoded, (w, h))
        }
        Err(_) => {
            // If the image crate cannot decode it (e.g. animated GIF, unsupported
            // sub-format), fall back to sending the raw bytes as-is with the
            // original MIME type.
            let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
            (encoded, (0, 0))
        }
    };

    // Build the text summary that accompanies the image block
    let summary = if dimensions != (0, 0) {
        format!(
            "Image file: {} ({}x{}, {} bytes)",
            target.display(),
            dimensions.0,
            dimensions.1,
            data.len()
        )
    } else {
        format!("Image file: {} ({} bytes)", target.display(), data.len())
    };

    // Build the Anthropic API image content block
    let image_block = json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": if dimensions != (0, 0) { "image/png" } else { mime },
            "data": base64_data,
        }
    });

    Ok(ToolResult {
        content: summary,
        content_blocks: vec![image_block],
        ..ToolResult::default()
    })
}

pub(crate) fn search_text(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let pattern = input
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("search_text requires a pattern"))?;
    let target = resolve_workspace_path_for_operation(
        context,
        input.get("path").and_then(Value::as_str),
        FilesystemOperation::Read,
    )?;
    let regex = Regex::new(pattern).or_else(|_| Regex::new(&regex::escape(pattern)))?;
    let max_matches = input
        .get("max_matches")
        .and_then(Value::as_u64)
        .unwrap_or(50) as usize;
    let mut matches = Vec::new();
    for entry in WalkDir::new(&target).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().components().any(|component| {
            IGNORED_DIRS.contains(&component.as_os_str().to_string_lossy().as_ref())
        }) {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for (index, line) in contents.lines().enumerate() {
            if regex.is_match(line) {
                let relative = entry
                    .path()
                    .strip_prefix(&context.cwd)
                    .unwrap_or(entry.path());
                matches.push(format!(
                    "{}:{}:{}",
                    relative.display(),
                    index + 1,
                    line.trim()
                ));
                if matches.len() >= max_matches {
                    return Ok(matches.join("\n"));
                }
            }
        }
    }
    if matches.is_empty() {
        Ok("No matches found.".to_owned())
    } else {
        Ok(matches.join("\n"))
    }
}

pub(crate) fn write_file(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let path = file_path_input(input).ok_or_else(|| anyhow!("write_file requires file_path"))?;
    let content = input
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("write_file requires content"))?;
    let append = input
        .get("append")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let target =
        resolve_workspace_path_for_operation(context, Some(path), FilesystemOperation::Create)?;
    let existing = match std::fs::read_to_string(&target) {
        Ok(existing) => Some(existing),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if let Some(existing) = existing.as_deref() {
        ensure_current_read_state(context, &target, existing)?;
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if append {
        let existing_str = existing.as_deref().unwrap_or_default();
        ensure_current_read_state_before_atomic_write(context, &target, existing_str).or_else(
            |error| {
                if target.exists() { Err(error) } else { Ok(()) }
            },
        )?;
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&target)?;
        file.write_all(content.as_bytes())?;
    } else {
        if let Some(existing) = existing.as_deref() {
            ensure_current_read_state_before_atomic_write(context, &target, existing)?;
        }
        std::fs::write(&target, content)?;
    }
    let final_content = if append {
        std::fs::read_to_string(&target).unwrap_or_else(|_| content.to_owned())
    } else {
        content.to_owned()
    };
    update_post_write_state(context, &target, final_content.clone());
    maybe_persist_plan_snapshot(&target);

    // Build structured result with line counts and unified diff.
    let new_lines = final_content.lines().count();
    let path_display = target.display().to_string();
    match existing {
        Some(old) => {
            let old_lines = old.lines().count();
            let added = new_lines.saturating_sub(old_lines);
            let removed = old_lines.saturating_sub(new_lines);
            if added > 0 || removed > 0 || old != final_content {
                let diff = compute_unified_diff(&old, &final_content, &path_display);
                let mut result = format!(
                    "Wrote {}\n{} → {} lines (+{}/−{})",
                    path_display, old_lines, new_lines, added, removed,
                );
                if !diff.is_empty() {
                    result.push('\n');
                    result.push_str(&diff);
                }
                Ok(result)
            } else {
                Ok(format!(
                    "Wrote {} (unchanged, {} lines)",
                    path_display, new_lines
                ))
            }
        }
        None => Ok(format!("Created {} ({} lines)", path_display, new_lines)),
    }
}

pub(crate) fn replace_in_file(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let path =
        file_path_input(input).ok_or_else(|| anyhow!("replace_in_file requires file_path"))?;
    let search = input
        .get("search")
        .or_else(|| input.get("old_string"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("replace_in_file requires search text"))?;
    let replace = input
        .get("replace")
        .or_else(|| input.get("new_string"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("replace_in_file requires replacement text"))?;
    let replace_all = input
        .get("all")
        .or_else(|| input.get("replace_all"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let target =
        resolve_workspace_path_for_operation(context, Some(path), FilesystemOperation::Write)?;
    // Read file bytes and handle UTF-16LE BOM encoding.
    let raw_bytes =
        std::fs::read(&target).with_context(|| format!("failed to read {}", target.display()))?;
    let mut is_utf16le = false;
    let original = if has_utf16le_bom(&raw_bytes) {
        is_utf16le = true;
        let utf16_bytes = &raw_bytes[2..];
        let u16_vec: Vec<u16> = utf16_bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        char::decode_utf16(u16_vec)
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect::<String>()
    } else {
        String::from_utf8_lossy(&raw_bytes).into_owned()
    };

    ensure_current_read_state(context, &target, &original)?;
    if search == replace {
        return Err(anyhow!(
            "No changes to make: old_string and new_string are exactly the same."
        ));
    }

    // Apply quote normalization: normalize quotes in both the file content and the
    // search needle before matching, so curly/smart quotes in the file can be
    // matched by straight quotes from the user. Reuse the same helpers as edit_file.
    let normalized_original = normalize_quotes(&original);
    let normalized_search = normalize_quotes(search);

    let actual_search = match find_actual_string(&normalized_original, &normalized_search) {
        Some(s) => s,
        None => {
            return Err(anyhow!(
                "String to replace not found in file.\nString: {search}"
            ));
        }
    };

    // Map the normalized match back to the original content to get the actual text.
    // We find the actual text from the original file using the same char-offset logic.
    let actual_original_search = if let Some(byte_index) = normalized_original.find(&actual_search)
    {
        let char_offset = normalized_original[..byte_index].chars().count();
        let char_len = actual_search.chars().count();
        let start_ok = original.char_indices().nth(char_offset).map(|(b, _)| b);
        let end_ok = original
            .char_indices()
            .nth(char_offset + char_len)
            .map(|(b, _)| b)
            .unwrap_or(original.len());
        if let Some(start) = start_ok {
            original[start..end_ok].to_owned()
        } else {
            actual_search.clone()
        }
    } else {
        actual_search.clone()
    };

    // Preserve curly quote style in the replacement when the file uses them.
    let actual_replace = preserve_quote_style(search, &actual_original_search, replace);

    let match_count = original.matches(&*actual_original_search).count();
    if match_count > 1 && !replace_all {
        return Err(anyhow!(
            "Found {match_count} occurrences of the search string. Use 'all: true' to replace all, or provide a more specific search string."
        ));
    }
    let updated = if replace_all {
        original.replace(&*actual_original_search, &actual_replace)
    } else {
        original.replacen(&*actual_original_search, &actual_replace, 1)
    };
    ensure_current_read_state_before_atomic_write(context, &target, &original)?;

    // Write back in the original encoding (UTF-16LE with BOM, or plain UTF-8).
    if is_utf16le {
        let mut out_bytes: Vec<u8> = vec![0xFF, 0xFE]; // UTF-16LE BOM
        let mut utf16_buf = [0u16; 2];
        for ch in updated.chars() {
            let utf16_slice = ch.encode_utf16(&mut utf16_buf);
            for &code_unit in utf16_slice.iter() {
                let le = code_unit.to_le_bytes();
                out_bytes.push(le[0]);
                out_bytes.push(le[1]);
            }
        }
        std::fs::write(&target, &out_bytes)?;
    } else {
        std::fs::write(&target, &updated)?;
    }

    let written_back = std::fs::read_to_string(&target).unwrap_or_default();
    update_post_write_state(context, &target, written_back);
    maybe_persist_plan_snapshot(&target);

    // Build structured result with diff
    let path_display = target.display().to_string();
    let old_lines = original.lines().count();
    let new_lines = updated.lines().count();
    let added = new_lines.saturating_sub(old_lines);
    let removed = old_lines.saturating_sub(new_lines);
    let diff = compute_unified_diff(&original, &updated, &path_display);
    let mut result = format!(
        "Updated {}\n{} → {} lines (+{}/−{})",
        path_display, old_lines, new_lines, added, removed,
    );
    if !diff.is_empty() {
        result.push('\n');
        result.push_str(&diff);
    }
    Ok(result)
}

pub(crate) fn edit_file(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let path = file_path_input(input).ok_or_else(|| anyhow!("edit_file requires file_path"))?;

    // Reject edits to .ipynb files — the NotebookEdit tool is the dedicated path.
    // Matches TS FileEditTool validateInput guard.
    if path.to_lowercase().ends_with(".ipynb") {
        return Err(anyhow!(
            "File is a Jupyter Notebook. Use the NotebookEdit tool to edit this file."
        ));
    }

    let target =
        resolve_workspace_path_for_operation(context, Some(path), FilesystemOperation::Write)?;

    // File size guard: prevent OOM on multi-GB files. 1 GiB matches TS MAX_EDIT_FILE_SIZE.
    const MAX_EDIT_FILE_SIZE: u64 = 1024 * 1024 * 1024; // 1 GiB
    if target.exists()
        && let Ok(metadata) = std::fs::metadata(&target)
        && metadata.len() > MAX_EDIT_FILE_SIZE
    {
        return Err(anyhow!(
            "File is too large to edit ({} bytes). Maximum editable file size is {} GiB.",
            metadata.len(),
            MAX_EDIT_FILE_SIZE / (1024 * 1024 * 1024)
        ));
    }

    let legacy_edits;
    let edits = if let Some(edits) = input.get("edits").and_then(Value::as_array) {
        edits
    } else {
        let old_string = input
            .get("old_string")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("edit_file requires old_string"))?;
        let new_string = input
            .get("new_string")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("edit_file requires new_string"))?;
        if old_string == new_string {
            return Err(anyhow!(
                "No changes to make: old_string and new_string are exactly the same."
            ));
        }
        legacy_edits = vec![serde_json::json!({
            "search": old_string,
            "replace": new_string,
            "all": input
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })];
        &legacy_edits
    };
    let create_if_missing = input
        .get("create_if_missing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // Track file encoding for round-trip preservation.
    let mut is_utf16le = false;
    // Detect and preserve line endings across edits.
    let mut file_line_ending: Option<LineEnding> = None;
    let mut content = if target.exists() {
        let raw_bytes = std::fs::read(&target)?;
        let content = if has_utf16le_bom(&raw_bytes) {
            is_utf16le = true;
            // Strip the BOM (2 bytes) and decode UTF-16LE to UTF-8.
            let utf16_bytes = &raw_bytes[2..];
            let u16_vec: Vec<u16> = utf16_bytes
                .chunks_exact(2)
                .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                .collect();
            // Use char::decode_utf16 for proper handling of surrogates.
            char::decode_utf16(u16_vec)
                .map(|r| r.unwrap_or('\u{FFFD}'))
                .collect::<String>()
        } else {
            String::from_utf8_lossy(&raw_bytes).into_owned()
        };
        // Detect the file's line ending style before any edits.
        file_line_ending = Some(detect_line_ending(&content));
        ensure_current_read_state(context, &target, &content)?;
        content
    } else if create_if_missing
        || edits
            .first()
            .and_then(|edit| edit.get("search").and_then(Value::as_str))
            == Some("")
    {
        String::new()
    } else {
        return Err(anyhow!("{} does not exist", target.display()));
    };
    let original_content = content.clone();
    for edit in edits {
        let search = edit
            .get("search")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("edit is missing search"))?;
        let replace = edit
            .get("replace")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("edit is missing replace"))?;
        let original_search = search.to_owned();
        let original_replace = replace.to_owned();
        let search = normalize_quotes(search);
        let replace = normalize_quotes(replace);
        let replace_all = edit.get("all").and_then(Value::as_bool).unwrap_or(false);
        if search.is_empty() {
            if content.is_empty() {
                content = replace.to_owned();
                continue;
            }
            return Err(anyhow!("Cannot create new file - file already exists."));
        }
        if search == replace {
            return Err(anyhow!(
                "No changes to make: old_string and new_string are exactly the same."
            ));
        }
        let actual_search = match find_actual_string(&content, &search) {
            Some(s) => s,
            None => {
                return Err(anyhow!(
                    "String to replace not found in file.\nString: {search}"
                ));
            }
        };
        // Preserve curly quote style in the replacement when the file uses them
        // and the user typed straight quotes. Matches TS preserveQuoteStyle.
        let mut actual_replace =
            preserve_quote_style(&original_search, &actual_search, &original_replace);
        // Preserve line endings: normalize the replacement text to match the
        // file's detected line ending style (CRLF or LF).
        if let Some(le) = file_line_ending {
            actual_replace = normalize_line_endings(&actual_replace, le);
        }
        let matches = content.matches(&*actual_search).count();
        if matches > 1 && !replace_all {
            return Err(anyhow!(
                "Found {matches} matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, please provide more context to uniquely identify the instance.\nString: {search}"
            ));
        }
        content = if replace_all {
            content.replace(&*actual_search, &actual_replace)
        } else {
            content.replacen(&*actual_search, &actual_replace, 1)
        };
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if target.exists() {
        ensure_current_read_state_before_atomic_write(context, &target, &original_content)?;
    }
    // Write back in the original encoding (UTF-16LE with BOM, or plain UTF-8).
    if is_utf16le {
        let mut out_bytes: Vec<u8> = vec![0xFF, 0xFE]; // UTF-16LE BOM
        let mut utf16_buf = [0u16; 2];
        for ch in content.chars() {
            let utf16_slice = ch.encode_utf16(&mut utf16_buf);
            for &code_unit in utf16_slice.iter() {
                let le = code_unit.to_le_bytes();
                out_bytes.push(le[0]);
                out_bytes.push(le[1]);
            }
        }
        std::fs::write(&target, &out_bytes)?;
    } else {
        std::fs::write(&target, &content)?;
    }
    let updated = std::fs::read_to_string(&target).unwrap_or_default();
    update_post_write_state(context, &target, updated.clone());
    maybe_persist_plan_snapshot(&target);
    // Compute diff stats and unified diff
    let old_lines: Vec<&str> = original_content.lines().collect();
    let new_lines: Vec<&str> = content.lines().collect();
    let lines_changed = old_lines.len().abs_diff(new_lines.len())
        + old_lines
            .iter()
            .zip(new_lines.iter())
            .filter(|(a, b)| a != b)
            .count();
    let path_display = target.display().to_string();
    let diff = compute_unified_diff(&original_content, &content, &path_display);
    let mut result = format!(
        "Applied {} edit(s) to {} ({} lines affected)",
        edits.len(),
        path_display,
        lines_changed
    );
    if !diff.is_empty() {
        result.push('\n');
        result.push_str(&diff);
    }
    Ok(result)
}

pub(crate) fn glob_files(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let pattern = input
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("glob requires a pattern"))?;
    let base = resolve_workspace_path_for_operation(
        context,
        input.get("path").and_then(Value::as_str),
        FilesystemOperation::Read,
    )?;
    const GLOB_MAX_RESULTS: usize = 100;
    let full_pattern = format!("{}/{}", base.display(), pattern).replace('\\', "/");
    let mut results: Vec<(String, u128)> = Vec::new(); // (relative_path, mtime_ms)
    let entries = glob::glob(&full_pattern).context("invalid glob pattern")?;
    for entry in entries {
        let path = match entry {
            Ok(p) => p,
            Err(_) => continue,
        };
        if path.is_dir() {
            continue;
        }
        let canonical_path = path.canonicalize().unwrap_or_else(|_| path.clone());
        let canonical_cwd = context
            .cwd
            .canonicalize()
            .unwrap_or_else(|_| context.cwd.clone());
        if !canonical_path.starts_with(&canonical_cwd) {
            continue;
        }
        let relative = path.strip_prefix(&context.cwd).unwrap_or(&path);
        let mtime = file_mtime_ms(&path).unwrap_or(0);
        results.push((relative.display().to_string(), mtime));
    }
    if results.is_empty() {
        Ok("No files matched.".to_owned())
    } else {
        let total_count = results.len();
        // Sort by modification time descending (newest first)
        results.sort_by(|a, b| b.1.cmp(&a.1));
        let truncated = results.len() > GLOB_MAX_RESULTS;
        results.truncate(GLOB_MAX_RESULTS);
        let mut output: String = results
            .iter()
            .map(|(p, _)| p.as_str())
            .collect::<Vec<&str>>()
            .join("\n");
        if truncated {
            output.push_str(&format!(
                "\n\n[Truncated: showing {} of {} results]",
                GLOB_MAX_RESULTS, total_count
            ));
        }
        Ok(output)
    }
}

pub(crate) fn grep_files(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let pattern = input
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("grep requires a pattern"))?;
    let target = resolve_workspace_path_for_operation(
        context,
        input.get("path").and_then(Value::as_str),
        FilesystemOperation::Read,
    )?;
    // Accept both "glob" (TS-compatible) and "include" (legacy) field names
    let glob_pattern = input
        .get("glob")
        .or_else(|| input.get("include"))
        .and_then(Value::as_str);
    // `type` parameter: filter by language (e.g. "js", "py", "rust") → ripgrep --type
    let type_filter = input.get("type").and_then(Value::as_str);
    let type_globs: Vec<String> = if let Some(tf) = type_filter {
        type_to_extensions(tf)
            .map(|exts| exts.iter().map(|e| e.to_string()).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    // `multiline` parameter: when true, `.` matches newlines (ripgrep --multiline-dotall)
    let multiline = input
        .get("multiline")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let output_mode = input
        .get("output_mode")
        .and_then(Value::as_str)
        .unwrap_or("files_with_matches");
    // head_limit: default 250, pass 0 for unlimited.
    let raw_head_limit = input
        .get("head_limit")
        .and_then(Value::as_u64)
        .unwrap_or(250) as usize;
    let head_limit = if raw_head_limit == 0 {
        usize::MAX
    } else {
        raw_head_limit
    };
    // offset: skip the first N results (used for pagination).
    let offset = input.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;

    // Context lines: `context` or `-C` overrides `-A` and `-B` (matches TS priority).
    // TS: context/C > -C > -B/-A. We treat `context` and `-C` as synonyms for the
    // symmetric "-C N" ripgrep mode — but only when neither -B nor -A is explicitly
    // set (TS precedence: context wins over everything else).
    let resolved_c = input
        .get("context")
        .or_else(|| input.get("-C"))
        .and_then(Value::as_u64);

    let context_before = if let Some(c) = resolved_c {
        c as usize
    } else {
        input.get("-B").and_then(Value::as_u64).unwrap_or(0) as usize
    };
    let context_after = if let Some(c) = resolved_c {
        c as usize
    } else {
        input.get("-A").and_then(Value::as_u64).unwrap_or(0) as usize
    };

    // Default to 0 context lines (matches TS reference behavior)
    let (ctx_before, ctx_after) = if context_before == 0 && context_after == 0 {
        (0, 0)
    } else {
        (context_before, context_after)
    };

    if !["content", "files_with_matches", "count"].contains(&output_mode) {
        return Err(anyhow!(
            "output_mode must be 'content', 'files_with_matches', or 'count'"
        ));
    }

    // -i flag for explicit case sensitivity control; auto-detect if not set
    let explicit_case_insensitive = input.get("-i").and_then(Value::as_bool);
    let case_insensitive = explicit_case_insensitive.unwrap_or_else(|| {
        pattern
            .chars()
            .all(|c| c.is_ascii_lowercase() || !c.is_alphabetic())
    });

    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .dot_matches_new_line(multiline)
        .build()
        .or_else(|_| {
            regex::RegexBuilder::new(&regex::escape(pattern))
                .dot_matches_new_line(multiline)
                .build()
        })?;

    // Build a combined glob matcher from both the explicit glob pattern and the
    // type-filter extensions.  If both are present a file must match *both*.
    let file_matcher: Option<globset::GlobMatcher> = match glob_pattern {
        Some(fp) => Some(
            GlobBuilder::new(fp)
                .literal_separator(true)
                .build()
                .context("invalid glob pattern")?
                .compile_matcher(),
        ),
        None => None,
    };
    let type_matcher: Option<globset::GlobMatcher> = if type_globs.is_empty() {
        None
    } else {
        // Build a single alternation glob like "*.{rs}" or "*.{js,jsx,mjs,cjs}"
        let combined = if type_globs.len() == 1 {
            type_globs[0].clone()
        } else {
            let alts: Vec<&str> = type_globs
                .iter()
                .map(|s| {
                    // Strip the leading "*." — globset alternation expects bare extensions
                    s.strip_prefix("*.").unwrap_or(s)
                })
                .collect();
            format!("*.{{{}}}", alts.join(","))
        };
        GlobBuilder::new(&combined)
            .literal_separator(true)
            .build()
            .ok()
            .map(|g| g.compile_matcher())
    };

    let mut walker = WalkBuilder::new(&target);
    walker.hidden(false).git_ignore(true).git_exclude(true);

    // Exclude VCS directories from traversal (matches TS ripgrep defaults).
    walker.filter_entry(move |entry| {
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            let name = entry.file_name();
            return !VCS_DIRS.contains(&name.to_string_lossy().as_ref());
        }
        true
    });

    let has_file_matcher = file_matcher.is_some();
    let has_type_matcher = type_matcher.is_some();
    if has_file_matcher || has_type_matcher {
        let fm = file_matcher;
        let tm = type_matcher;
        walker.filter_entry(move |entry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                return true;
            }
            let name = entry.path().file_name();
            let matches_file = match &fm {
                Some(m) => name.is_some_and(|n| m.is_match(n)),
                None => true,
            };
            let matches_type = match &tm {
                Some(m) => name.is_some_and(|n| m.is_match(n)),
                None => true,
            };
            matches_file && matches_type
        });
    }

    let mut files_with_matches: Vec<(PathBuf, u128)> = Vec::new();
    let mut count_per_file: Vec<(PathBuf, usize)> = Vec::new();
    let mut content_matches: Vec<String> = Vec::new();
    let mut total_content_matches = 0usize;
    let mut skipped_for_offset = 0usize;

    for entry in walker.build().filter_map(|e| e.ok()) {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path().to_path_buf();
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let relative = path
            .strip_prefix(&context.cwd)
            .unwrap_or(&path)
            .to_path_buf();
        let lines: Vec<&str> = contents.lines().collect();

        match output_mode {
            "files_with_matches" => {
                if lines.iter().any(|line| re.is_match(line)) {
                    let mtime = file_mtime_ms(&path).unwrap_or(0);
                    files_with_matches.push((relative, mtime));
                }
            }
            "count" => {
                let count = lines.iter().filter(|l| re.is_match(l)).count();
                if count > 0 {
                    count_per_file.push((relative, count));
                }
            }
            _ => {
                for (index, line) in lines.iter().enumerate() {
                    if re.is_match(line) {
                        total_content_matches += 1;
                        // Skip matches that fall within the offset range.
                        if skipped_for_offset < offset {
                            skipped_for_offset += 1;
                            continue;
                        }
                        let effective_match_idx = total_content_matches - skipped_for_offset;
                        if effective_match_idx <= head_limit {
                            let start = index.saturating_sub(ctx_before);
                            let end = (index + ctx_after + 1).min(lines.len());
                            for (ci, context_line) in lines[start..end].iter().enumerate() {
                                let line_idx = start + ci;
                                let prefix = if line_idx == index { ">" } else { " " };
                                let trimmed = context_line.trim_end();
                                let truncated = if trimmed.len() > MAX_COLUMNS {
                                    format!(
                                        "{}...",
                                        &trimmed[..trimmed.ceil_char_boundary(MAX_COLUMNS)]
                                    )
                                } else {
                                    trimmed.to_owned()
                                };
                                content_matches.push(format!(
                                    "{}:{}{} {}",
                                    relative.display(),
                                    line_idx + 1,
                                    prefix,
                                    truncated
                                ));
                            }
                            content_matches.push(String::new());
                        }
                    }
                }
            }
        }

        if output_mode == "content" && total_content_matches >= offset + head_limit {
            break;
        }
    }

    match output_mode {
        "files_with_matches" => {
            if files_with_matches.is_empty() {
                return Ok("No files matched.".to_owned());
            }
            files_with_matches.sort_by(|a, b| b.1.cmp(&a.1));
            let skip = offset.min(files_with_matches.len());
            files_with_matches.drain(..skip);
            let truncated = files_with_matches.len() > head_limit;
            let file_count = files_with_matches.len();
            files_with_matches.truncate(head_limit);
            let mut out: Vec<String> = files_with_matches
                .iter()
                .map(|(p, _)| format!("{}", p.display()))
                .collect();
            if truncated {
                out.push(
                    "\nFiles still truncated. Consider using a more specific path or pattern."
                        .to_owned(),
                );
            }
            let mut result = format!("Found {} file(s)\n", file_count.min(head_limit));
            result.push_str(&out.join("\n"));
            Ok(result)
        }
        "count" => {
            if count_per_file.is_empty() {
                return Ok("No files matched.".to_owned());
            }
            let total_matches: usize = count_per_file.iter().map(|(_, c)| c).sum();
            let total_files = count_per_file.len();
            let skip = offset.min(count_per_file.len());
            count_per_file.drain(..skip);
            let truncated = count_per_file.len() > head_limit;
            count_per_file.truncate(head_limit);
            let lines: Vec<String> = count_per_file
                .iter()
                .map(|(path, count)| format!("{}:{}", path.display(), count))
                .collect();
            let mut result = lines.join("\n");
            if truncated {
                result.push_str(
                    "\n\nFiles still truncated. Consider using a more specific path or pattern.",
                );
            }
            result.push_str(&format!(
                "\n\nFound {} total occurrences across {} files",
                total_matches, total_files
            ));
            Ok(result)
        }
        _ => {
            if content_matches.is_empty() {
                return Ok("No files matched.".to_owned());
            }
            let effective_total = total_content_matches.saturating_sub(skipped_for_offset);
            let truncated = effective_total > head_limit;
            if truncated {
                content_matches.push(format!(
                    "\n[Showing {} of {} results (skipped {} with offset). Use a more specific pattern to narrow results.]",
                    head_limit.min(effective_total),
                    effective_total,
                    skipped_for_offset
                ));
            }
            let mut result = content_matches.join("\n").trim_end().to_owned();
            // Truncate grep output if it exceeds MAX_RESULT_SIZE_CHARS.
            if result.len() > MAX_RESULT_SIZE_CHARS {
                result.truncate(result.ceil_char_boundary(MAX_RESULT_SIZE_CHARS));
                result.push_str("\n\n[Output truncated. Use a more specific pattern or narrower path to reduce results.]");
            }
            Ok(result)
        }
    }
}

fn current_plan_file_path() -> Option<PathBuf> {
    crate::plan_mode::current_plan_file_path()
}

fn extract_pdf_via_pdftotext(path: &Path, pages: Option<&str>, file_size: u64) -> Result<String> {
    let pdftotext = which_pdftotext()?;
    let mut cmd = std::process::Command::new(&pdftotext);
    cmd.arg("-layout");

    if let Some(page_range) = pages {
        let (start, end) = parse_pdf_page_range(page_range)?;
        if end - start + 1 > 20 {
            return Err(anyhow!(
                "Maximum 20 pages per read request. Requested {} pages.",
                end - start + 1
            ));
        }
        cmd.arg("-f").arg(start.to_string());
        cmd.arg("-l").arg(end.to_string());
    } else if file_size > 5_000_000 {
        cmd.arg("-f").arg("1").arg("-l").arg("20");
    }

    let output = cmd.arg(path).arg("-").output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("pdftotext failed: {}", stderr.trim()));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let truncated: String = text.chars().take(50_000).collect();
    let header = format!(
        "PDF file: {} ({} bytes)\n{}\n",
        path.display(),
        file_size,
        if let Some(p) = pages {
            format!("Pages: {p}")
        } else {
            "Full document".to_owned()
        }
    );
    Ok(format!("{}{}", header, truncated))
}

fn which_pdftotext() -> Result<PathBuf> {
    #[cfg(windows)]
    let resolver = "where";
    #[cfg(not(windows))]
    let resolver = "which";

    let output = std::process::Command::new(resolver)
        .arg("pdftotext")
        .output()?;

    if !output.status.success() {
        return Err(anyhow!("pdftotext not found in PATH"));
    }

    let path = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("pdftotext not found"))?;

    if path.exists() {
        Ok(path)
    } else {
        Err(anyhow!("pdftotext not found at {}", path.display()))
    }
}

fn parse_pdf_page_range(range: &str) -> Result<(u32, u32)> {
    let range = range.trim();
    if let Some((s, e)) = range.split_once('-') {
        let start: u32 = s
            .trim()
            .parse()
            .map_err(|_| anyhow!("Invalid page number: {}", s))?;
        let end: u32 = e
            .trim()
            .parse()
            .map_err(|_| anyhow!("Invalid page number: {}", e))?;
        if start == 0 || end == 0 || start > end {
            return Err(anyhow!("Invalid page range: {}", range));
        }
        return Ok((start, end));
    }
    let pages: Vec<u32> = range
        .split(',')
        .filter_map(|p| p.trim().parse().ok())
        .filter(|&p| p > 0)
        .collect();
    if pages.is_empty() {
        return Err(anyhow!("Invalid page range: {}", range));
    }
    let start = *pages.iter().min().expect("pages is non-empty after check");
    let end = *pages.iter().max().expect("pages is non-empty after check");
    Ok((start, end))
}

fn render_notebook_cells(
    raw: &str,
    start_line: usize,
    end_line: usize,
    max_chars: usize,
) -> Result<String> {
    let notebook: serde_json::Value =
        serde_json::from_str(raw).with_context(|| "failed to parse .ipynb JSON")?;

    let cells = notebook
        .get("cells")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("notebook has no cells array"))?;

    let mut lines = Vec::new();
    lines.push(format!("Notebook: {} cells\n", cells.len()));

    for (idx, cell) in cells.iter().enumerate() {
        let cell_type = cell
            .get("cell_type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let cell_id = cell.get("id").and_then(Value::as_str).unwrap_or("?");
        let source = cell
            .get("source")
            .and_then(|s| {
                if s.is_array() {
                    Some(
                        s.as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join("")
                            })
                            .unwrap_or_default(),
                    )
                } else {
                    s.as_str().map(|s| s.to_owned())
                }
            })
            .unwrap_or_default();

        let outputs = cell
            .get("outputs")
            .and_then(|o| o.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|out| {
                        out.get("text")
                            .or_else(|| out.get("data").and_then(|d| d.get("text/plain")))
                            .and_then(|t| {
                                if t.is_array() {
                                    Some(
                                        t.as_array()
                                            .map(|arr| {
                                                arr.iter()
                                                    .filter_map(Value::as_str)
                                                    .collect::<Vec<_>>()
                                                    .join("")
                                            })
                                            .unwrap_or_default(),
                                    )
                                } else {
                                    t.as_str().map(|s| s.to_owned())
                                }
                            })
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        let cell_header = format!("--- Cell {} ({}) [id={}] ---", idx + 1, cell_type, cell_id);
        lines.push(cell_header);
        for source_line in source.lines() {
            lines.push(source_line.to_owned());
        }
        if !outputs.is_empty() {
            lines.push("Output:".to_owned());
            for output_line in outputs.lines() {
                lines.push(format!("  {}", output_line));
            }
        }
        lines.push(String::new());
    }

    let all_text = lines.join("\n");
    let rendered: String = all_text
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let line_num = i + 1;
            if line_num < start_line || line_num > end_line {
                None
            } else {
                Some(format!("{line_num:>4} {line}"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    Ok(rendered.chars().take(max_chars).collect())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use once_cell::sync::Lazy;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::plan_mode::{self, ExitPlanModeInput, PlanModeRuntime, PlanModeRuntimeSnapshot};

    static FILE_OPS_TEST_MUTEX: Lazy<std::sync::Mutex<()>> =
        Lazy::new(|| std::sync::Mutex::new(()));

    #[derive(Debug)]
    struct StubPlanRuntime {
        plan_file_path: PathBuf,
    }

    impl PlanModeRuntime for StubPlanRuntime {
        fn enter_plan_mode(&self, _objective: &str) -> Result<String> {
            Ok(String::new())
        }

        fn exit_plan_mode(&self, _input: ExitPlanModeInput) -> Result<String> {
            Ok(String::new())
        }

        fn snapshot(&self) -> PlanModeRuntimeSnapshot {
            PlanModeRuntimeSnapshot {
                permission_mode: claude_core::PermissionMode::Plan,
                plan_file_path: Some(self.plan_file_path.clone()),
            }
        }

        fn persist_plan_snapshot(&self) -> Result<()> {
            // Stub: no-op for tests
            Ok(())
        }
    }

    #[test]
    fn resolve_workspace_path_allows_active_plan_file_outside_workspace() {
        let _guard = FILE_OPS_TEST_MUTEX.lock().expect("test mutex");
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        let plan_file = profile.join("plans").join("plan.md");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(plan_file.parent().expect("plan dir")).expect("plans");

        plan_mode::configure_plan_mode_runtime(Some(Arc::new(StubPlanRuntime {
            plan_file_path: plan_file.clone(),
        })))
        .expect("install plan runtime");

        let context = ToolExecutionContext {
            cwd: workspace.clone(),
            ..ToolExecutionContext::default()
        };
        let result = write_file(
            &json!({
                "path": plan_file.to_string_lossy().to_string(),
                "content": "# plan"
            }),
            &context,
        )
        .expect("plan file write");

        assert!(result.contains("plan.md"));
        assert_eq!(
            std::fs::read_to_string(plan_file).expect("plan file"),
            "# plan"
        );

        plan_mode::configure_plan_mode_runtime(None).expect("clear plan runtime");
    }
}
