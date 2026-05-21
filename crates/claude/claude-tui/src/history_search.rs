//! History search engine for the TUI.
//!
//! Provides fuzzy and substring search over command history,
//! inspired by Claude Code's history search hook. Supports:
//! - Substring matching (case-insensitive)
//! - Fuzzy matching with scoring
//! - Navigation through search results
//! - Session-scoped history tracking

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// History Entry
// ---------------------------------------------------------------------------

/// A single history entry.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    /// The command/text entered.
    pub text: String,
    /// Unix timestamp when the entry was created.
    pub timestamp: u64,
    /// Session ID this entry belongs to.
    pub session_id: String,
}

impl HistoryEntry {
    /// Create a new history entry with the current timestamp.
    pub fn new(text: impl Into<String>, session_id: impl Into<String>) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        HistoryEntry {
            text: text.into(),
            timestamp,
            session_id: session_id.into(),
        }
    }

    /// Create a history entry with a specific timestamp.
    pub fn with_timestamp(
        text: impl Into<String>,
        timestamp: u64,
        session_id: impl Into<String>,
    ) -> Self {
        HistoryEntry {
            text: text.into(),
            timestamp,
            session_id: session_id.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Search Match
// ---------------------------------------------------------------------------

/// A search match result.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchMatch {
    /// Index of the matched entry in the history.
    pub index: usize,
    /// The matched entry.
    pub entry: HistoryEntry,
    /// Score of the match (higher = better).
    pub score: f64,
    /// Positions of matched characters.
    pub match_positions: Vec<usize>,
}

// ---------------------------------------------------------------------------
// Search Mode
// ---------------------------------------------------------------------------

/// Search mode for history search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Case-insensitive substring search.
    Substring,
    /// Fuzzy matching with scoring.
    Fuzzy,
}

impl fmt::Display for SearchMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Substring => write!(f, "substring"),
            Self::Fuzzy => write!(f, "fuzzy"),
        }
    }
}

// ---------------------------------------------------------------------------
// History Search Engine
// ---------------------------------------------------------------------------

/// Engine for searching through command history.
#[derive(Debug, Clone)]
pub struct HistorySearchEngine {
    /// All history entries, newest first.
    entries: Vec<HistoryEntry>,
    /// Maximum number of entries to keep.
    max_entries: usize,
}

impl HistorySearchEngine {
    /// Create a new search engine with a maximum capacity.
    pub fn new(max_entries: usize) -> Self {
        HistorySearchEngine {
            entries: Vec::new(),
            max_entries: max_entries.max(1),
        }
    }

    /// Create with default capacity (1000 entries).
    pub fn with_default_capacity() -> Self {
        Self::new(1000)
    }

    /// Add an entry to the history.
    pub fn add(&mut self, entry: HistoryEntry) {
        // Deduplicate: remove existing entry with same text.
        self.entries.retain(|e| e.text != entry.text);
        // Insert at the front (newest first).
        self.entries.insert(0, entry);
        // Trim to max capacity.
        if self.entries.len() > self.max_entries {
            self.entries.truncate(self.max_entries);
        }
    }

    /// Get all entries.
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Get the number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the engine is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Search for entries matching the query.
    pub fn search(&self, query: &str, mode: SearchMode) -> Vec<SearchMatch> {
        if query.is_empty() {
            return Vec::new();
        }

        let mut matches: Vec<SearchMatch> = Vec::new();

        for (index, entry) in self.entries.iter().enumerate() {
            match mode {
                SearchMode::Substring => {
                    if let Some(positions) = substring_match(&entry.text, query) {
                        let score = compute_substring_score(entry, &positions);
                        matches.push(SearchMatch {
                            index,
                            entry: entry.clone(),
                            score,
                            match_positions: positions,
                        });
                    }
                }
                SearchMode::Fuzzy => {
                    if let Some((positions, score)) = fuzzy_match(&entry.text, query) {
                        matches.push(SearchMatch {
                            index,
                            entry: entry.clone(),
                            score,
                            match_positions: positions,
                        });
                    }
                }
            }
        }

        // Sort by score descending.
        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches
    }

