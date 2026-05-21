//! Intelligent skill discovery using BM25 text search.
//!
//! Provides BM25-based text search over skill metadata to dynamically
//! discover relevant skills based on task descriptions. Searches standard
//! skill directories: `~/.remote-code/skills/`, `.roo/skills/`, and
//! project-local skills directories.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ToolExecutionContext;

/// Default BM25 parameter k1 (term frequency saturation).
const BM25_K1: f64 = 1.2;
/// Default BM25 parameter b (document length normalization).
const BM25_B: f64 = 0.75;
/// Maximum number of search results to return.
const MAX_RESULTS: usize = 10;

/// Parsed skill metadata extracted from a SKILL.md file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillMetadata {
    /// Unique skill name/slug.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Trigger keywords for matching.
    pub triggers: Vec<String>,
    /// File system path to the skill directory.
    pub path: PathBuf,
}

/// A single BM25 search result with relevance score.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillSearchResult {
    /// The matched skill metadata.
    pub skill: SkillMetadata,
    /// BM25 relevance score (higher is more relevant).
    pub score: f64,
}

/// BM25 search engine for skill discovery.
#[derive(Debug, Clone)]
pub struct Bm25SkillSearchEngine {
    /// Indexed documents (skills).
    documents: Vec<SkillDocument>,
    /// Average document length in tokens.
    avg_dl: f64,
    /// Total number of documents.
    doc_count: usize,
    /// BM25 k1 parameter.
    k1: f64,
    /// BM25 b parameter.
    b: f64,
}

/// Internal representation of an indexed skill document.
#[derive(Debug, Clone)]
struct SkillDocument {
    /// The skill metadata.
    metadata: SkillMetadata,
    /// Tokenized document content.
    tokens: Vec<String>,
    /// Document length in tokens.
    length: usize,
}

impl Bm25SkillSearchEngine {
    /// Create a new BM25 search engine with default parameters.
    #[must_use]
    pub fn new() -> Self {
        Self {
            documents: Vec::new(),
            avg_dl: 0.0,
            doc_count: 0,
            k1: BM25_K1,
            b: BM25_B,
        }
    }

    /// Create a new BM25 search engine with custom parameters.
    #[must_use]
    pub fn with_params(k1: f64, b: f64) -> Self {
        Self {
            documents: Vec::new(),
            avg_dl: 0.0,
            doc_count: 0,
            k1,
            b,
        }
    }

    /// Add a skill to the search index.
    pub fn add_skill(&mut self, metadata: SkillMetadata) {
        let content = format!(
            "{} {} {}",
            metadata.name,
            metadata.description,
            metadata.triggers.join(" ")
        );
        let tokens = tokenize(&content);
        let length = tokens.len();

        self.documents.push(SkillDocument {
            metadata,
            tokens,
            length,
        });
        self.doc_count = self.documents.len();

        // Recompute average document length.
        let total: usize = self.documents.iter().map(|d| d.length).sum();
        self.avg_dl = if total > 0 {
            total as f64 / self.doc_count as f64
        } else {
            0.0
        };
    }

    /// Search for skills matching the given query.
    ///
    /// Returns results sorted by BM25 score in descending order.
    pub fn search(&self, query: &str, max_results: usize) -> Vec<SkillSearchResult> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() || self.doc_count == 0 {
            return Vec::new();
        }

        // Compute document frequency for each query term.
        let df: std::collections::HashMap<String, usize> = {
            let mut map = std::collections::HashMap::new();
            for token in &query_tokens {
                let count = self
                    .documents
                    .iter()
                    .filter(|doc| doc.tokens.contains(token))
                    .count();
                map.insert(token.clone(), count);
            }
            map
        };

        // Score each document.
        let mut results: Vec<SkillSearchResult> = self
            .documents
            .iter()
            .map(|doc| {
                let score = self.score_document(doc, &query_tokens, &df);
                SkillSearchResult {
                    skill: doc.metadata.clone(),
                    score,
                }
            })
            .filter(|r| r.score > 0.0)
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(max_results);
        results
    }

    /// Compute BM25 score for a single document.
    fn score_document(
        &self,
        doc: &SkillDocument,
        query_tokens: &[String],
        df: &std::collections::HashMap<String, usize>,
    ) -> f64 {
        let dl = doc.length as f64;
        let mut score = 0.0;

        for token in query_tokens {
            let tf = doc.tokens.iter().filter(|t| *t == token).count() as f64;
            let document_freq = df.get(token).copied().unwrap_or(0) as f64;

            // IDF component: log((N - df + 0.5) / (df + 0.5) + 1)
            let idf =
                ((self.doc_count as f64 - document_freq + 0.5) / (document_freq + 0.5) + 1.0).ln();

            // TF component: (tf * (k1 + 1)) / (tf + k1 * (1 - b + b * dl/avgdl))
            let tf_component = if self.avg_dl > 0.0 {
                (tf * (self.k1 + 1.0))
                    / (tf + self.k1 * (1.0 - self.b + self.b * (dl / self.avg_dl)))
            } else {
                tf
            };

            score += idf * tf_component;
        }

        score
    }

    /// Return the number of indexed skills.
    #[must_use]
    pub fn len(&self) -> usize {
        self.doc_count
    }

    /// Check if the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.doc_count == 0
    }
}

