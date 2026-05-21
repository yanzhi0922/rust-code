//! Post-compaction attachment handling.
//!
//! Provides functions for creating attachment messages that re-inject important
//! context (file contents, plan files, skills) after a compaction event.
//! Mirrors the attachment logic from `compact.ts`.

use claude_core::{
    Attachment, AttachmentMediaType, Message, MessageBase, MessageOrigin, UserMessage,
};

use crate::prompt::rough_token_count;

/// Maximum number of files to restore after compaction.
pub const POST_COMPACT_MAX_FILES_TO_RESTORE: usize = 5;

/// Total token budget for post-compact file attachments.
pub const POST_COMPACT_TOKEN_BUDGET: u64 = 50_000;

/// Maximum tokens per individual file attachment.
pub const POST_COMPACT_MAX_TOKENS_PER_FILE: u64 = 5_000;

/// Maximum tokens per skill attachment.
pub const POST_COMPACT_MAX_TOKENS_PER_SKILL: u64 = 5_000;

/// Total token budget for skill attachments.
pub const POST_COMPACT_SKILLS_TOKEN_BUDGET: u64 = 25_000;

/// Truncation marker appended to skill content that exceeds the per-skill budget.
const SKILL_TRUNCATION_MARKER: &str = "\n\n[... skill content truncated for compaction; use Read on the skill path if you need the full text]";

// ---------------------------------------------------------------------------
// File attachment state
// ---------------------------------------------------------------------------

/// Tracks a recently-read file for post-compact restoration.
#[derive(Debug, Clone)]
pub struct FileState {
    /// File path.
    pub filename: String,
    /// Last-known file content.
    pub content: String,
    /// Timestamp of the last read (epoch millis).
    pub timestamp: i64,
}

/// Collects file states for post-compact restoration.
#[derive(Debug, Clone, Default)]
pub struct FileStateCache {
    files: Vec<FileState>,
}

impl FileStateCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Upsert a file entry.
    pub fn insert(&mut self, filename: String, content: String, timestamp: i64) {
        if let Some(existing) = self.files.iter_mut().find(|f| f.filename == filename) {
            existing.content = content;
            existing.timestamp = timestamp;
        } else {
            self.files.push(FileState {
                filename,
                content,
                timestamp,
            });
        }
    }

    /// Remove a file entry, returning whether it existed.
    pub fn remove(&mut self, filename: &str) -> bool {
        let before = self.files.len();
        self.files.retain(|f| f.filename != filename);
        self.files.len() != before
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.files.clear();
    }

    /// Return a snapshot of the current state as a sorted (most-recent-first) vec.
    pub fn to_vec(&self) -> Vec<&FileState> {
        let mut v: Vec<&FileState> = self.files.iter().collect();
        v.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        v
    }

    /// Return the `limit` most recently accessed files, sorted by
    /// descending timestamp.
    pub fn most_recent(&self, limit: usize) -> Vec<&FileState> {
        let mut v: Vec<&FileState> = self.files.iter().collect();
        v.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        v.truncate(limit);
        v
    }

    /// Return the number of tracked files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Return whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Skill invocation tracking
// ---------------------------------------------------------------------------

/// Tracks a skill that was invoked during the session.
#[derive(Debug, Clone)]
pub struct InvokedSkill {
    /// Skill name.
    pub skill_name: String,
    /// Path to the skill file.
    pub skill_path: String,
    /// Full content of the skill file.
    pub content: String,
    /// Timestamp when the skill was invoked (epoch millis).
    pub invoked_at: i64,
}

/// Collects invoked skills scoped to an agent or the main session.
#[derive(Debug, Clone, Default)]
pub struct InvokedSkillRegistry {
    skills: Vec<InvokedSkill>,
}

impl InvokedSkillRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a skill invocation (upsert).
    pub fn invoke(&mut self, name: String, path: String, content: String, invoked_at: i64) {
        if let Some(existing) = self.skills.iter_mut().find(|s| s.skill_name == name) {
            existing.invoked_at = invoked_at;
            existing.content = content;
        } else {
            self.skills.push(InvokedSkill {
                skill_name: name,
                skill_path: path,
                content,
                invoked_at,
            });
        }
    }

    /// Return skills sorted most-recent-first.
    pub fn to_vec(&self) -> Vec<&InvokedSkill> {
        let mut v: Vec<&InvokedSkill> = self.skills.iter().collect();
        v.sort_by(|a, b| b.invoked_at.cmp(&a.invoked_at));
        v
    }

    /// Return whether any skills have been invoked.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Attachment creation helpers
// ---------------------------------------------------------------------------

/// Create a [`UserMessage`] wrapping a file attachment for post-compact
/// restoration.
///
/// The caller is responsible for reading the actual file content; this
/// function just wraps it in the proper message envelope.
pub fn create_file_attachment_message(
    filename: &str,
    content: &str,
    max_tokens: u64,
) -> Option<UserMessage> {
    let truncated = truncate_to_tokens(content, max_tokens);
    if truncated.is_empty() {
        return None;
    }

    Some(UserMessage {
        base: MessageBase::with_origin(MessageOrigin::Compact),
        text: String::new(),
        attachments: vec![Attachment::from_bytes(
            AttachmentMediaType::ApplicationPdf,
            truncated.as_bytes(),
            Some(filename.to_string()),
        )],
        provider_content_blocks: Vec::new(),
        summarize_metadata: None,
    })
}

