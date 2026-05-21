//! Skill Search Service — BM25-based local skill search and indexing.
//!
//! Provides full-text search over discovered skills using a simplified BM25
//! ranking algorithm. Supports indexing skill metadata, prefetching skill
//! content, and returning ranked search results with match scores.
//!
//! # Architecture
//!
//! - [`SkillSearchIndex`] — inverted index over skill documents
//! - [`SkillSearchResult`] — a ranked search hit
//! - [`SkillSearchEngine`] — orchestrates indexing and querying
//! - [`SkillPrefetch`] — background prefetch logic for skill content

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tokenization
// ---------------------------------------------------------------------------

/// Splits text into lowercase tokens for indexing.
/// Handles camelCase, snake_case, and kebab-case splitting.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch.is_whitespace() || ch == '_' || ch == '-' {
            if !current.is_empty() {
                tokens.push(current.to_lowercase());
                current.clear();
            }
        } else if ch.is_uppercase() && !current.is_empty() {
            // camelCase boundary
            tokens.push(current.to_lowercase());
            current.clear();
            current.extend(ch.to_lowercase());
        } else {
            current.extend(ch.to_lowercase());
        }
    }

    if !current.is_empty() {
        tokens.push(current.to_lowercase());
    }

    tokens
}

/// Computes term frequency: count of `term` occurrences in `tokens`.
#[allow(dead_code)]
fn term_frequency(tokens: &[String], term: &str) -> f64 {
    let count = tokens.iter().filter(|t| *t == term).count();
    f64::from(u32::try_from(count).unwrap_or(0))
}

// ---------------------------------------------------------------------------
// Search Document
// ---------------------------------------------------------------------------

/// A document in the search index representing a single skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDocument {
    /// Skill slug (unique identifier).
    pub slug: String,
    /// Skill title.
    pub title: String,
    /// Short description / summary.
    pub summary: String,
    /// Tags for categorization.
    pub tags: Vec<String>,
    /// Trigger phrases.
    pub triggers: Vec<String>,
    /// Tools used by this skill.
    pub tools: Vec<String>,
    /// Full instruction text (indexed but not returned in results).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instructions: String,
}

impl SkillDocument {
    /// Creates a new skill document.
    #[must_use]
    pub fn new(slug: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            slug: slug.into(),
            title: title.into(),
            summary: String::new(),
            tags: Vec::new(),
            triggers: Vec::new(),
            tools: Vec::new(),
            instructions: String::new(),
        }
    }

    /// Returns all searchable text concatenated.
    #[must_use]
    pub fn searchable_text(&self) -> String {
        let mut parts = Vec::new();
        parts.push(self.title.clone());
        if !self.summary.is_empty() {
            parts.push(self.summary.clone());
        }
        parts.extend(self.tags.clone());
        parts.extend(self.triggers.clone());
        parts.extend(self.tools.clone());
        parts.push(self.slug.replace('-', " "));
        parts.join(" ")
    }

    /// Tokenizes the searchable text.
    #[must_use]
    pub fn tokens(&self) -> Vec<String> {
        tokenize(&self.searchable_text())
    }
}

// ---------------------------------------------------------------------------
// Search Result
// ---------------------------------------------------------------------------

/// A ranked search result for a skill query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSearchResult {
    /// The matched skill's slug.
    pub slug: String,
    /// The matched skill's title.
    pub title: String,
    /// The matched skill's summary.
    pub summary: String,
    /// BM25 relevance score (higher is better).
    pub score: f64,
    /// Matched terms that contributed to the score.
    pub matched_terms: Vec<String>,
    /// Tags of the matched skill.
    pub tags: Vec<String>,
}

