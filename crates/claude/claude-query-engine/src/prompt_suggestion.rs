//! Prompt Suggestion Engine — context-aware next-prompt generation.
//!
//! Generates suggestions for what the user might want to do next, based on
//! conversation context, recent tool usage, and configurable speculation modes.
//!
//! # Architecture
//!
//! - [`PromptSuggestionEngine`] — main engine for generating suggestions
//! - [`SuggestionCandidate`] — a candidate suggestion with scoring metadata
//! - [`SpeculationMode`] — controls how aggressively suggestions are generated
//! - [`generate_suggestions()`] — core suggestion generation function

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Speculation Mode
// ---------------------------------------------------------------------------

/// Controls how aggressively the engine generates speculative suggestions.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeculationMode {
    /// Speculation disabled — only direct suggestions.
    #[default]
    Off,
    /// Conservative — only speculate when confidence is high.
    Conservative,
    /// Aggressive — speculate broadly, accept lower confidence.
    Aggressive,
}

impl SpeculationMode {
    /// Returns the minimum confidence threshold for speculative suggestions.
    #[must_use]
    pub fn min_confidence(self) -> f64 {
        match self {
            Self::Off => 0.5,
            Self::Conservative => 0.7,
            Self::Aggressive => 0.4,
        }
    }

    /// Returns the maximum number of suggestions to generate.
    #[must_use]
    pub fn max_suggestions(self) -> usize {
        match self {
            Self::Off => 3,
            Self::Conservative => 5,
            Self::Aggressive => 8,
        }
    }

    /// Parses a speculation mode from a string.
    #[must_use]
    pub fn from_str_opt(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "off" | "disabled" => Self::Off,
            "conservative" | "cons" => Self::Conservative,
            "aggressive" | "agg" => Self::Aggressive,
            _ => Self::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Suggestion Candidate
// ---------------------------------------------------------------------------

/// Category of a suggestion.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionCategory {
    /// A follow-up action related to the current task.
    #[default]
    FollowUp,
    /// A fix for a detected error or issue.
    Fix,
    /// An exploration or investigation suggestion.
    Explore,
    /// A refactoring suggestion.
    Refactor,
    /// A testing suggestion.
    Test,
    /// A documentation suggestion.
    Document,
    /// A deployment or CI suggestion.
    Deploy,
}

/// A candidate suggestion with scoring metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionCandidate {
    /// The suggested prompt text.
    pub text: String,
    /// Confidence score (0.0–1.0).
    pub confidence: f64,
    /// Category of the suggestion.
    pub category: SuggestionCategory,
    /// Source that generated this suggestion.
    pub source: String,
    /// Whether this suggestion was generated speculatively.
    pub speculative: bool,
    /// Tags for filtering and tracking.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl SuggestionCandidate {
    /// Creates a new suggestion candidate.
    #[must_use]
    pub fn new(text: impl Into<String>, confidence: f64, category: SuggestionCategory) -> Self {
        Self {
            text: text.into(),
            confidence: confidence.clamp(0.0, 1.0),
            category,
            source: String::new(),
            speculative: false,
            tags: Vec::new(),
        }
    }

    /// Sets the source of this suggestion.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    /// Marks this suggestion as speculative.
    #[must_use]
    pub fn with_speculative(mut self, speculative: bool) -> Self {
        self.speculative = speculative;
        self
    }

    /// Adds a tag to this suggestion.
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Returns whether this candidate passes the confidence threshold.
    #[must_use]
    pub fn passes_threshold(&self, min_confidence: f64) -> bool {
        self.confidence >= min_confidence
    }
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// Simplified conversation message for suggestion context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMessage {
    /// Message role.
    pub role: String,
    /// Message content.
    pub content: String,
    /// Whether this message contains tool calls.
    pub has_tool_calls: bool,
    /// Tool names used in this message.
    pub tool_names: Vec<String>,
}

impl ContextMessage {
    /// Creates a new context message.
    #[must_use]
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            has_tool_calls: false,
            tool_names: Vec::new(),
        }
    }

    /// Creates a context message with tool calls.
    #[must_use]
    pub fn with_tools(role: impl Into<String>, content: impl Into<String>, tools: &[&str]) -> Self {
        Self {
            has_tool_calls: true,
            tool_names: tools.iter().map(|t| t.to_string()).collect(),
            ..Self::new(role, content)
        }
    }
}

