//! Post-compaction cleanup and result tracking.
//!
//! Provides utilities for cleaning up conversation state after a compaction
//! operation, including result tracking, warning state management,
//! post-compact file re-reads, and a cascade cache-reset registry matching
//! TS's `runPostCompactCleanup()`.

use claude_core::Message;

use crate::attachment::FileStateCache;

// ---------------------------------------------------------------------------
// PostCompactResult
// ---------------------------------------------------------------------------

/// Summary of a post-compaction cleanup pass.
#[derive(Debug, Clone)]
pub struct PostCompactResult {
    /// Number of messages removed during cleanup.
    pub removed_count: usize,
    /// Number of messages kept after cleanup.
    pub kept_count: usize,
    /// Approximate tokens saved by the cleanup.
    pub tokens_saved: u64,
}

impl PostCompactResult {
    /// Create a new post-compact result.
    #[must_use]
    pub fn new(removed_count: usize, kept_count: usize, tokens_saved: u64) -> Self {
        Self {
            removed_count,
            kept_count,
            tokens_saved,
        }
    }

    /// Whether any messages were removed.
    #[must_use]
    pub fn has_changes(&self) -> bool {
        self.removed_count > 0
    }

    /// Total messages before cleanup.
    #[must_use]
    pub fn total_before(&self) -> usize {
        self.removed_count + self.kept_count
    }

    /// Reduction ratio (0.0 to 1.0).
    #[must_use]
    pub fn reduction_ratio(&self) -> f64 {
        let total = self.total_before();
        if total == 0 {
            0.0
        } else {
            self.removed_count as f64 / total as f64
        }
    }
}

impl Default for PostCompactResult {
    fn default() -> Self {
        Self::new(0, 0, 0)
    }
}

// ---------------------------------------------------------------------------
// run_post_compact_cleanup
// ---------------------------------------------------------------------------

/// Criteria for messages to remove during post-compact cleanup.
#[derive(Debug, Clone)]
pub struct CleanupCriteria {
    /// Remove tombstone messages.
    pub remove_tombstones: bool,
    /// Remove messages older than this many messages from the end (0 = keep all).
    pub remove_older_than: usize,
    /// Remove system messages that are not the most recent.
    pub deduplicate_system_messages: bool,
}

impl Default for CleanupCriteria {
    fn default() -> Self {
        Self {
            remove_tombstones: true,
            remove_older_than: 0,
            deduplicate_system_messages: false,
        }
    }
}

/// Execute a post-compaction cleanup pass.
///
/// Removes messages matching the given criteria and returns a summary of
/// what was removed.
pub fn run_post_compact_cleanup(
    messages: &[Message],
    criteria: &CleanupCriteria,
) -> (Vec<Message>, PostCompactResult) {
    let mut kept = Vec::new();
    let mut removed = 0;
    let mut tokens_saved: u64 = 0;

    let cutoff_from_end =
        if criteria.remove_older_than > 0 && messages.len() > criteria.remove_older_than {
            messages.len() - criteria.remove_older_than
        } else {
            0
        };

    // Track the last system message index for deduplication
    let last_system_idx = if criteria.deduplicate_system_messages {
        messages
            .iter()
            .enumerate()
            .rev()
            .find(|(_, m)| matches!(m, Message::System(_)))
            .map(|(i, _)| i)
    } else {
        None
    };

    for (i, msg) in messages.iter().enumerate() {
        let mut should_remove = false;

        // Remove tombstones
        if criteria.remove_tombstones && matches!(msg, Message::Tombstone(_)) {
            should_remove = true;
        }

        // Remove old messages
        if cutoff_from_end > 0 && i < cutoff_from_end {
            should_remove = true;
        }

        // Deduplicate system messages (keep only the last one)
        if let Some(last_idx) = last_system_idx
            && matches!(msg, Message::System(_))
            && i != last_idx
        {
            should_remove = true;
        }

        if should_remove {
            removed += 1;
            // Rough token estimate
            let text = format!("{msg:?}");
            tokens_saved += (text.len() as u64).div_ceil(4);
        } else {
            kept.push(msg.clone());
        }
    }

    let result = PostCompactResult::new(removed, kept.len(), tokens_saved);
    (kept, result)
}

// ---------------------------------------------------------------------------
// Post-compact file re-reads
// ---------------------------------------------------------------------------