impl SkillSearchResult {
    /// Creates a new search result.
    #[must_use]
    pub fn new(slug: impl Into<String>, title: impl Into<String>, score: f64) -> Self {
        Self {
            slug: slug.into(),
            title: title.into(),
            summary: String::new(),
            score,
            matched_terms: Vec::new(),
            tags: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Inverted Index
// ---------------------------------------------------------------------------

/// Posting entry for a term in a document.
#[derive(Debug, Clone)]
struct Posting {
    /// Document index in the index.
    doc_idx: usize,
    /// Term frequency in this document.
    tf: f64,
}

/// BM25 parameters.
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

/// A local inverted index over skill documents with BM25 scoring.
pub struct SkillSearchIndex {
    /// Indexed documents.
    documents: Vec<SkillDocument>,
    /// Inverted index: term → list of postings.
    postings: HashMap<String, Vec<Posting>>,
    /// Document lengths (token count).
    doc_lengths: Vec<f64>,
    /// Average document length.
    avg_dl: f64,
    /// Total number of documents.
    doc_count: usize,
}

impl Default for SkillSearchIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillSearchIndex {
    /// Creates a new empty index.
    #[must_use]
    pub fn new() -> Self {
        Self {
            documents: Vec::new(),
            postings: HashMap::new(),
            doc_lengths: Vec::new(),
            avg_dl: 0.0,
            doc_count: 0,
        }
    }

    /// Returns the number of indexed documents.
    #[must_use]
    pub fn len(&self) -> usize {
        self.doc_count
    }

    /// Returns `true` if the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.doc_count == 0
    }

    /// Adds a skill document to the index.
    pub fn add_document(&mut self, doc: SkillDocument) {
        let doc_idx = self.documents.len();
        let tokens = doc.tokens();
        let dl = f64::from(u32::try_from(tokens.len()).unwrap_or(0));

        // Build term frequency map for this document
        let mut term_freqs: HashMap<String, f64> = HashMap::new();
        for token in &tokens {
            *term_freqs.entry(token.clone()).or_insert(0.0) += 1.0;
        }

        // Add postings
        for (term, tf) in &term_freqs {
            self.postings
                .entry(term.clone())
                .or_default()
                .push(Posting { doc_idx, tf: *tf });
        }

        self.documents.push(doc);
        self.doc_lengths.push(dl);
        self.doc_count += 1;

        // Recompute average document length
        let total: f64 = self.doc_lengths.iter().sum();
        self.avg_dl = if self.doc_count > 0 {
            total / f64::from(u32::try_from(self.doc_count).unwrap_or(1))
        } else {
            0.0
        };
    }

    /// Removes a document by slug. Returns `true` if found and removed.
    pub fn remove_document(&mut self, slug: &str) -> bool {
        let doc_idx = self.documents.iter().position(|d| d.slug == slug);

        let idx = match doc_idx {
            Some(i) => i,
            None => return false,
        };

        // Remove from postings
        for postings in self.postings.values_mut() {
            postings.retain(|p| p.doc_idx != idx);
        }

        // Remove document and rebuild indices
        self.documents.remove(idx);
        self.rebuild();

        true
    }

    /// Rebuilds the index from scratch using existing documents.
    fn rebuild(&mut self) {
        self.postings.clear();
        self.doc_lengths.clear();

        let docs: Vec<SkillDocument> = self.documents.drain(..).collect();
        self.doc_count = 0;

        for doc in docs {
            self.add_document(doc);
        }
    }

    /// Searches the index using BM25 scoring.
    /// Returns results sorted by score (descending).
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<SkillSearchResult> {
        if self.is_empty() || query.trim().is_empty() {
            return Vec::new();
        }

        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        let n = f64::from(u32::try_from(self.doc_count).unwrap_or(1));

        // Compute BM25 scores per document
        let mut scores: HashMap<usize, f64> = HashMap::new();
        let mut matched_terms: HashMap<usize, Vec<String>> = HashMap::new();

        for term in &query_tokens {
            let postings = match self.postings.get(term) {
                Some(p) => p,
                None => continue,
            };

            // IDF = ln((N - df + 0.5) / (df + 0.5) + 1)
            let df = f64::from(u32::try_from(postings.len()).unwrap_or(0));
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();

            for posting in postings {
                let dl = self
                    .doc_lengths
                    .get(posting.doc_idx)
                    .copied()
                    .unwrap_or(0.0);

                // TF component: (tf * (k1 + 1)) / (tf + k1 * (1 - b + b * dl/avgdl))
                let tf_norm = if self.avg_dl > 0.0 {
                    (posting.tf * (BM25_K1 + 1.0))
                        / (posting.tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / self.avg_dl))
                } else {
                    posting.tf
                };

                let score = idf * tf_norm;
                *scores.entry(posting.doc_idx).or_insert(0.0) += score;
                matched_terms
                    .entry(posting.doc_idx)
                    .or_default()
                    .push(term.clone());
            }
        }

        // Sort by score descending
        let mut ranked: Vec<(usize, f64)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Build results
        ranked
            .into_iter()
            .take(limit)
            .map(|(doc_idx, score)| {
                let doc = &self.documents[doc_idx];
                let terms = matched_terms.remove(&doc_idx).unwrap_or_default();
                SkillSearchResult {
                    slug: doc.slug.clone(),
                    title: doc.title.clone(),
                    summary: doc.summary.clone(),
                    score,
                    matched_terms: terms,
                    tags: doc.tags.clone(),
                }
            })
            .collect()
    }

    /// Returns all indexed documents.
    #[must_use]
    pub fn documents(&self) -> &[SkillDocument] {
        &self.documents
    }

    /// Returns the vocabulary size (number of unique terms).
    #[must_use]
    pub fn vocabulary_size(&self) -> usize {
        self.postings.len()
    }
}

// ---------------------------------------------------------------------------
// Skill Prefetch
// ---------------------------------------------------------------------------

/// Prefetch state for skill content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PrefetchState {
    /// Not yet fetched.
    #[default]
    Pending,
    /// Fetch in progress.
    InProgress,
    /// Successfully fetched.
    Completed,
    /// Fetch failed.
    Failed,
}