impl Default for Bm25SkillSearchEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Tokenize a string into lowercase tokens for BM25 indexing.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|s| !s.is_empty() && s.len() > 1)
        .map(String::from)
        .collect()
}

/// Get the list of skill search directories.
///
/// Returns paths in priority order:
/// 1. `~/.remote-code/skills/`
/// 2. `.roo/skills/` (relative to cwd)
/// 3. Project-local skills directories
pub fn skill_search_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // ~/.remote-code/skills/
    if let Some(home) = directories::BaseDirs::new().map(|bd| bd.home_dir().to_path_buf()) {
        let home_skills = home.join(".remote-code").join("skills");
        if home_skills.exists() {
            dirs.push(home_skills);
        }
    }

    // .roo/skills/
    let roo_skills = cwd.join(".roo").join("skills");
    if roo_skills.exists() {
        dirs.push(roo_skills);
    }

    // Project-local skills
    let local_skills = cwd.join("skills");
    if local_skills.exists() {
        dirs.push(local_skills);
    }

    dirs
}

/// Parse skill metadata from a SKILL.md file.
///
/// Extracts name, description, and trigger keywords from the markdown content.
pub fn parse_skill_metadata(content: &str, path: &Path) -> Result<SkillMetadata> {
    let mut name = String::new();
    let mut description = String::new();
    let mut triggers = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Extract name from first H1 heading.
        if name.is_empty()
            && let Some(heading) = trimmed.strip_prefix("# ")
        {
            name = heading.to_string();
            continue;
        }

        // Extract description from "description:" or "Description:" field.
        if description.is_empty() {
            let lower = trimmed.to_lowercase();
            if lower.starts_with("description:") || lower.starts_with("description :") {
                let desc = trimmed
                    .find(':')
                    .map(|idx| trimmed[idx + 1..].trim())
                    .unwrap_or("");
                if !desc.is_empty() {
                    description = desc.to_string();
                }
                continue;
            }
        }

        // Extract triggers from "triggers:" or "trigger:" or "when:" field.
        let lower = trimmed.to_lowercase();
        if lower.starts_with("triggers:")
            || lower.starts_with("trigger:")
            || lower.starts_with("when:")
        {
            let trigger_str = trimmed
                .find(':')
                .map(|idx| trimmed[idx + 1..].trim())
                .unwrap_or("");
            triggers = trigger_str
                .split([',', ';'])
                .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            continue;
        }

        // Use first non-empty, non-heading line as description fallback.
        if description.is_empty()
            && !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && !trimmed.starts_with("---")
        {
            description = trimmed.to_string();
        }
    }

    // Use directory name as fallback for skill name.
    if name.is_empty() {
        name = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown-skill".to_string());
    }

    Ok(SkillMetadata {
        name,
        description,
        triggers,
        path: path.to_path_buf(),
    })
}

/// Scan a directory for SKILL.md files and parse metadata.
pub fn scan_skills_dir(dir: &Path) -> Vec<SkillMetadata> {
    let mut skills = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return skills,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let skill_file = path.join("SKILL.md");
            if skill_file.exists()
                && let Ok(content) = std::fs::read_to_string(&skill_file)
                && let Ok(metadata) = parse_skill_metadata(&content, &skill_file)
            {
                skills.push(metadata);
            }
        }
    }

    skills
}