    /// Search with substring mode (convenience).
    pub fn search_substring(&self, query: &str) -> Vec<SearchMatch> {
        self.search(query, SearchMode::Substring)
    }

    /// Search with fuzzy mode (convenience).
    pub fn search_fuzzy(&self, query: &str) -> Vec<SearchMatch> {
        self.search(query, SearchMode::Fuzzy)
    }

    /// Clear all history.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Get entries for a specific session.
    pub fn entries_for_session(&self, session_id: &str) -> Vec<&HistoryEntry> {
        self.entries
            .iter()
            .filter(|e| e.session_id == session_id)
            .collect()
    }
}

impl Default for HistorySearchEngine {
    fn default() -> Self {
        Self::with_default_capacity()
    }
}

// ---------------------------------------------------------------------------
// History Navigator
// ---------------------------------------------------------------------------

/// Navigator for stepping through search results.
#[derive(Debug, Clone)]
pub struct HistoryNavigator {
    /// The search engine.
    engine: HistorySearchEngine,
    /// Current search results.
    results: Vec<SearchMatch>,
    /// Current position in results.
    position: usize,
    /// The current search query.
    query: String,
    /// Search mode.
    mode: SearchMode,
}

impl HistoryNavigator {
    /// Create a new navigator with the given engine.
    pub fn new(engine: HistorySearchEngine) -> Self {
        HistoryNavigator {
            engine,
            results: Vec::new(),
            position: 0,
            query: String::new(),
            mode: SearchMode::Substring,
        }
    }

    /// Set the search mode.
    pub fn set_mode(&mut self, mode: SearchMode) {
        self.mode = mode;
    }

    /// Get the current search mode.
    pub fn mode(&self) -> SearchMode {
        self.mode
    }

    /// Start a search with the given query.
    pub fn search(&mut self, query: &str) -> usize {
        self.query = query.to_string();
        self.results = self.engine.search(query, self.mode);
        self.position = 0;
        self.results.len()
    }

    /// Get the current match.
    pub fn current(&self) -> Option<&SearchMatch> {
        self.results.get(self.position)
    }

    /// Move to the next match. Returns `true` if moved.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> bool {
        if self.results.is_empty() {
            return false;
        }
        self.position = (self.position + 1) % self.results.len();
        true
    }

    /// Move to the previous match. Returns `true` if moved.
    pub fn previous(&mut self) -> bool {
        if self.results.is_empty() {
            return false;
        }
        if self.position == 0 {
            self.position = self.results.len() - 1;
        } else {
            self.position -= 1;
        }
        true
    }

    /// Get the total number of results.
    pub fn result_count(&self) -> usize {
        self.results.len()
    }

    /// Get the current position (0-based).
    pub fn position(&self) -> usize {
        self.position
    }

    /// Get the current query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Reset the search state.
    pub fn reset(&mut self) {
        self.results.clear();
        self.position = 0;
        self.query.clear();
    }

    /// Get the underlying engine (read-only).
    pub fn engine(&self) -> &HistorySearchEngine {
        &self.engine
    }

    /// Get the underlying engine (mutable).
    pub fn engine_mut(&mut self) -> &mut HistorySearchEngine {
        &mut self.engine
    }
}

// ---------------------------------------------------------------------------
// Matching helpers
// ---------------------------------------------------------------------------

/// Case-insensitive substring match. Returns positions of matched chars.
fn substring_match(text: &str, query: &str) -> Option<Vec<usize>> {
    let text_lower = text.to_lowercase();
    let query_lower = query.to_lowercase();

    let start = text_lower.find(&query_lower)?;
    let positions: Vec<usize> = (start..start + query_lower.len()).collect();
    Some(positions)
}