/// Tracks prefetch status for skill documents.
#[derive(Debug, Clone, Default)]
pub struct SkillPrefetch {
    /// Map of slug → prefetch state.
    states: BTreeMap<String, PrefetchState>,
}

impl SkillPrefetch {
    /// Creates a new empty prefetch tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a skill for prefetching.
    pub fn register(&mut self, slug: impl Into<String>) {
        self.states
            .entry(slug.into())
            .or_insert(PrefetchState::Pending);
    }

    /// Marks a skill as in-progress.
    pub fn mark_in_progress(&mut self, slug: &str) {
        if let Some(state) = self.states.get_mut(slug) {
            *state = PrefetchState::InProgress;
        }
    }

    /// Marks a skill as completed.
    pub fn mark_completed(&mut self, slug: &str) {
        if let Some(state) = self.states.get_mut(slug) {
            *state = PrefetchState::Completed;
        }
    }

    /// Marks a skill as failed.
    pub fn mark_failed(&mut self, slug: &str) {
        if let Some(state) = self.states.get_mut(slug) {
            *state = PrefetchState::Failed;
        }
    }

    /// Returns the prefetch state for a skill.
    #[must_use]
    pub fn state(&self, slug: &str) -> PrefetchState {
        self.states
            .get(slug)
            .copied()
            .unwrap_or(PrefetchState::Pending)
    }

    /// Returns the number of pending prefetches.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.states
            .values()
            .filter(|s| **s == PrefetchState::Pending)
            .count()
    }

    /// Returns the number of completed prefetches.
    #[must_use]
    pub fn completed_count(&self) -> usize {
        self.states
            .values()
            .filter(|s| **s == PrefetchState::Completed)
            .count()
    }

    /// Returns slugs that are pending prefetch.
    #[must_use]
    pub fn pending_slugs(&self) -> Vec<&str> {
        self.states
            .iter()
            .filter(|(_, s)| **s == PrefetchState::Pending)
            .map(|(slug, _)| slug.as_str())
            .collect()
    }

    /// Returns the total number of registered skills.
    #[must_use]
    pub fn total(&self) -> usize {
        self.states.len()
    }
}