/// Maximum number of recently accessed files to re-read after compaction.
const POST_COMPACT_RE_READ_LIMIT: usize = 5;

/// Total token budget for post-compact file re-reads.
const POST_COMPACT_TOKEN_BUDGET: usize = 50_000;

/// Maximum estimated tokens per individual file re-read.
const POST_COMPACT_MAX_TOKENS_PER_FILE: usize = 5_000;

/// Re-read up to `limit` recently accessed files from the file state cache
/// and return them as user messages containing the file content.
///
/// After compaction, the model loses context about the contents of files it
/// was working with. Re-reading the most recently accessed files helps the
/// model maintain continuity. The files are sorted by most recent access
/// timestamp, and only the top `limit` are included.
///
/// # Arguments
///
/// * `file_state` - The file state cache tracking recently read files.
/// * `limit` - Maximum number of files to re-read (defaults to 5).
/// * `preserved_file_paths` - File paths that are already in the preserved tail;
///   these are skipped to avoid duplicating context.
///
/// # Returns
///
/// A vector of `Message::User` messages, each containing the file path and
/// its content as a text block suitable for LLM consumption.
pub fn re_read_recent_files(
    file_state: &FileStateCache,
    limit: usize,
    preserved_file_paths: &std::collections::HashSet<String>,
) -> Vec<Message> {
    let limit = if limit == 0 {
        POST_COMPACT_RE_READ_LIMIT
    } else {
        limit
    };

    let recent_files = file_state.most_recent(limit);
    let mut messages = Vec::new();
    let mut total_tokens_used: usize = 0;

    for file in recent_files {
        // Deduplication: skip files already in the preserved tail
        if preserved_file_paths.contains(&file.filename) {
            continue;
        }

        // Token budget: estimate tokens as content.len() / 4
        let estimated_tokens = file.content.len() / 4;

        // Skip individual files exceeding per-file budget
        if estimated_tokens > POST_COMPACT_MAX_TOKENS_PER_FILE {
            continue;
        }

        // Stop when total budget is exhausted
        if total_tokens_used + estimated_tokens > POST_COMPACT_TOKEN_BUDGET {
            break;
        }

        total_tokens_used += estimated_tokens;

        use claude_core::{MessageBase, MessageOrigin, UserMessage};

        let text = format!(
            "<file_content path=\"{}\">\n{}\n</file_content>",
            file.filename, file.content
        );

        let message = Message::User(UserMessage {
            base: {
                let mut base = MessageBase::with_origin(MessageOrigin::Compact);
                base.is_meta = true;
                base
            },
            text,
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        });

        messages.push(message);
    }

    messages
}

// ---------------------------------------------------------------------------
// compact_warning_state
// ---------------------------------------------------------------------------

/// Manages the warning state for compaction operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactWarningManager {
    /// Current state.
    state: CompactWarningState,
    /// Number of compactions since last warning.
    compactions_since_warning: u32,
}

/// State of the compaction warning lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactWarningState {
    /// No warning has been issued.
    None,
    /// A warning has been issued but not acknowledged.
    Warned,
    /// The user has acknowledged the warning.
    Acknowledged,
}

impl CompactWarningManager {
    /// Create a new warning manager in the initial state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: CompactWarningState::None,
            compactions_since_warning: 0,
        }
    }

    /// Get the current warning state.
    #[must_use]
    pub fn state(&self) -> CompactWarningState {
        self.state
    }

    /// Issue a warning, transitioning to the Warned state.
    pub fn warn(&mut self) {
        self.state = CompactWarningState::Warned;
    }

    /// Acknowledge the warning, transitioning to the Acknowledged state.
    pub fn acknowledge(&mut self) {
        self.state = CompactWarningState::Acknowledged;
        self.compactions_since_warning = 0;
    }

    /// Record that a compaction occurred.
    pub fn record_compaction(&mut self) {
        self.compactions_since_warning += 1;
    }

    /// Whether a warning should be shown before the next compaction.
    #[must_use]
    pub fn should_warn(&self) -> bool {
        matches!(self.state, CompactWarningState::None)
            || (matches!(self.state, CompactWarningState::Acknowledged)
                && self.compactions_since_warning >= 3)
    }

    /// Reset the warning state.
    pub fn reset(&mut self) {
        self.state = CompactWarningState::None;
        self.compactions_since_warning = 0;
    }

    /// Get compactions since last warning.
    #[must_use]
    pub fn compactions_since_warning(&self) -> u32 {
        self.compactions_since_warning
    }
}