/// Compute a score for a substring match.
fn compute_substring_score(entry: &HistoryEntry, positions: &[usize]) -> f64 {
    let mut score: f64 = 100.0;

    // Prefer matches at the start.
    if let Some(&first) = positions.first() {
        if first == 0 {
            score += 50.0;
        } else {
            score -= first as f64 * 0.5;
        }
    }

    // Prefer recent entries.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let age_secs = now.saturating_sub(entry.timestamp);
    let age_hours = age_secs as f64 / 3600.0;
    score -= age_hours * 0.1;

    // Prefer shorter texts (more specific).
    score -= entry.text.len() as f64 * 0.01;

    score
}

/// Fuzzy match: check if all query chars appear in order in text.
fn fuzzy_match(text: &str, query: &str) -> Option<(Vec<usize>, f64)> {
    let text_lower = text.to_lowercase();
    let query_lower = query.to_lowercase();
    let query_chars: Vec<char> = query_lower.chars().collect();
    let text_chars: Vec<char> = text_lower.chars().collect();

    if query_chars.is_empty() {
        return None;
    }

    let mut positions: Vec<usize> = Vec::new();
    let mut text_idx: usize = 0;

    for &q_char in &query_chars {
        let mut found = false;
        while text_idx < text_chars.len() {
            if text_chars[text_idx] == q_char {
                positions.push(text_idx);
                text_idx += 1;
                found = true;
                break;
            }
            text_idx += 1;
        }
        if !found {
            return None;
        }
    }

    let score = compute_fuzzy_score(text, &positions, query_chars.len());
    Some((positions, score))
}