// ---------------------------------------------------------------------------
// Search Engine
// ---------------------------------------------------------------------------

/// High-level search engine that combines indexing and querying.
pub struct SkillSearchEngine {
    /// The underlying search index.
    index: SkillSearchIndex,
    /// Prefetch tracker.
    prefetch: SkillPrefetch,
    /// Whether the search feature is enabled.
    enabled: bool,
}

impl Default for SkillSearchEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillSearchEngine {
    /// Creates a new search engine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            index: SkillSearchIndex::new(),
            prefetch: SkillPrefetch::new(),
            enabled: true,
        }
    }

    /// Creates a disabled search engine.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::new()
        }
    }

    /// Returns whether the engine is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Indexes a skill document.
    pub fn index_skill(&mut self, doc: SkillDocument) {
        if !self.enabled {
            return;
        }
        self.prefetch.register(&doc.slug);
        self.index.add_document(doc);
    }

    /// Removes a skill from the index.
    pub fn remove_skill(&mut self, slug: &str) -> bool {
        self.index.remove_document(slug)
    }

    /// Searches for skills matching the query.
    /// Returns up to `limit` results sorted by relevance.
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<SkillSearchResult> {
        if !self.enabled {
            return Vec::new();
        }
        self.index.search(query, limit)
    }

    /// Returns the number of indexed skills.
    #[must_use]
    pub fn indexed_count(&self) -> usize {
        self.index.len()
    }

    /// Returns a reference to the prefetch tracker.
    #[must_use]
    pub fn prefetch(&self) -> &SkillPrefetch {
        &self.prefetch
    }

    /// Returns a mutable reference to the prefetch tracker.
    pub fn prefetch_mut(&mut self) -> &mut SkillPrefetch {
        &mut self.prefetch
    }

    /// Returns the vocabulary size.
    #[must_use]
    pub fn vocabulary_size(&self) -> usize {
        self.index.vocabulary_size()
    }
}

// ---------------------------------------------------------------------------
// Convenience function
// ---------------------------------------------------------------------------

