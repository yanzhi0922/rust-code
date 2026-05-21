//! BM25-based tool search engine.
//!
//! Provides full-text search over tool names, descriptions, and tags using the
//! BM25 ranking function. This allows the `tool_search` tool to return
//! relevance-ranked results instead of simple substring matching.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single search result returned by [`ToolSearchEngine::search`].
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Tool name (e.g. `"read_file"`).
    pub name: String,
    /// BM25 relevance score (higher = more relevant).
    pub score: f64,
    /// Tool description.
    pub description: String,
}

// ---------------------------------------------------------------------------
// Internal document representation
// ---------------------------------------------------------------------------

/// Internal document used by the BM25 index.
struct SearchDocument {
    /// Tool name.
    name: String,
    /// Tokenised terms (kept for potential future use / debugging).
    #[allow(dead_code)]
    tokens: Vec<String>,
    /// Term frequency map: term → count.
    tf: HashMap<String, u64>,
    /// Document length (number of tokens).
    length: usize,
    /// Original description (returned in results).
    description: String,
}

// ---------------------------------------------------------------------------
// BM25 search engine
// ---------------------------------------------------------------------------

/// BM25 tool search engine.
///
/// # Example
///
/// ```
/// use claude_tools::search::ToolSearchEngine;
///
/// let mut engine = ToolSearchEngine::new();
/// engine.add_tool("read_file", "Read a UTF-8 text file from the workspace.", &["file", "read"]);
/// engine.add_tool("write_file", "Create or overwrite a text file.", &["file", "write"]);
///
/// let results = engine.search("read text file", 5);
/// assert!(!results.is_empty());
/// assert_eq!(results[0].name, "read_file");
/// ```
pub struct ToolSearchEngine {
    /// Indexed documents keyed by tool name.
    documents: HashMap<String, SearchDocument>,
    /// Average document length across the corpus.
    avg_dl: f64,
    /// Total number of documents.
    doc_count: u64,
    /// BM25 k1 parameter (term frequency saturation).
    k1: f64,
    /// BM25 b parameter (length normalisation).
    b: f64,
}

impl ToolSearchEngine {
    /// Create a new empty search engine with default BM25 parameters.
    #[must_use]
    pub fn new() -> Self {
        Self {
            documents: HashMap::new(),
            avg_dl: 0.0,
            doc_count: 0,
            k1: 1.2,
            b: 0.75,
        }
    }

    /// Add (or replace) a tool in the search index.
    pub fn add_tool(&mut self, name: &str, description: &str, tags: &[&str]) {
        // Build the document text: name + description + tags.
        let combined = format!("{} {} {}", name, description, tags.join(" "));
        let tokens = Self::tokenize(&combined);

        // Compute term frequencies.
        let mut tf: HashMap<String, u64> = HashMap::new();
        for token in &tokens {
            *tf.entry(token.clone()).or_insert(0) += 1;
        }

        let length = tokens.len();

        // Remove old entry if it exists so we can recompute averages.
        if let Some(old) = self.documents.remove(name) {
            // Subtract the old document from running totals.
            // We'll recompute avg_dl below.
            let _old = old; // just drop it
        } else {
            self.doc_count += 1;
        }

        self.documents.insert(
            name.to_owned(),
            SearchDocument {
                name: name.to_owned(),
                tokens,
                tf,
                length,
                description: description.to_owned(),
            },
        );

        // Recompute average document length.
        let total_len: usize = self.documents.values().map(|d| d.length).sum();
        let count = self.documents.len() as f64;
        self.avg_dl = if count > 0.0 {
            total_len as f64 / count
        } else {
            0.0
        };
    }

    /// Search for tools matching `query`, returning at most `max_results`
    /// results sorted by descending BM25 score.
    pub fn search(&self, query: &str, max_results: usize) -> Vec<SearchResult> {
        let query_tokens = Self::tokenize(query);
        if query_tokens.is_empty() || self.documents.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<SearchResult> = self
            .documents
            .values()
            .filter_map(|doc| {
                let score = self.bm25_score(doc, &query_tokens);
                if score > 0.0 {
                    Some(SearchResult {
                        name: doc.name.clone(),
                        score,
                        description: doc.description.clone(),
                    })
                } else {
                    None
                }
            })
            .collect();

        // Sort by descending score.
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(max_results);
        scored
    }