/// Compute a fuzzy match score.
fn compute_fuzzy_score(text: &str, positions: &[usize], query_len: usize) -> f64 {
    let mut score: f64 = 50.0;

    // Reward contiguous matches.
    let mut contiguous: usize = 0;
    for window in positions.windows(2) {
        if window[1] == window[0] + 1 {
            contiguous += 1;
        }
    }
    score += contiguous as f64 * 10.0;

    // Reward matches at word boundaries.
    let text_chars: Vec<char> = text.chars().collect();
    for &pos in positions {
        if pos == 0 {
            score += 15.0;
        } else if let Some(prev) = text_chars.get(pos - 1)
            && (*prev == ' ' || *prev == '_' || *prev == '-' || *prev == '/')
        {
            score += 10.0;
        }
    }

    // Penalize spread-out matches.
    if positions.len() >= 2 {
        let span = positions[positions.len() - 1] - positions[0];
        score -= span as f64 * 0.3;
    }

    // Reward coverage ratio.
    let coverage = query_len as f64 / text.len().max(1) as f64;
    score += coverage * 30.0;

    score
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(text: &str) -> HistoryEntry {
        HistoryEntry::with_timestamp(text, 1000, "test-session")
    }

    fn make_entry_with_session(text: &str, session: &str) -> HistoryEntry {
        HistoryEntry::with_timestamp(text, 1000, session)
    }

    #[test]
    fn test_history_entry_new() {
        let entry = HistoryEntry::new("hello world", "session-1");
        assert_eq!(entry.text, "hello world");
        assert_eq!(entry.session_id, "session-1");
        assert!(entry.timestamp > 0);
    }

    #[test]
    fn test_history_entry_with_timestamp() {
        let entry = HistoryEntry::with_timestamp("test", 42, "s1");
        assert_eq!(entry.timestamp, 42);
    }

    #[test]
    fn test_engine_add() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("hello"));
        engine.add(make_entry("world"));
        assert_eq!(engine.len(), 2);
    }

    #[test]
    fn test_engine_dedup() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("hello"));
        engine.add(make_entry("hello"));
        assert_eq!(engine.len(), 1);
    }

    #[test]
    fn test_engine_max_capacity() {
        let mut engine = HistorySearchEngine::new(3);
        engine.add(make_entry("a"));
        engine.add(make_entry("b"));
        engine.add(make_entry("c"));
        engine.add(make_entry("d"));
        assert_eq!(engine.len(), 3);
        // "d" should be first (newest).
        assert_eq!(engine.entries()[0].text, "d");
    }

    #[test]
    fn test_engine_clear() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("hello"));
        engine.clear();
        assert!(engine.is_empty());
    }

    #[test]
    fn test_search_substring_basic() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("cargo build"));
        engine.add(make_entry("cargo test"));
        engine.add(make_entry("git commit"));

        let results = engine.search_substring("cargo");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_substring_case_insensitive() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("Cargo Build"));

        let results = engine.search_substring("cargo");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_substring_empty_query() {
        let engine = HistorySearchEngine::new(100);
        let results = engine.search_substring("");
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_substring_no_match() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("hello world"));

        let results = engine.search_substring("xyz");
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_fuzzy_basic() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("cargo build --release"));

        let results = engine.search_fuzzy("cb");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_fuzzy_no_match() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("abc"));

        let results = engine.search_fuzzy("xyz");
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_sorted_by_score() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("cargo build"));
        engine.add(make_entry("cargo"));

        let results = engine.search_substring("cargo");
        assert_eq!(results.len(), 2);
        // "cargo" (shorter) should score higher.
        assert!(results[0].score >= results[1].score);
    }

    #[test]
    fn test_entries_for_session() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry_with_session("a", "s1"));
        engine.add(make_entry_with_session("b", "s2"));
        engine.add(make_entry_with_session("c", "s1"));

        let s1 = engine.entries_for_session("s1");
        assert_eq!(s1.len(), 2);
        let s2 = engine.entries_for_session("s2");
        assert_eq!(s2.len(), 1);
    }

    #[test]
    fn test_navigator_search() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("cargo build"));
        engine.add(make_entry("cargo test"));
        engine.add(make_entry("git commit"));

        let mut nav = HistoryNavigator::new(engine);
        let count = nav.search("cargo");
        assert_eq!(count, 2);
        assert_eq!(nav.result_count(), 2);
    }

    #[test]
    fn test_navigator_next_previous() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("cargo build"));
        engine.add(make_entry("cargo test"));

        let mut nav = HistoryNavigator::new(engine);
        nav.search("cargo");

        assert!(nav.next());
        assert_eq!(nav.position(), 1);
        assert!(nav.previous());
        assert_eq!(nav.position(), 0);
    }

    #[test]
    fn test_navigator_wrap_around() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("cargo build"));
        engine.add(make_entry("cargo test"));

        let mut nav = HistoryNavigator::new(engine);
        nav.search("cargo");
        assert!(nav.next());
        assert_eq!(nav.position(), 1);
        assert!(nav.next()); // wraps to 0.
        assert_eq!(nav.position(), 0);
    }

    #[test]
    fn test_navigator_current() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("cargo build"));

        let mut nav = HistoryNavigator::new(engine);
        nav.search("cargo");
        let current = nav.current().expect("should have current");
        assert_eq!(current.entry.text, "cargo build");
    }

    #[test]
    fn test_navigator_reset() {
        let mut engine = HistorySearchEngine::new(100);
        engine.add(make_entry("cargo build"));

        let mut nav = HistoryNavigator::new(engine);
        nav.search("cargo");
        nav.reset();
        assert_eq!(nav.result_count(), 0);
        assert!(nav.query().is_empty());
    }

    #[test]
    fn test_navigator_empty_results() {
        let engine = HistorySearchEngine::new(100);
        let mut nav = HistoryNavigator::new(engine);
        nav.search("nonexistent");
        assert!(!nav.next());
        assert!(!nav.previous());
        assert!(nav.current().is_none());
    }

    #[test]
    fn test_search_mode_display() {
        assert_eq!(SearchMode::Substring.to_string(), "substring");
        assert_eq!(SearchMode::Fuzzy.to_string(), "fuzzy");
    }

    #[test]
    fn test_navigator_set_mode() {
        let engine = HistorySearchEngine::new(100);
        let mut nav = HistoryNavigator::new(engine);
        assert_eq!(nav.mode(), SearchMode::Substring);
        nav.set_mode(SearchMode::Fuzzy);
        assert_eq!(nav.mode(), SearchMode::Fuzzy);
    }
}