/// Context provided to the suggestion engine.
#[derive(Debug, Clone)]
pub struct SuggestionContext {
    /// Recent conversation messages.
    pub messages: Vec<ContextMessage>,
    /// Files recently modified.
    pub recent_files: Vec<String>,
    /// Errors encountered recently.
    pub recent_errors: Vec<String>,
    /// Current working directory.
    pub cwd: Option<String>,
    /// Whether the session is interactive.
    pub is_interactive: bool,
    /// Number of assistant turns so far.
    pub assistant_turn_count: usize,
    /// Whether the last response was an error.
    pub last_response_is_error: bool,
    /// Whether there's a pending permission request.
    pub pending_permission: bool,
    /// Custom metadata for suggestion generators.
    pub metadata: HashMap<String, String>,
}

impl Default for SuggestionContext {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            recent_files: Vec::new(),
            recent_errors: Vec::new(),
            cwd: None,
            is_interactive: true,
            assistant_turn_count: 0,
            last_response_is_error: false,
            pending_permission: false,
            metadata: HashMap::new(),
        }
    }
}

impl SuggestionContext {
    /// Creates a new empty context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether suggestions should be suppressed.
    #[must_use]
    pub fn suppression_reason(&self) -> Option<&'static str> {
        if !self.is_interactive {
            return Some("non_interactive");
        }
        if self.pending_permission {
            return Some("pending_permission");
        }
        if self.last_response_is_error {
            return Some("last_response_error");
        }
        if self.assistant_turn_count < 2 {
            return Some("early_conversation");
        }
        None
    }

    /// Returns the last assistant message content.
    #[must_use]
    pub fn last_assistant_content(&self) -> Option<&str> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .map(|m| m.content.as_str())
    }

    /// Returns the last user message content.
    #[must_use]
    pub fn last_user_content(&self) -> Option<&str> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
    }

    /// Returns all tool names used in recent messages.
    #[must_use]
    pub fn recent_tools(&self) -> Vec<&str> {
        self.messages
            .iter()
            .rev()
            .flat_map(|m| m.tool_names.iter().map(String::as_str))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Suggestion Filters
// ---------------------------------------------------------------------------

/// Filter criteria for suggestions.
#[derive(Debug, Clone, Default)]
pub struct SuggestionFilter {
    /// Minimum confidence threshold.
    pub min_confidence: Option<f64>,
    /// Maximum text length.
    pub max_length: Option<usize>,
    /// Blocked substrings.
    pub blocked_patterns: Vec<String>,
    /// Whether to exclude speculative suggestions.
    pub exclude_speculative: bool,
}

impl SuggestionFilter {
    /// Creates a new filter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether a suggestion passes the filter.
    #[must_use]
    pub fn passes(&self, candidate: &SuggestionCandidate) -> bool {
        if let Some(min) = self.min_confidence
            && candidate.confidence < min
        {
            return false;
        }

        if let Some(max) = self.max_length
            && candidate.text.len() > max
        {
            return false;
        }

        if self.exclude_speculative && candidate.speculative {
            return false;
        }

        for pattern in &self.blocked_patterns {
            if candidate
                .text
                .to_lowercase()
                .contains(&pattern.to_lowercase())
            {
                return false;
            }
        }

        true
    }
}

// ---------------------------------------------------------------------------
// Prompt Suggestion Engine
// ---------------------------------------------------------------------------

/// Configuration for the suggestion engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSuggestionConfig {
    /// Whether prompt suggestions are enabled.
    pub enabled: bool,
    /// Speculation mode.
    pub speculation_mode: SpeculationMode,
    /// Maximum number of suggestions to return.
    pub max_suggestions: usize,
    /// Minimum confidence threshold.
    pub min_confidence: f64,
    /// Maximum suggestion text length.
    pub max_suggestion_length: usize,
}

impl Default for PromptSuggestionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            speculation_mode: SpeculationMode::Off,
            max_suggestions: 3,
            min_confidence: 0.5,
            max_suggestion_length: 200,
        }
    }
}

/// The prompt suggestion engine.
pub struct PromptSuggestionEngine {
    /// Configuration.
    config: PromptSuggestionConfig,
    /// Suggestion filter.
    filter: SuggestionFilter,
    /// Cached suggestions from previous generation.
    cached_suggestions: Vec<SuggestionCandidate>,
    /// Generation counter for cache invalidation.
    generation: u64,
}

impl Default for PromptSuggestionEngine {
    fn default() -> Self {
        Self::new(PromptSuggestionConfig::default())
    }
}