    /// Tokenise text: split on whitespace and punctuation, convert to lowercase.
    fn tokenize(text: &str) -> Vec<String> {
        text.split(|c: char| c.is_whitespace() || !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase())
            .collect()
    }

    /// Compute IDF (inverse document frequency) for a term.
    ///
    /// Uses the standard BM25 IDF formula:
    /// `idf(t) = ln((N - n(t) + 0.5) / (n(t) + 0.5) + 1)`
    ///
    /// where `N` = total documents, `n(t)` = documents containing term `t`.
    fn idf(&self, term: &str) -> f64 {
        let n_t = self
            .documents
            .values()
            .filter(|doc| doc.tf.contains_key(term))
            .count() as f64;
        let n = self.doc_count as f64;
        ((n - n_t + 0.5) / (n_t + 0.5) + 1.0).ln()
    }

    /// Compute the BM25 score for a single document given query tokens.
    fn bm25_score(&self, doc: &SearchDocument, query_tokens: &[String]) -> f64 {
        let dl = doc.length as f64;
        let avg_dl = if self.avg_dl > 0.0 { self.avg_dl } else { 1.0 };

        let mut score = 0.0;
        for term in query_tokens {
            let tf = doc.tf.get(term).copied().unwrap_or(0) as f64;
            if tf == 0.0 {
                continue;
            }
            let idf = self.idf(term);
            let numerator = tf * (self.k1 + 1.0);
            let denominator = tf + self.k1 * (1.0 - self.b + self.b * (dl / avg_dl));
            score += idf * (numerator / denominator);
        }
        score
    }
}

impl Default for ToolSearchEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::ToolSearchEngine;

    #[test]
    fn empty_engine_returns_no_results() {
        let engine = ToolSearchEngine::new();
        let results = engine.search("anything", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn exact_name_match_ranks_highest() {
        let mut engine = ToolSearchEngine::new();
        engine.add_tool(
            "read_file",
            "Read a UTF-8 text file from the workspace.",
            &["file", "read"],
        );
        engine.add_tool(
            "write_file",
            "Create or overwrite a text file.",
            &["file", "write"],
        );
        engine.add_tool(
            "bash_command",
            "Run a shell command in the workspace.",
            &["shell", "command"],
        );

        let results = engine.search("read file", 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "read_file");
    }

    #[test]
    fn search_by_description() {
        let mut engine = ToolSearchEngine::new();
        engine.add_tool(
            "glob",
            "Search for files using glob patterns.",
            &["files", "pattern"],
        );
        engine.add_tool(
            "grep",
            "Search files for a regex pattern with context lines.",
            &["regex", "search"],
        );

        let results = engine.search("regex pattern search", 5);
        assert!(!results.is_empty());
        // grep should rank higher for "regex" query
        assert_eq!(results[0].name, "grep");
    }

    #[test]
    fn search_by_tag() {
        let mut engine = ToolSearchEngine::new();
        engine.add_tool("tool_a", "Does something.", &["alpha", "beta"]);
        engine.add_tool("tool_b", "Does another thing.", &["gamma", "delta"]);

        let results = engine.search("alpha", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "tool_a");
    }

    #[test]
    fn max_results_limits_output() {
        let mut engine = ToolSearchEngine::new();
        for i in 0..20 {
            engine.add_tool(
                &format!("tool_{i}"),
                "A tool that does file operations.",
                &["file"],
            );
        }

        let results = engine.search("file", 5);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn no_match_returns_empty() {
        let mut engine = ToolSearchEngine::new();
        engine.add_tool("read_file", "Read a file.", &["file"]);

        let results = engine.search("xyzzy_nonsense", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn default_trait_works() {
        let engine = ToolSearchEngine::default();
        assert!(engine.search("test", 1).is_empty());
    }
}