/// Create post-compact file attachments from the file state cache.
///
/// Selects the most recently accessed files, respecting both the file-count
/// cap and the total token budget.  Files whose paths appear in
/// `preserved_read_paths` are skipped (their content is already visible in
/// the preserved conversation tail).
pub fn create_post_compact_file_attachments(
    file_state: &FileStateCache,
    max_files: usize,
    token_budget: u64,
    preserved_read_paths: &[String],
) -> Vec<Message> {
    let preserved_set: std::collections::HashSet<&str> =
        preserved_read_paths.iter().map(|s| s.as_str()).collect();

    let candidates: Vec<&FileState> = file_state
        .to_vec()
        .into_iter()
        .filter(|f| !preserved_set.contains(f.filename.as_str()))
        .take(max_files)
        .collect();

    let mut used_tokens: u64 = 0;
    let mut results = Vec::new();

    for file in candidates {
        let tokens = rough_token_count(&file.content);
        if used_tokens + tokens > token_budget {
            break;
        }

        if let Some(msg) = create_file_attachment_message(
            &file.filename,
            &file.content,
            POST_COMPACT_MAX_TOKENS_PER_FILE,
        ) {
            used_tokens += tokens;
            results.push(Message::User(msg));
        }
    }

    results
}

/// Create a plan-file attachment if plan content is provided.
///
/// Mirrors `createPlanAttachmentIfNeeded()`.
pub fn create_plan_attachment_if_needed(
    plan_content: Option<&str>,
    plan_file_path: Option<&str>,
) -> Option<Message> {
    let content = plan_content?;
    let path = plan_file_path.unwrap_or("<plan>");

    Some(Message::User(UserMessage {
        base: MessageBase::with_origin(MessageOrigin::Compact),
        text: format!("Plan file reference: {path}"),
        attachments: vec![Attachment::from_bytes(
            AttachmentMediaType::ApplicationPdf,
            content.as_bytes(),
            Some(path.to_string()),
        )],
        provider_content_blocks: Vec::new(),
        summarize_metadata: None,
    }))
}

/// Create a skill attachment for all invoked skills that fit within the budget.
///
/// Mirrors `createSkillAttachmentIfNeeded()`.
pub fn create_skill_attachment_if_needed(registry: &InvokedSkillRegistry) -> Option<Message> {
    if registry.is_empty() {
        return None;
    }

    let mut used_tokens: u64 = 0;
    let mut skills_out = Vec::new();

    for skill in registry.to_vec() {
        let truncated = truncate_to_tokens(&skill.content, POST_COMPACT_MAX_TOKENS_PER_SKILL);
        let tokens = rough_token_count(&truncated);
        if used_tokens + tokens > POST_COMPACT_SKILLS_TOKEN_BUDGET {
            continue;
        }
        used_tokens += tokens;
        skills_out.push(SkillEntry {
            name: skill.skill_name.clone(),
            path: skill.skill_path.clone(),
            content: truncated,
        });
    }

    if skills_out.is_empty() {
        return None;
    }

    // Serialize to a simple text representation
    let mut text = String::from("Invoked skills:\n");
    for skill in &skills_out {
        text.push_str(&format!(
            "\n## {}\nPath: {}\n{}\n",
            skill.name, skill.path, skill.content
        ));
    }

    Some(Message::User(UserMessage {
        base: MessageBase::with_origin(MessageOrigin::Compact),
        text,
        attachments: Vec::new(),
        provider_content_blocks: Vec::new(),
        summarize_metadata: None,
    }))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// A skill entry for serialization in the attachment.
#[derive(Debug, Clone)]
struct SkillEntry {
    name: String,
    path: String,
    content: String,
}

/// Truncate content to roughly `max_tokens`, keeping the head.
/// Uses the same ~4 chars/token heuristic as `rough_token_count`.
fn truncate_to_tokens(content: &str, max_tokens: u64) -> String {
    if rough_token_count(content) <= max_tokens {
        return content.to_string();
    }
    let char_budget = (max_tokens as usize)
        .saturating_mul(4)
        .saturating_sub(SKILL_TRUNCATION_MARKER.len());
    let mut result: String = content.chars().take(char_budget).collect();
    result.push_str(SKILL_TRUNCATION_MARKER);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_state_cache_insert_and_sort() {
        let mut cache = FileStateCache::new();
        cache.insert("a.txt".into(), "content a".into(), 100);
        cache.insert("b.txt".into(), "content b".into(), 200);
        cache.insert("c.txt".into(), "content c".into(), 150);

        let sorted = cache.to_vec();
        assert_eq!(sorted[0].filename, "b.txt");
        assert_eq!(sorted[1].filename, "c.txt");
        assert_eq!(sorted[2].filename, "a.txt");
    }

    #[test]
    fn file_state_cache_remove() {
        let mut cache = FileStateCache::new();
        cache.insert("a.txt".into(), "content".into(), 100);
        assert!(cache.remove("a.txt"));
        assert!(!cache.remove("a.txt"));
        assert!(cache.is_empty());
    }

    #[test]
    fn truncate_to_tokens_short_content() {
        let content = "short";
        let result = truncate_to_tokens(content, 100);
        assert_eq!(result, content);
    }

    #[test]
    fn truncate_to_tokens_long_content() {
        let content = "x".repeat(1000);
        let result = truncate_to_tokens(&content, 10);
        assert!(result.contains("[... skill content truncated"));
        assert!(result.len() < content.len());
    }

    #[test]
    fn create_plan_attachment_returns_none_when_no_content() {
        assert!(create_plan_attachment_if_needed(None, Some("/plan.md")).is_none());
    }

    #[test]
    fn create_skill_attachment_returns_none_when_empty() {
        let registry = InvokedSkillRegistry::new();
        assert!(create_skill_attachment_if_needed(&registry).is_none());
    }
}