impl PromptSuggestionEngine {
    /// Creates a new engine with the given configuration.
    #[must_use]
    pub fn new(config: PromptSuggestionConfig) -> Self {
        Self {
            config,
            filter: SuggestionFilter::new(),
            cached_suggestions: Vec::new(),
            generation: 0,
        }
    }

    /// Returns whether the engine is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Returns the current speculation mode.
    #[must_use]
    pub fn speculation_mode(&self) -> SpeculationMode {
        self.config.speculation_mode
    }

    /// Returns the current generation counter.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Generates suggestions based on the given context.
    pub fn generate(&mut self, context: &SuggestionContext) -> Vec<SuggestionCandidate> {
        if !self.config.enabled {
            return Vec::new();
        }

        // Check suppression
        if context.suppression_reason().is_some() {
            return Vec::new();
        }

        let min_confidence = self
            .config
            .speculation_mode
            .min_confidence()
            .max(self.config.min_confidence);

        let max_suggestions = self
            .config
            .speculation_mode
            .max_suggestions()
            .min(self.config.max_suggestions);

        // Generate candidates
        let mut candidates = generate_suggestions(context, self.config.speculation_mode);

        // Apply filter
        self.filter.min_confidence = Some(min_confidence);
        self.filter.max_length = Some(self.config.max_suggestion_length);
        candidates.retain(|c| self.filter.passes(c));

        // Sort by confidence descending
        candidates.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Deduplicate by text
        let mut seen = std::collections::HashSet::new();
        candidates.retain(|c| seen.insert(c.text.to_lowercase()));

        // Truncate to max
        candidates.truncate(max_suggestions);

        self.cached_suggestions = candidates.clone();
        self.generation += 1;

        candidates
    }

    /// Returns the cached suggestions.
    #[must_use]
    pub fn cached_suggestions(&self) -> &[SuggestionCandidate] {
        &self.cached_suggestions
    }

    /// Sets a custom filter.
    pub fn set_filter(&mut self, filter: SuggestionFilter) {
        self.filter = filter;
    }

    /// Resets the engine state.
    pub fn reset(&mut self) {
        self.cached_suggestions.clear();
        self.generation = 0;
    }
}

// ---------------------------------------------------------------------------
// Suggestion Generation
// ---------------------------------------------------------------------------

/// Generates raw suggestion candidates from context.
pub fn generate_suggestions(
    context: &SuggestionContext,
    mode: SpeculationMode,
) -> Vec<SuggestionCandidate> {
    let mut candidates = Vec::new();

    // Generate from recent errors
    for error in &context.recent_errors {
        candidates.push(
            SuggestionCandidate::new(
                format!("Fix the error: {}", truncate_str(error, 100)),
                0.9,
                SuggestionCategory::Fix,
            )
            .with_source("error_detector"),
        );
    }

    // Generate from recent files
    for file in &context.recent_files {
        let ext = file.rsplit('.').next().unwrap_or("");
        match ext {
            "rs" => {
                candidates.push(
                    SuggestionCandidate::new(
                        format!("Run cargo test for {file}"),
                        0.7,
                        SuggestionCategory::Test,
                    )
                    .with_source("file_tracker"),
                );
            }
            "ts" | "js" => {
                candidates.push(
                    SuggestionCandidate::new(
                        format!("Run tests for {file}"),
                        0.7,
                        SuggestionCategory::Test,
                    )
                    .with_source("file_tracker"),
                );
            }
            _ => {}
        }
    }

    // Generate from last assistant message
    if let Some(last_content) = context.last_assistant_content() {
        let lower = last_content.to_lowercase();

        if lower.contains("error") || lower.contains("failed") {
            candidates.push(
                SuggestionCandidate::new(
                    "Investigate and fix the error",
                    0.85,
                    SuggestionCategory::Fix,
                )
                .with_source("context_analyzer"),
            );
        }

        if lower.contains("todo") || lower.contains("remaining") {
            candidates.push(
                SuggestionCandidate::new(
                    "Continue with the remaining tasks",
                    0.75,
                    SuggestionCategory::FollowUp,
                )
                .with_source("context_analyzer"),
            );
        }

        if lower.contains("refactor") {
            candidates.push(
                SuggestionCandidate::new(
                    "Apply the suggested refactoring",
                    0.7,
                    SuggestionCategory::Refactor,
                )
                .with_source("context_analyzer"),
            );
        }
    }

    // Generate from tool usage patterns
    let tools = context.recent_tools();
    if tools.contains(&"Read") && !tools.contains(&"Edit") {
        candidates.push(
            SuggestionCandidate::new(
                "Make the necessary changes",
                0.6,
                SuggestionCategory::FollowUp,
            )
            .with_source("tool_pattern"),
        );
    }

    if tools.contains(&"Edit") || tools.contains(&"Write") {
        candidates.push(
            SuggestionCandidate::new("Run tests to verify changes", 0.8, SuggestionCategory::Test)
                .with_source("tool_pattern"),
        );
    }

    // Speculative suggestions
    if mode != SpeculationMode::Off
        && let Some(user_msg) = context.last_user_content()
    {
        let lower = user_msg.to_lowercase();
        if lower.contains("implement") || lower.contains("build") || lower.contains("create") {
            candidates.push(
                SuggestionCandidate::new(
                    "Add tests for the new code",
                    mode.min_confidence(),
                    SuggestionCategory::Test,
                )
                .with_source("speculation")
                .with_speculative(true),
            );
            candidates.push(
                SuggestionCandidate::new(
                    "Review the implementation",
                    mode.min_confidence() * 0.9,
                    SuggestionCategory::Explore,
                )
                .with_source("speculation")
                .with_speculative(true),
            );
        }

        if lower.contains("fix") || lower.contains("bug") {
            candidates.push(
                SuggestionCandidate::new(
                    "Verify the fix works correctly",
                    mode.min_confidence(),
                    SuggestionCategory::Test,
                )
                .with_source("speculation")
                .with_speculative(true),
            );
        }
    }

    candidates
}