impl Default for CompactWarningManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Post-compact cache-reset cascade (matches TS runPostCompactCleanup)
// ---------------------------------------------------------------------------

/// A named cleanup callback that is invoked after compaction.
///
/// The TS `runPostCompactCleanup()` resets 10+ caches after compaction:
/// microcompactState, contextCollapse, getUserContext cache, getMemoryFiles cache,
/// systemPromptSections, classifierApprovals, speculativeChecks, betaTracingState,
/// attributionHooks.sweepFileContentCache, sessionMessagesCache.
///
/// In Rust, these caches are spread across crates. The registry allows each
/// module to register a cleanup callback that is called in cascade after compaction.
pub type CleanupCallback = Box<dyn Fn() + Send + Sync>;

/// A named cleanup entry in the registry.
pub struct CleanupEntry {
    /// Human-readable name for debugging/logging.
    pub name: String,
    /// The callback to invoke.
    pub callback: CleanupCallback,
}

impl std::fmt::Debug for CleanupEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanupEntry")
            .field("name", &self.name)
            .field("callback", &"<callback>")
            .finish()
    }
}

/// Registry of post-compact cleanup callbacks.
///
/// Modules register their cleanup callbacks during initialization.
/// After compaction, [`PostCompactCleanupRegistry::run_cascade`] invokes all
/// registered callbacks in registration order.
pub struct PostCompactCleanupRegistry {
    entries: Vec<CleanupEntry>,
}