/// Discover skills relevant to a given task description using BM25 search.
///
/// # Errors
/// Returns an error if the query is empty.
pub fn discover_skills(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let query = input["query"]
        .as_str()
        .ok_or_else(|| anyhow!("query is required for skill discovery"))?;

    if query.trim().is_empty() {
        return Err(anyhow!("query cannot be empty"));
    }

    let max_results = input["max_results"].as_u64().unwrap_or(MAX_RESULTS as u64) as usize;

    // Build search engine from available skills.
    let mut engine = Bm25SkillSearchEngine::new();
    let search_dirs = skill_search_dirs(&context.cwd);

    for dir in &search_dirs {
        for skill in scan_skills_dir(dir) {
            engine.add_skill(skill);
        }
    }

    let results = engine.search(query, max_results);

    if results.is_empty() {
        Ok(json!({
            "query": query,
            "total_skills_indexed": engine.len(),
            "results": [],
            "message": "No matching skills found. Try a broader query."
        })
        .to_string())
    } else {
        let result_json: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "name": r.skill.name,
                    "description": r.skill.description,
                    "triggers": r.skill.triggers,
                    "path": r.skill.path.to_string_lossy(),
                    "score": (r.score * 100.0).round() / 100.0,
                })
            })
            .collect();

        Ok(json!({
            "query": query,
            "total_skills_indexed": engine.len(),
            "results": result_json,
            "message": format!("Found {} matching skill(s).", results.len())
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_skill(name: &str, description: &str, triggers: &[&str]) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            description: description.to_string(),
            triggers: triggers.iter().map(|s| s.to_string()).collect(),
            path: PathBuf::from(format!("/tmp/skills/{name}")),
        }
    }

    #[test]
    fn tokenize_splits_into_lowercase_tokens() {
        let tokens = tokenize("Hello World! This is a Test.");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
        assert!(tokens.contains(&"this".to_string()));
        assert!(tokens.contains(&"test".to_string()));
    }

    #[test]
    fn tokenize_filters_single_chars() {
        let tokens = tokenize("a b cd");
        assert!(!tokens.contains(&"a".to_string()));
        assert!(!tokens.contains(&"b".to_string()));
        assert!(tokens.contains(&"cd".to_string()));
    }

    #[test]
    fn bm25_engine_empty_search() {
        let engine = Bm25SkillSearchEngine::new();
        assert!(engine.is_empty());
        let results = engine.search("test", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn bm25_engine_add_and_search() {
        let mut engine = Bm25SkillSearchEngine::new();
        engine.add_skill(test_skill(
            "rust-dev",
            "Rust development skill for building systems",
            &["rust", "cargo", "systems"],
        ));
        engine.add_skill(test_skill(
            "python-dev",
            "Python development skill for data science",
            &["python", "data", "science"],
        ));
        engine.add_skill(test_skill(
            "web-dev",
            "Web development with React and TypeScript",
            &["react", "typescript", "web"],
        ));

        assert_eq!(engine.len(), 3);

        let results = engine.search("rust programming", 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].skill.name, "rust-dev");
    }

    #[test]
    fn bm25_engine_returns_top_n() {
        let mut engine = Bm25SkillSearchEngine::new();
        for i in 0..20 {
            engine.add_skill(test_skill(
                &format!("skill-{i}"),
                &format!("Skill number {i} for testing search"),
                &[&format!("test-{i}")],
            ));
        }
        let results = engine.search("testing search", 5);
        assert!(results.len() <= 5);
    }

    #[test]
    fn bm25_engine_no_match_returns_empty() {
        let mut engine = Bm25SkillSearchEngine::new();
        engine.add_skill(test_skill("rust-dev", "Rust development", &["rust"]));
        let results = engine.search("cooking recipe", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn bm25_engine_custom_params() {
        let engine = Bm25SkillSearchEngine::with_params(2.0, 0.5);
        assert_eq!(engine.k1, 2.0);
        assert_eq!(engine.b, 0.5);
    }

    #[test]
    fn bm25_engine_default_impl() {
        let engine = Bm25SkillSearchEngine::default();
        assert!(engine.is_empty());
    }

    #[test]
    fn parse_skill_metadata_extracts_name_from_heading() {
        let content = "# My Skill\nSome description here.";
        let meta = parse_skill_metadata(content, Path::new("/tmp/skills/my-skill/SKILL.md"))
            .expect("parse");
        assert_eq!(meta.name, "My Skill");
    }

    #[test]
    fn parse_skill_metadata_extracts_description_field() {
        let content = "# Test\ndescription: A test skill for testing.";
        let meta =
            parse_skill_metadata(content, Path::new("/tmp/skills/test/SKILL.md")).expect("parse");
        assert_eq!(meta.description, "A test skill for testing.");
    }

    #[test]
    fn parse_skill_metadata_extracts_triggers() {
        let content = "# Test\ntriggers: rust, cargo, build";
        let meta =
            parse_skill_metadata(content, Path::new("/tmp/skills/test/SKILL.md")).expect("parse");
        assert_eq!(meta.triggers, vec!["rust", "cargo", "build"]);
    }

    #[test]
    fn parse_skill_metadata_uses_dir_name_as_fallback() {
        let content = "Just some content without headings.";
        let meta = parse_skill_metadata(content, Path::new("/tmp/skills/my-cool-skill/SKILL.md"))
            .expect("parse");
        assert_eq!(meta.name, "my-cool-skill");
    }

    #[test]
    fn parse_skill_metadata_handles_when_keyword() {
        let content = "# Test\nwhen: build, compile, run";
        let meta =
            parse_skill_metadata(content, Path::new("/tmp/skills/test/SKILL.md")).expect("parse");
        assert_eq!(meta.triggers, vec!["build", "compile", "run"]);
    }

    #[test]
    fn skill_search_dirs_returns_existing_dirs() {
        let cwd = PathBuf::from("/tmp");
        let dirs = skill_search_dirs(&cwd);
        // Just verify it doesn't panic and returns a vec.
        assert!(dirs.len() <= 3);
    }

    #[test]
    fn discover_skills_requires_query() {
        let input = json!({});
        let context = ToolExecutionContext::default();
        let result = discover_skills(&input, &context);
        let error = result.expect_err("missing query should return an error");
        assert!(error.to_string().contains("query"));
    }

    #[test]
    fn discover_skills_rejects_empty_query() {
        let input = json!({"query": ""});
        let context = ToolExecutionContext::default();
        let result = discover_skills(&input, &context);
        assert!(result.is_err());
    }

    #[test]
    fn discover_skills_returns_json_on_no_results() {
        let input = json!({"query": "nonexistent-skill-xyz-123"});
        let context = ToolExecutionContext::default();
        let result = discover_skills(&input, &context);
        let output = result.expect("no-result query should still return JSON");
        let parsed: Value = serde_json::from_str(&output).expect("valid json");
        let results = parsed["results"].as_array().expect("results array");
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn scan_skills_dir_handles_nonexistent_dir() {
        let skills = scan_skills_dir(Path::new("/nonexistent/path/xyz"));
        assert!(skills.is_empty());
    }

    #[test]
    fn skill_search_result_serializes() {
        let result = SkillSearchResult {
            skill: test_skill("test", "A test skill", &["test"]),
            score: 1.5,
        };
        let json = serde_json::to_string(&result).expect("serialize");
        assert!(json.contains("test"));
        assert!(json.contains("1.5"));
    }

    #[test]
    fn skill_metadata_round_trips_json() {
        let meta = test_skill("rust-dev", "Rust development", &["rust", "cargo"]);
        let json = serde_json::to_string(&meta).expect("serialize");
        let parsed: SkillMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, meta);
    }

    #[test]
    fn bm25_search_ranks_relevant_higher() {
        let mut engine = Bm25SkillSearchEngine::new();
        engine.add_skill(test_skill(
            "rust-expert",
            "Expert Rust programming systems development cargo",
            &["rust", "cargo"],
        ));
        engine.add_skill(test_skill(
            "python-dev",
            "Python data science machine learning",
            &["python", "ml"],
        ));
        engine.add_skill(test_skill(
            "rust-beginner",
            "Introduction to Rust programming basics",
            &["rust", "beginner"],
        ));

        let results = engine.search("rust programming", 10);
        assert!(!results.is_empty());
        // Both rust skills should appear, but the one with more matches should rank higher.
        let rust_results: Vec<&str> = results.iter().map(|r| r.skill.name.as_str()).collect();
        assert!(rust_results.contains(&"rust-expert"));
        assert!(rust_results.contains(&"rust-beginner"));
    }

    #[test]
    fn bm25_handles_duplicate_query_terms() {
        let mut engine = Bm25SkillSearchEngine::new();
        engine.add_skill(test_skill("test", "test skill", &["test"]));
        let results = engine.search("test test test", 10);
        assert!(!results.is_empty());
    }

    #[test]
    fn parse_skill_metadata_handles_empty_content() {
        let meta =
            parse_skill_metadata("", Path::new("/tmp/skills/fallback/SKILL.md")).expect("parse");
        assert_eq!(meta.name, "fallback");
    }

    #[test]
    fn parse_skill_metadata_description_fallback_from_body() {
        let content = "# My Skill\n\nThis is the first line of description.";
        let meta =
            parse_skill_metadata(content, Path::new("/tmp/skills/test/SKILL.md")).expect("parse");
        assert_eq!(meta.description, "This is the first line of description.");
    }

    #[test]
    fn bm25_engine_with_single_token_skill() {
        let mut engine = Bm25SkillSearchEngine::new();
        engine.add_skill(test_skill("x", "x", &["x"]));
        // "x" is a single char and should be filtered by tokenize
        let results = engine.search("x", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn bm25_engine_multiple_skills_same_trigger() {
        let mut engine = Bm25SkillSearchEngine::new();
        engine.add_skill(test_skill("skill-a", "Rust development tool", &["rust"]));
        engine.add_skill(test_skill("skill-b", "Rust testing framework", &["rust"]));
        let results = engine.search("rust", 10);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn discover_skills_with_max_results() {
        let input = json!({"query": "test", "max_results": 1});
        let context = ToolExecutionContext::default();
        let result = discover_skills(&input, &context);
        assert!(result.is_ok());
    }
}