/// Searches skills using a simple index. This is a convenience function
/// for quick searches without managing an engine instance.
#[must_use]
pub fn search_skills(
    documents: &[SkillDocument],
    query: &str,
    limit: usize,
) -> Vec<SkillSearchResult> {
    let mut index = SkillSearchIndex::new();
    for doc in documents {
        index.add_document(doc.clone());
    }
    index.search(query, limit)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_doc(slug: &str, title: &str, tags: &[&str]) -> SkillDocument {
        SkillDocument {
            slug: slug.to_string(),
            title: title.to_string(),
            summary: String::new(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            triggers: Vec::new(),
            tools: Vec::new(),
            instructions: String::new(),
        }
    }

    // --- Tokenizer tests ---

    #[test]
    fn test_tokenize_simple() {
        let tokens = tokenize("hello world");
        assert_eq!(tokens, vec!["hello", "world"]);
    }

    #[test]
    fn test_tokenize_snake_case() {
        let tokens = tokenize("my_awesome_skill");
        assert!(tokens.contains(&"my".to_string()));
        assert!(tokens.contains(&"awesome".to_string()));
        assert!(tokens.contains(&"skill".to_string()));
    }

    #[test]
    fn test_tokenize_kebab_case() {
        let tokens = tokenize("my-awesome-skill");
        assert!(tokens.contains(&"my".to_string()));
        assert!(tokens.contains(&"awesome".to_string()));
        assert!(tokens.contains(&"skill".to_string()));
    }

    #[test]
    fn test_tokenize_camel_case() {
        let tokens = tokenize("myAwesomeSkill");
        assert!(tokens.contains(&"my".to_string()));
        assert!(tokens.contains(&"awesome".to_string()));
        assert!(tokens.contains(&"skill".to_string()));
    }

    #[test]
    fn test_tokenize_empty() {
        let tokens = tokenize("");
        assert!(tokens.is_empty());
    }

    // --- SkillDocument tests ---

    #[test]
    fn test_skill_document_new() {
        let doc = SkillDocument::new("test-skill", "Test Skill");
        assert_eq!(doc.slug, "test-skill");
        assert_eq!(doc.title, "Test Skill");
    }

    #[test]
    fn test_skill_document_searchable_text() {
        let doc = SkillDocument {
            slug: "rust-dev".to_string(),
            title: "Rust Development".to_string(),
            summary: "Build Rust applications".to_string(),
            tags: vec!["rust".to_string(), "development".to_string()],
            triggers: vec!["build rust".to_string()],
            tools: vec!["cargo".to_string()],
            instructions: String::new(),
        };
        let text = doc.searchable_text();
        assert!(text.contains("Rust Development"));
        assert!(text.contains("Build Rust applications"));
        assert!(text.contains("rust"));
    }

    #[test]
    fn test_skill_document_tokens() {
        let doc = make_test_doc("rust-dev", "Rust Development", &["rust"]);
        let tokens = doc.tokens();
        assert!(tokens.contains(&"rust".to_string()));
        assert!(tokens.contains(&"development".to_string()));
    }

    // --- SkillSearchIndex tests ---

    #[test]
    fn test_index_new() {
        let index = SkillSearchIndex::new();
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn test_index_add_document() {
        let mut index = SkillSearchIndex::new();
        index.add_document(make_test_doc("skill-1", "Rust Dev", &["rust"]));
        assert_eq!(index.len(), 1);
        assert!(!index.is_empty());
    }

    #[test]
    fn test_index_add_multiple_documents() {
        let mut index = SkillSearchIndex::new();
        index.add_document(make_test_doc("skill-1", "Rust Dev", &["rust"]));
        index.add_document(make_test_doc("skill-2", "Python Dev", &["python"]));
        index.add_document(make_test_doc("skill-3", "JavaScript Dev", &["js"]));
        assert_eq!(index.len(), 3);
    }

    #[test]
    fn test_index_search_basic() {
        let mut index = SkillSearchIndex::new();
        index.add_document(make_test_doc("rust-dev", "Rust Development", &["rust"]));
        index.add_document(make_test_doc(
            "python-dev",
            "Python Development",
            &["python"],
        ));

        let results = index.search("rust", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].slug, "rust-dev");
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn test_index_search_ranking() {
        let mut index = SkillSearchIndex::new();
        let mut doc1 = make_test_doc("rust-1", "Rust Core", &["rust"]);
        doc1.summary = "Rust rust rust programming".to_string(); // High TF
        let mut doc2 = make_test_doc("rust-2", "General Dev", &["rust"]);
        doc2.summary = "Some development".to_string(); // Low TF

        index.add_document(doc1);
        index.add_document(doc2);

        let results = index.search("rust", 10);
        assert_eq!(results.len(), 2);
        // doc1 should rank higher due to more occurrences
        assert_eq!(results[0].slug, "rust-1");
    }

    #[test]
    fn test_index_search_empty_query() {
        let mut index = SkillSearchIndex::new();
        index.add_document(make_test_doc("skill-1", "Test", &[]));
        let results = index.search("", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_index_search_no_match() {
        let mut index = SkillSearchIndex::new();
        index.add_document(make_test_doc("rust-dev", "Rust Dev", &["rust"]));
        let results = index.search("python", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_index_search_limit() {
        let mut index = SkillSearchIndex::new();
        for i in 0..10 {
            index.add_document(make_test_doc(
                &format!("skill-{i}"),
                &format!("Rust Dev {i}"),
                &["rust"],
            ));
        }
        let results = index.search("rust", 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_index_remove_document() {
        let mut index = SkillSearchIndex::new();
        index.add_document(make_test_doc("skill-1", "Rust Dev", &["rust"]));
        index.add_document(make_test_doc("skill-2", "Python Dev", &["python"]));
        assert!(index.remove_document("skill-1"));
        assert_eq!(index.len(), 1);
        let results = index.search("rust", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_index_remove_nonexistent() {
        let mut index = SkillSearchIndex::new();
        index.add_document(make_test_doc("skill-1", "Test", &[]));
        assert!(!index.remove_document("nonexistent"));
    }

    #[test]
    fn test_index_vocabulary_size() {
        let mut index = SkillSearchIndex::new();
        index.add_document(make_test_doc("skill-1", "Rust Dev", &["rust"]));
        assert!(index.vocabulary_size() > 0);
    }

    // --- SkillPrefetch tests ---

    #[test]
    fn test_prefetch_new() {
        let pf = SkillPrefetch::new();
        assert_eq!(pf.total(), 0);
        assert_eq!(pf.pending_count(), 0);
    }

    #[test]
    fn test_prefetch_register() {
        let mut pf = SkillPrefetch::new();
        pf.register("skill-1");
        assert_eq!(pf.total(), 1);
        assert_eq!(pf.pending_count(), 1);
    }

    #[test]
    fn test_prefetch_lifecycle() {
        let mut pf = SkillPrefetch::new();
        pf.register("skill-1");
        assert_eq!(pf.state("skill-1"), PrefetchState::Pending);

        pf.mark_in_progress("skill-1");
        assert_eq!(pf.state("skill-1"), PrefetchState::InProgress);

        pf.mark_completed("skill-1");
        assert_eq!(pf.state("skill-1"), PrefetchState::Completed);
        assert_eq!(pf.completed_count(), 1);
        assert_eq!(pf.pending_count(), 0);
    }

    #[test]
    fn test_prefetch_failed() {
        let mut pf = SkillPrefetch::new();
        pf.register("skill-1");
        pf.mark_failed("skill-1");
        assert_eq!(pf.state("skill-1"), PrefetchState::Failed);
    }

    #[test]
    fn test_prefetch_pending_slugs() {
        let mut pf = SkillPrefetch::new();
        pf.register("skill-1");
        pf.register("skill-2");
        pf.mark_completed("skill-1");
        let pending = pf.pending_slugs();
        assert_eq!(pending.len(), 1);
        assert!(pending.contains(&"skill-2"));
    }

    // --- SkillSearchEngine tests ---

    #[test]
    fn test_engine_new() {
        let engine = SkillSearchEngine::new();
        assert!(engine.is_enabled());
        assert_eq!(engine.indexed_count(), 0);
    }

    #[test]
    fn test_engine_disabled() {
        let engine = SkillSearchEngine::disabled();
        assert!(!engine.is_enabled());
    }

    #[test]
    fn test_engine_index_and_search() {
        let mut engine = SkillSearchEngine::new();
        engine.index_skill(make_test_doc("rust-dev", "Rust Development", &["rust"]));
        engine.index_skill(make_test_doc(
            "python-dev",
            "Python Development",
            &["python"],
        ));

        let results = engine.search("rust", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].slug, "rust-dev");
    }

    #[test]
    fn test_engine_disabled_search() {
        let mut engine = SkillSearchEngine::disabled();
        engine.index_skill(make_test_doc("rust-dev", "Rust Dev", &["rust"]));
        let results = engine.search("rust", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_engine_remove_skill() {
        let mut engine = SkillSearchEngine::new();
        engine.index_skill(make_test_doc("skill-1", "Test", &[]));
        assert!(engine.remove_skill("skill-1"));
        assert_eq!(engine.indexed_count(), 0);
    }

    // --- search_skills convenience function ---

    #[test]
    fn test_search_skills_convenience() {
        let docs = vec![
            make_test_doc("rust-dev", "Rust Development", &["rust"]),
            make_test_doc("python-dev", "Python Development", &["python"]),
        ];
        let results = search_skills(&docs, "rust", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].slug, "rust-dev");
    }
}