impl PostCompactCleanupRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Register a cleanup callback.
    pub fn register(&mut self, name: impl Into<String>, callback: CleanupCallback) {
        self.entries.push(CleanupEntry {
            name: name.into(),
            callback,
        });
    }

    /// Run all registered cleanup callbacks in order.
    ///
    /// Returns the number of callbacks invoked.
    pub fn run_cascade(&self) -> usize {
        let count = self.entries.len();
        for entry in &self.entries {
            tracing::debug!("post-compact cleanup: {}", entry.name);
            (entry.callback)();
        }
        count
    }

    /// Number of registered callbacks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether any callbacks are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all registered callbacks.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for PostCompactCleanupRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::{MessageBase, SystemMessage, SystemMessageSubtype, UserMessage};

    fn make_user_msg(text: &str) -> Message {
        Message::User(UserMessage {
            base: MessageBase::default(),
            text: text.to_string(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        })
    }

    fn make_system_msg(text: &str) -> Message {
        Message::System(SystemMessage {
            base: MessageBase::default(),
            subtype: SystemMessageSubtype::Informational,
            text: text.to_string(),
            error: None,
        })
    }

    fn make_tombstone_msg() -> Message {
        Message::Tombstone(claude_core::TombstoneMessage {
            base: MessageBase::default(),
            replaced_message_ids: Vec::new(),
            summary: "compacted".to_string(),
        })
    }

    // -- PostCompactResult ----------------------------------------------------

    #[test]
    fn post_compact_result_new() {
        let r = PostCompactResult::new(5, 10, 1000);
        assert_eq!(r.removed_count, 5);
        assert_eq!(r.kept_count, 10);
        assert_eq!(r.tokens_saved, 1000);
    }

    #[test]
    fn post_compact_result_default() {
        let r = PostCompactResult::default();
        assert_eq!(r.removed_count, 0);
        assert_eq!(r.kept_count, 0);
        assert_eq!(r.tokens_saved, 0);
    }

    #[test]
    fn post_compact_result_has_changes() {
        assert!(PostCompactResult::new(1, 0, 0).has_changes());
        assert!(!PostCompactResult::new(0, 5, 0).has_changes());
    }

    #[test]
    fn post_compact_result_total_before() {
        let r = PostCompactResult::new(3, 7, 0);
        assert_eq!(r.total_before(), 10);
    }

    #[test]
    fn post_compact_result_reduction_ratio() {
        let r = PostCompactResult::new(5, 5, 0);
        assert!((r.reduction_ratio() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn post_compact_result_reduction_ratio_zero() {
        let r = PostCompactResult::new(0, 0, 0);
        assert_eq!(r.reduction_ratio(), 0.0);
    }

    // -- run_post_compact_cleanup ---------------------------------------------

    #[test]
    fn cleanup_empty_messages() {
        let (kept, result) = run_post_compact_cleanup(&[], &CleanupCriteria::default());
        assert!(kept.is_empty());
        assert_eq!(result.removed_count, 0);
    }

    #[test]
    fn cleanup_removes_tombstones() {
        let msgs = vec![
            make_user_msg("hello"),
            make_tombstone_msg(),
            make_user_msg("world"),
        ];
        let (kept, result) = run_post_compact_cleanup(&msgs, &CleanupCriteria::default());
        assert_eq!(kept.len(), 2);
        assert_eq!(result.removed_count, 1);
        assert!(result.has_changes());
    }

    #[test]
    fn cleanup_no_tombstone_removal_when_disabled() {
        let msgs = vec![make_user_msg("hello"), make_tombstone_msg()];
        let criteria = CleanupCriteria {
            remove_tombstones: false,
            ..CleanupCriteria::default()
        };
        let (kept, result) = run_post_compact_cleanup(&msgs, &criteria);
        assert_eq!(kept.len(), 2);
        assert_eq!(result.removed_count, 0);
    }

    #[test]
    fn cleanup_removes_old_messages() {
        let msgs = vec![
            make_user_msg("old1"),
            make_user_msg("old2"),
            make_user_msg("old3"),
            make_user_msg("recent1"),
            make_user_msg("recent2"),
        ];
        let criteria = CleanupCriteria {
            remove_older_than: 2,
            ..CleanupCriteria::default()
        };
        let (kept, result) = run_post_compact_cleanup(&msgs, &criteria);
        assert_eq!(kept.len(), 2);
        assert_eq!(result.removed_count, 3);
    }

    #[test]
    fn cleanup_deduplicate_system_messages() {
        let msgs = vec![
            make_system_msg("sys1"),
            make_user_msg("hello"),
            make_system_msg("sys2"),
            make_user_msg("world"),
            make_system_msg("sys3"),
        ];
        let criteria = CleanupCriteria {
            deduplicate_system_messages: true,
            ..CleanupCriteria::default()
        };
        let (kept, result) = run_post_compact_cleanup(&msgs, &criteria);
        // Only the last system message should be kept
        let system_count = kept
            .iter()
            .filter(|m| matches!(m, Message::System(_)))
            .count();
        assert_eq!(system_count, 1);
        assert_eq!(result.removed_count, 2);
    }

    #[test]
    fn cleanup_no_changes_when_nothing_matches() {
        let msgs = vec![make_user_msg("hello"), make_user_msg("world")];
        let (kept, result) = run_post_compact_cleanup(&msgs, &CleanupCriteria::default());
        assert_eq!(kept.len(), 2);
        assert!(!result.has_changes());
    }

    // -- CompactWarningManager ------------------------------------------------

    #[test]
    fn warning_manager_new() {
        let mgr = CompactWarningManager::new();
        assert_eq!(mgr.state(), CompactWarningState::None);
        assert_eq!(mgr.compactions_since_warning(), 0);
    }

    #[test]
    fn warning_manager_default() {
        let mgr = CompactWarningManager::default();
        assert_eq!(mgr.state(), CompactWarningState::None);
    }

    #[test]
    fn warning_manager_warn() {
        let mut mgr = CompactWarningManager::new();
        mgr.warn();
        assert_eq!(mgr.state(), CompactWarningState::Warned);
    }

    #[test]
    fn warning_manager_acknowledge() {
        let mut mgr = CompactWarningManager::new();
        mgr.warn();
        mgr.acknowledge();
        assert_eq!(mgr.state(), CompactWarningState::Acknowledged);
        assert_eq!(mgr.compactions_since_warning(), 0);
    }

    #[test]
    fn warning_manager_should_warn_initial() {
        let mgr = CompactWarningManager::new();
        assert!(mgr.should_warn());
    }

    #[test]
    fn warning_manager_should_not_warn_after_acknowledge() {
        let mut mgr = CompactWarningManager::new();
        mgr.warn();
        mgr.acknowledge();
        assert!(!mgr.should_warn());
    }

    #[test]
    fn warning_manager_should_warn_after_many_compactions() {
        let mut mgr = CompactWarningManager::new();
        mgr.warn();
        mgr.acknowledge();
        for _ in 0..3 {
            mgr.record_compaction();
        }
        assert!(mgr.should_warn());
    }

    #[test]
    fn warning_manager_record_compaction() {
        let mut mgr = CompactWarningManager::new();
        mgr.record_compaction();
        mgr.record_compaction();
        assert_eq!(mgr.compactions_since_warning(), 2);
    }

    #[test]
    fn warning_manager_reset() {
        let mut mgr = CompactWarningManager::new();
        mgr.warn();
        mgr.acknowledge();
        mgr.record_compaction();
        mgr.reset();
        assert_eq!(mgr.state(), CompactWarningState::None);
        assert_eq!(mgr.compactions_since_warning(), 0);
    }

    #[test]
    fn warning_manager_lifecycle() {
        let mut mgr = CompactWarningManager::new();
        assert!(mgr.should_warn());
        mgr.warn();
        assert_eq!(mgr.state(), CompactWarningState::Warned);
        mgr.acknowledge();
        assert!(!mgr.should_warn());
        for _ in 0..2 {
            mgr.record_compaction();
        }
        assert!(!mgr.should_warn());
        mgr.record_compaction();
        assert!(mgr.should_warn());
    }

    // -- re_read_recent_files: token budget & dedup tests ---------------------

    #[test]
    fn re_read_skips_preserved_files() {
        let mut cache = FileStateCache::new();
        cache.insert("file_a.txt".into(), "content a".into(), 100);
        cache.insert("file_b.txt".into(), "content b".into(), 200);
        let preserved: std::collections::HashSet<String> =
            vec!["file_a.txt".to_string()].into_iter().collect();
        let msgs = re_read_recent_files(&cache, 5, &preserved);
        // file_b.txt has higher timestamp so is most recent
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            Message::User(u) => assert!(u.text.contains("file_b.txt")),
            _ => panic!("expected User"),
        }
    }

    #[test]
    fn re_read_skips_oversized_files() {
        let mut cache = FileStateCache::new();
        // 30,000 chars -> ~7,500 tokens, exceeds 5,000 per-file budget
        cache.insert("big.txt".into(), "x".repeat(30_000), 100);
        let preserved = std::collections::HashSet::new();
        let msgs = re_read_recent_files(&cache, 5, &preserved);
        assert!(msgs.is_empty(), "oversized file should be skipped");
    }

    #[test]
    fn re_read_stops_at_total_budget() {
        let mut cache = FileStateCache::new();
        // Each file: 10,000 chars -> ~2,500 tokens. Budget is 50,000 so max ~20 files.
        for i in 0..25 {
            cache.insert(format!("file_{i}.txt"), "a".repeat(10_000), 100 + i as i64);
        }
        let preserved = std::collections::HashSet::new();
        let msgs = re_read_recent_files(&cache, 25, &preserved);
        // Should stop before exhausting all 25 files
        assert!(msgs.len() < 25, "should stop at token budget");
        assert!(!msgs.is_empty(), "should include some files");
    }

    // -- PostCompactCleanupRegistry tests --------------------------------------

    #[test]
    fn cleanup_registry_starts_empty() {
        let registry = PostCompactCleanupRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn cleanup_registry_register_and_run() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        let mut registry = PostCompactCleanupRegistry::new();
        registry.register(
            "test_callback",
            Box::new(move || {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            }),
        );

        assert_eq!(registry.len(), 1);
        let count = registry.run_cascade();
        assert_eq!(count, 1);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cleanup_registry_multiple_callbacks() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let counter = Arc::new(AtomicU32::new(0));

        let mut registry = PostCompactCleanupRegistry::new();
        for i in 0..5 {
            let counter_clone = Arc::clone(&counter);
            registry.register(
                format!("callback_{i}"),
                Box::new(move || {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                }),
            );
        }

        assert_eq!(registry.len(), 5);
        registry.run_cascade();
        assert_eq!(counter.load(Ordering::SeqCst), 5);

        // Run again — callbacks should fire again.
        registry.run_cascade();
        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }

    #[test]
    fn cleanup_registry_clear() {
        let mut registry = PostCompactCleanupRegistry::new();
        registry.register("cb1", Box::new(|| {}));
        registry.register("cb2", Box::new(|| {}));
        assert_eq!(registry.len(), 2);
        registry.clear();
        assert!(registry.is_empty());
    }
}