/// Truncates a string to a maximum length with ellipsis.
fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        // Find a safe byte boundary
        let mut end = max_len.saturating_sub(3);
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &s[..end]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- SpeculationMode tests ---

    #[test]
    fn test_speculation_mode_default() {
        assert_eq!(SpeculationMode::default(), SpeculationMode::Off);
    }

    #[test]
    fn test_speculation_mode_confidence_thresholds() {
        assert_eq!(SpeculationMode::Off.min_confidence(), 0.5);
        assert_eq!(SpeculationMode::Conservative.min_confidence(), 0.7);
        assert_eq!(SpeculationMode::Aggressive.min_confidence(), 0.4);
    }

    #[test]
    fn test_speculation_mode_max_suggestions() {
        assert_eq!(SpeculationMode::Off.max_suggestions(), 3);
        assert_eq!(SpeculationMode::Conservative.max_suggestions(), 5);
        assert_eq!(SpeculationMode::Aggressive.max_suggestions(), 8);
    }

    #[test]
    fn test_speculation_mode_from_str() {
        assert_eq!(SpeculationMode::from_str_opt("off"), SpeculationMode::Off);
        assert_eq!(
            SpeculationMode::from_str_opt("conservative"),
            SpeculationMode::Conservative
        );
        assert_eq!(
            SpeculationMode::from_str_opt("aggressive"),
            SpeculationMode::Aggressive
        );
        assert_eq!(
            SpeculationMode::from_str_opt("unknown"),
            SpeculationMode::Off
        );
    }

    #[test]
    fn test_speculation_mode_serialization() {
        let json = serde_json::to_string(&SpeculationMode::Conservative).expect("serialize");
        assert_eq!(json, "\"conservative\"");
    }

    // --- SuggestionCandidate tests ---

    #[test]
    fn test_candidate_new() {
        let c = SuggestionCandidate::new("Fix the bug", 0.9, SuggestionCategory::Fix);
        assert_eq!(c.text, "Fix the bug");
        assert!((c.confidence - 0.9).abs() < f64::EPSILON);
        assert_eq!(c.category, SuggestionCategory::Fix);
        assert!(!c.speculative);
    }

    #[test]
    fn test_candidate_confidence_clamped() {
        let c = SuggestionCandidate::new("test", 1.5, SuggestionCategory::FollowUp);
        assert!((c.confidence - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_candidate_with_source() {
        let c = SuggestionCandidate::new("test", 0.8, SuggestionCategory::Test)
            .with_source("unit_test");
        assert_eq!(c.source, "unit_test");
    }

    #[test]
    fn test_candidate_with_speculative() {
        let c = SuggestionCandidate::new("test", 0.8, SuggestionCategory::Explore)
            .with_speculative(true);
        assert!(c.speculative);
    }

    #[test]
    fn test_candidate_passes_threshold() {
        let c = SuggestionCandidate::new("test", 0.8, SuggestionCategory::FollowUp);
        assert!(c.passes_threshold(0.7));
        assert!(!c.passes_threshold(0.9));
    }

    // --- SuggestionContext tests ---

    #[test]
    fn test_context_default() {
        let ctx = SuggestionContext::new();
        assert!(ctx.messages.is_empty());
        assert!(ctx.is_interactive);
        assert_eq!(ctx.assistant_turn_count, 0);
    }

    #[test]
    fn test_context_suppression_non_interactive() {
        let mut ctx = SuggestionContext::new();
        ctx.is_interactive = false;
        assert_eq!(ctx.suppression_reason(), Some("non_interactive"));
    }

    #[test]
    fn test_context_suppression_early_conversation() {
        let ctx = SuggestionContext::new();
        assert_eq!(ctx.suppression_reason(), Some("early_conversation"));
    }

    #[test]
    fn test_context_suppression_pending_permission() {
        let mut ctx = SuggestionContext::new();
        ctx.is_interactive = true;
        ctx.assistant_turn_count = 5;
        ctx.pending_permission = true;
        assert_eq!(ctx.suppression_reason(), Some("pending_permission"));
    }

    #[test]
    fn test_context_no_suppression() {
        let mut ctx = SuggestionContext::new();
        ctx.is_interactive = true;
        ctx.assistant_turn_count = 5;
        assert_eq!(ctx.suppression_reason(), None);
    }

    #[test]
    fn test_context_last_assistant_content() {
        let mut ctx = SuggestionContext::new();
        ctx.messages.push(ContextMessage::new("user", "Hello"));
        ctx.messages
            .push(ContextMessage::new("assistant", "Hi there!"));
        assert_eq!(ctx.last_assistant_content(), Some("Hi there!"));
    }

    #[test]
    fn test_context_recent_tools() {
        let mut ctx = SuggestionContext::new();
        ctx.messages.push(ContextMessage::with_tools(
            "assistant",
            "Reading",
            &["Read", "Grep"],
        ));
        ctx.messages.push(ContextMessage::with_tools(
            "assistant",
            "Editing",
            &["Edit"],
        ));
        let tools = ctx.recent_tools();
        assert!(tools.contains(&"Edit"));
        assert!(tools.contains(&"Read"));
    }

    // --- SuggestionFilter tests ---

    #[test]
    fn test_filter_confidence() {
        let filter = SuggestionFilter {
            min_confidence: Some(0.8),
            ..SuggestionFilter::default()
        };
        let high = SuggestionCandidate::new("test", 0.9, SuggestionCategory::FollowUp);
        let low = SuggestionCandidate::new("test", 0.5, SuggestionCategory::FollowUp);
        assert!(filter.passes(&high));
        assert!(!filter.passes(&low));
    }

    #[test]
    fn test_filter_max_length() {
        let filter = SuggestionFilter {
            max_length: Some(10),
            ..SuggestionFilter::default()
        };
        let short = SuggestionCandidate::new("Hi", 0.9, SuggestionCategory::FollowUp);
        let long = SuggestionCandidate::new(
            "This is a very long suggestion",
            0.9,
            SuggestionCategory::FollowUp,
        );
        assert!(filter.passes(&short));
        assert!(!filter.passes(&long));
    }

    #[test]
    fn test_filter_blocked_patterns() {
        let filter = SuggestionFilter {
            blocked_patterns: vec!["delete".to_string()],
            ..SuggestionFilter::default()
        };
        let ok = SuggestionCandidate::new("Fix the bug", 0.9, SuggestionCategory::Fix);
        let blocked =
            SuggestionCandidate::new("Delete all files", 0.9, SuggestionCategory::FollowUp);
        assert!(filter.passes(&ok));
        assert!(!filter.passes(&blocked));
    }

    #[test]
    fn test_filter_exclude_speculative() {
        let filter = SuggestionFilter {
            exclude_speculative: true,
            ..SuggestionFilter::default()
        };
        let spec = SuggestionCandidate::new("test", 0.9, SuggestionCategory::Explore)
            .with_speculative(true);
        let direct = SuggestionCandidate::new("test", 0.9, SuggestionCategory::Explore);
        assert!(!filter.passes(&spec));
        assert!(filter.passes(&direct));
    }

    // --- PromptSuggestionEngine tests ---

    #[test]
    fn test_engine_default() {
        let engine = PromptSuggestionEngine::default();
        assert!(engine.is_enabled());
        assert_eq!(engine.speculation_mode(), SpeculationMode::Off);
        assert!(engine.cached_suggestions().is_empty());
    }

    #[test]
    fn test_engine_disabled() {
        let config = PromptSuggestionConfig {
            enabled: false,
            ..PromptSuggestionConfig::default()
        };
        let mut engine = PromptSuggestionEngine::new(config);
        let ctx = SuggestionContext::new();
        let results = engine.generate(&ctx);
        assert!(results.is_empty());
    }

    #[test]
    fn test_engine_generate_suppressed_early() {
        let mut engine = PromptSuggestionEngine::default();
        let ctx = SuggestionContext::new(); // assistant_turn_count = 0
        let results = engine.generate(&ctx);
        assert!(results.is_empty());
    }

    #[test]
    fn test_engine_generate_with_errors() {
        let mut engine = PromptSuggestionEngine::default();
        let mut ctx = SuggestionContext::new();
        ctx.is_interactive = true;
        ctx.assistant_turn_count = 5;
        ctx.recent_errors = vec!["Compilation error in main.rs".to_string()];

        let results = engine.generate(&ctx);
        assert!(!results.is_empty());
        assert!(results[0].text.contains("error"));
        assert_eq!(engine.generation(), 1);
    }

    #[test]
    fn test_engine_generate_with_tool_pattern() {
        let mut engine = PromptSuggestionEngine::default();
        let mut ctx = SuggestionContext::new();
        ctx.is_interactive = true;
        ctx.assistant_turn_count = 5;
        ctx.messages.push(ContextMessage::with_tools(
            "assistant",
            "Editing",
            &["Edit", "Write"],
        ));

        let results = engine.generate(&ctx);
        assert!(
            results
                .iter()
                .any(|r| r.category == SuggestionCategory::Test)
        );
    }

    #[test]
    fn test_engine_generate_with_speculation() {
        let config = PromptSuggestionConfig {
            speculation_mode: SpeculationMode::Conservative,
            ..PromptSuggestionConfig::default()
        };
        let mut engine = PromptSuggestionEngine::new(config);

        let mut ctx = SuggestionContext::new();
        ctx.is_interactive = true;
        ctx.assistant_turn_count = 5;
        ctx.messages
            .push(ContextMessage::new("user", "Implement the new feature"));

        let results = engine.generate(&ctx);
        assert!(results.iter().any(|r| r.speculative));
    }

    #[test]
    fn test_engine_reset() {
        let mut engine = PromptSuggestionEngine::default();
        let mut ctx = SuggestionContext::new();
        ctx.is_interactive = true;
        ctx.assistant_turn_count = 5;
        ctx.recent_errors = vec!["Error".to_string()];
        let _ = engine.generate(&ctx);

        engine.reset();
        assert!(engine.cached_suggestions().is_empty());
        assert_eq!(engine.generation(), 0);
    }

    // --- generate_suggestions function tests ---

    #[test]
    fn test_generate_suggestions_from_errors() {
        let mut ctx = SuggestionContext::new();
        ctx.recent_errors = vec!["Type mismatch".to_string()];
        let results = generate_suggestions(&ctx, SpeculationMode::Off);
        assert!(
            results
                .iter()
                .any(|r| r.category == SuggestionCategory::Fix)
        );
    }

    #[test]
    fn test_generate_suggestions_from_files() {
        let mut ctx = SuggestionContext::new();
        ctx.recent_files = vec!["src/main.rs".to_string()];
        let results = generate_suggestions(&ctx, SpeculationMode::Off);
        assert!(results.iter().any(|r| r.text.contains("cargo test")));
    }

    #[test]
    fn test_generate_suggestions_speculative_off() {
        let mut ctx = SuggestionContext::new();
        ctx.messages
            .push(ContextMessage::new("user", "Implement feature"));
        let results = generate_suggestions(&ctx, SpeculationMode::Off);
        assert!(!results.iter().any(|r| r.speculative));
    }

    #[test]
    fn test_generate_suggestions_speculative_on() {
        let mut ctx = SuggestionContext::new();
        ctx.messages
            .push(ContextMessage::new("user", "Implement feature"));
        let results = generate_suggestions(&ctx, SpeculationMode::Aggressive);
        assert!(results.iter().any(|r| r.speculative));
    }

    // --- truncate_str tests ---

    #[test]
    fn test_truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_str_long() {
        let result = truncate_str("hello world this is a long string", 15);
        assert!(result.len() <= 15);
    }
}
