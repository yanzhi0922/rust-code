use crate::session::{ContentBlock, ConversationMessage, MessageRole};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashMap;
use std::path::Path;

const MEMORY_STORE_VERSION: u32 = 1;
const MAX_MEMORIES_PER_CATEGORY: usize = 200;
const MAX_MEMORY_CONTENT_LEN: usize = 1000;
const MEMORY_FILE_NAME: &str = "memories.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCategory {
    Pattern,
    Decision,
    Code,
    ToolResult,
    Annotation,
    Summary,
    Preference,
    Error,
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryCategory::Pattern => write!(f, "pattern"),
            MemoryCategory::Decision => write!(f, "decision"),
            MemoryCategory::Code => write!(f, "code"),
            MemoryCategory::ToolResult => write!(f, "tool_result"),
            MemoryCategory::Annotation => write!(f, "annotation"),
            MemoryCategory::Summary => write!(f, "summary"),
            MemoryCategory::Preference => write!(f, "preference"),
            MemoryCategory::Error => write!(f, "error"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub category: MemoryCategory,
    pub content: String,
    #[serde(default)]
    pub source: MemorySource,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub relevance_score: f64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub access_count: u32,
}

impl MemoryEntry {
    pub fn new(category: MemoryCategory, content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
            category,
            content: content.into(),
            source: MemorySource::default(),
            created_at: Utc::now(),
            updated_at: None,
            relevance_score: 0.5,
            tags: Vec::new(),
            access_count: 0,
        }
    }

    pub fn with_source(mut self, source: MemorySource) -> Self {
        self.source = source;
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn with_relevance(mut self, score: f64) -> Self {
        self.relevance_score = score.clamp(0.0, 1.0);
        self
    }

    pub fn touch(&mut self) {
        self.access_count += 1;
    }

    fn content_fingerprint(&self) -> String {
        let normalized = self.content.to_lowercase().trim().to_string();
        let mut hash = sha2::Sha256::new();
        sha2::Digest::update(&mut hash, normalized.as_bytes());
        sha2::Digest::update(&mut hash, self.category.to_string().as_bytes());
        format!("{:x}", sha2::Digest::finalize(hash))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemorySource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_number: Option<u32>,
}

impl MemorySource {
    pub fn from_message(index: usize) -> Self {
        Self {
            message_index: Some(index),
            ..Default::default()
        }
    }

    pub fn from_tool(tool_name: impl Into<String>, index: usize) -> Self {
        Self {
            message_index: Some(index),
            tool_name: Some(tool_name.into()),
            ..Default::default()
        }
    }

    pub fn from_turn(turn: u32) -> Self {
        Self {
            turn_number: Some(turn),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStore {
    pub version: u32,
    pub memories: Vec<MemoryEntry>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<DateTime<Utc>>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self {
            version: MEMORY_STORE_VERSION,
            memories: Vec::new(),
            metadata: HashMap::new(),
            last_updated: None,
        }
    }
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, entry: MemoryEntry) {
        let fingerprint = entry.content_fingerprint();
        if let Some(existing) = self
            .memories
            .iter_mut()
            .find(|m| m.content_fingerprint() == fingerprint)
        {
            existing.content = entry.content;
            existing.updated_at = Some(Utc::now());
            existing.relevance_score = existing.relevance_score.max(entry.relevance_score);
            for tag in entry.tags {
                if !existing.tags.contains(&tag) {
                    existing.tags.push(tag);
                }
            }
            return;
        }

        self.enforce_limits();
        self.memories.push(entry);
        self.last_updated = Some(Utc::now());
    }

    pub fn add_many(&mut self, entries: Vec<MemoryEntry>) {
        for entry in entries {
            self.add(entry);
        }
    }

    pub fn get(&self, id: &str) -> Option<&MemoryEntry> {
        self.memories.iter().find(|m| m.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut MemoryEntry> {
        self.memories.iter_mut().find(|m| m.id == id)
    }

    pub fn remove(&mut self, id: &str) -> Option<MemoryEntry> {
        if let Some(pos) = self.memories.iter().position(|m| m.id == id) {
            let removed = self.memories.remove(pos);
            self.last_updated = Some(Utc::now());
            Some(removed)
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.memories.clear();
        self.last_updated = Some(Utc::now());
    }

    pub fn len(&self) -> usize {
        self.memories.len()
    }

    pub fn is_empty(&self) -> bool {
        self.memories.is_empty()
    }

    pub fn by_category(&self, category: MemoryCategory) -> Vec<&MemoryEntry> {
        self.memories
            .iter()
            .filter(|m| m.category == category)
            .collect()
    }

    pub fn consolidate(&mut self) {
        let mut merged: Vec<MemoryEntry> = Vec::new();
        let mut seen_fingerprints: HashMap<String, usize> = HashMap::new();

        for entry in self.memories.drain(..) {
            let fp = entry.content_fingerprint();
            if let Some(&idx) = seen_fingerprints.get(&fp) {
                let existing = &mut merged[idx];
                existing.access_count += entry.access_count;
                existing.relevance_score = existing.relevance_score.max(entry.relevance_score);
                if entry.created_at < existing.created_at {
                    existing.created_at = entry.created_at;
                }
                if entry.updated_at.is_some() {
                    existing.updated_at = entry.updated_at;
                }
                for tag in entry.tags {
                    if !existing.tags.contains(&tag) {
                        existing.tags.push(tag);
                    }
                }
            } else {
                seen_fingerprints.insert(fp, merged.len());
                merged.push(entry);
            }
        }

        merged.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        self.memories = merged;
        self.last_updated = Some(Utc::now());
    }

    pub fn get_memories_for_context(&self, context: &str, max_entries: usize) -> Vec<&MemoryEntry> {
        let context_lower = context.to_lowercase();
        let context_words: Vec<&str> = context_lower
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .collect();

        let mut scored: Vec<(f64, &MemoryEntry)> = self
            .memories
            .iter()
            .map(|m| {
                let mut score = m.relevance_score;
                let content_lower = m.content.to_lowercase();
                for word in &context_words {
                    if content_lower.contains(word) {
                        score += 0.1;
                    }
                }
                for tag in &m.tags {
                    if context_lower.contains(&tag.to_lowercase()) {
                        score += 0.15;
                    }
                }
                (score, m)
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(max_entries);

        scored.into_iter().map(|(_, m)| m).collect()
    }

    pub fn save_to_file(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load_from_file(path: &Path) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let store: MemoryStore = serde_json::from_str(&json)?;
        Ok(store)
    }

    pub fn load_or_create(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            Self::load_from_file(path)
        } else {
            let store = Self::new();
            store.save_to_file(path)?;
            Ok(store)
        }
    }

    pub fn save_to_dir(&self, dir: &Path) -> anyhow::Result<()> {
        self.save_to_file(&dir.join(MEMORY_FILE_NAME))
    }

    pub fn load_from_dir(dir: &Path) -> anyhow::Result<Self> {
        Self::load_or_create(&dir.join(MEMORY_FILE_NAME))
    }

    fn enforce_limits(&mut self) {
        let mut counts: HashMap<MemoryCategory, usize> = HashMap::new();
        self.memories.retain(|m| {
            let count = counts.entry(m.category).or_insert(0);
            *count += 1;
            *count <= MAX_MEMORIES_PER_CATEGORY
        });

        if self.memories.len() > MAX_MEMORIES_PER_CATEGORY * 4 {
            self.memories.sort_by(|a, b| {
                b.relevance_score
                    .partial_cmp(&a.relevance_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            self.memories.truncate(MAX_MEMORIES_PER_CATEGORY * 4);
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExtractMemoriesConfig {
    pub extract_patterns: bool,
    pub extract_decisions: bool,
    pub extract_code: bool,
    pub extract_tool_results: bool,
    pub extract_annotations: bool,
    pub extract_errors: bool,
    pub max_entries_per_pass: usize,
}

impl Default for ExtractMemoriesConfig {
    fn default() -> Self {
        Self {
            extract_patterns: true,
            extract_decisions: true,
            extract_code: true,
            extract_tool_results: true,
            extract_annotations: true,
            extract_errors: true,
            max_entries_per_pass: 50,
        }
    }
}

pub fn extract_memories(
    messages: &[ConversationMessage],
    config: &ExtractMemoriesConfig,
) -> Vec<MemoryEntry> {
    let mut all_memories = Vec::new();
    let mut turn_number: u32 = 0;

    for (msg_idx, msg) in messages.iter().enumerate() {
        if matches!(msg.role, MessageRole::User) && !msg.tool_uses().is_empty() {
            turn_number += 1;
        }

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    if config.extract_annotations {
                        let mut entries = extract_annotations(text, msg_idx);
                        all_memories.append(&mut entries);
                    }
                    if config.extract_patterns {
                        let mut entries = extract_memory_patterns(text, msg_idx);
                        all_memories.append(&mut entries);
                    }
                    if config.extract_decisions {
                        let mut entries = extract_decision_patterns(text, msg_idx);
                        all_memories.append(&mut entries);
                    }
                    if config.extract_code {
                        let mut entries = extract_code_patterns(text, msg_idx);
                        all_memories.append(&mut entries);
                    }
                }
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    if config.extract_tool_results && !is_error.unwrap_or(false) {
                        let mut entries =
                            extract_memories_from_tool_results(content, msg_idx, turn_number);
                        all_memories.append(&mut entries);
                    }
                    if config.extract_errors && is_error.unwrap_or(false) {
                        let entry = MemoryEntry::new(
                            MemoryCategory::Error,
                            truncate_str(content, MAX_MEMORY_CONTENT_LEN),
                        )
                        .with_source(MemorySource::from_message(msg_idx))
                        .with_relevance(0.3);
                        all_memories.push(entry);
                    }
                }
                _ => {}
            }
        }
    }

    all_memories = merge_memories(all_memories);
    all_memories.truncate(config.max_entries_per_pass);
    all_memories
}

fn extract_annotations(text: &str, msg_idx: usize) -> Vec<MemoryEntry> {
    let patterns = [
        ("TODO:", MemoryCategory::Annotation, "todo"),
        ("FIXME:", MemoryCategory::Annotation, "fixme"),
        ("NOTE:", MemoryCategory::Annotation, "note"),
        ("IMPORTANT:", MemoryCategory::Annotation, "important"),
        ("REMEMBER:", MemoryCategory::Annotation, "remember"),
        ("KEY INSIGHT:", MemoryCategory::Annotation, "insight"),
        ("DISCOVERY:", MemoryCategory::Annotation, "discovery"),
        ("WARNING:", MemoryCategory::Annotation, "warning"),
        ("HACK:", MemoryCategory::Annotation, "hack"),
        ("BUG:", MemoryCategory::Annotation, "bug"),
        ("OPTIMIZE:", MemoryCategory::Annotation, "optimize"),
        ("SECURITY:", MemoryCategory::Annotation, "security"),
    ];

    let mut entries = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        for (pattern, category, tag) in &patterns {
            if trimmed.starts_with(pattern) {
                let content = truncate_str(trimmed, MAX_MEMORY_CONTENT_LEN);
                let relevance = match *tag {
                    "important" | "security" | "remember" => 0.9,
                    "fixme" | "bug" => 0.8,
                    "todo" | "warning" => 0.7,
                    "insight" | "discovery" => 0.75,
                    _ => 0.6,
                };
                entries.push(
                    MemoryEntry::new(*category, content)
                        .with_source(MemorySource::from_message(msg_idx))
                        .with_tags(vec![tag.to_string()])
                        .with_relevance(relevance),
                );
            }
        }
    }

    entries
}

fn extract_memory_patterns(text: &str, msg_idx: usize) -> Vec<MemoryEntry> {
    let mut entries = Vec::new();
    let pattern_indicators = [
        ("i always", 0.7),
        ("we always", 0.7),
        ("typically", 0.6),
        ("usually", 0.6),
        ("by default", 0.65),
        ("the convention", 0.7),
        ("the pattern", 0.7),
        ("best practice", 0.8),
        ("i prefer", 0.75),
        ("we prefer", 0.75),
        ("it's better to", 0.7),
        ("make sure to", 0.65),
        ("don't forget", 0.7),
        ("needs to be", 0.6),
        ("should always", 0.7),
        ("must always", 0.75),
    ];

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.len() < 10 {
            continue;
        }
        for (indicator, relevance) in &pattern_indicators {
            if trimmed.to_lowercase().contains(indicator) {
                let content = truncate_str(trimmed, MAX_MEMORY_CONTENT_LEN);
                entries.push(
                    MemoryEntry::new(MemoryCategory::Pattern, content)
                        .with_source(MemorySource::from_message(msg_idx))
                        .with_relevance(*relevance)
                        .with_tags(vec!["pattern".to_string()]),
                );
                break;
            }
        }
    }

    entries
}

fn extract_decision_patterns(text: &str, msg_idx: usize) -> Vec<MemoryEntry> {
    let mut entries = Vec::new();
    let decision_indicators = [
        ("decision:", 0.85),
        ("configuration:", 0.8),
        ("we decided", 0.8),
        ("i decided", 0.8),
        ("let's use", 0.7),
        ("let's go with", 0.7),
        ("going with", 0.7),
        ("chose to", 0.75),
        ("agreed to", 0.75),
        ("will use", 0.65),
        ("settled on", 0.75),
        ("the approach is", 0.7),
        ("the plan is", 0.7),
        ("the strategy is", 0.7),
    ];

    let preference_indicators = [
        ("i like", 0.6),
        ("i prefer", 0.7),
        ("please use", 0.65),
        ("use spaces", 0.7),
        ("use tabs", 0.7),
        ("indent with", 0.7),
        ("coding style", 0.65),
        ("style guide", 0.7),
        ("naming convention", 0.7),
    ];

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.len() < 10 {
            continue;
        }
        let lower = trimmed.to_lowercase();

        for (indicator, relevance) in &decision_indicators {
            if lower.contains(indicator) {
                let content = truncate_str(trimmed, MAX_MEMORY_CONTENT_LEN);
                entries.push(
                    MemoryEntry::new(MemoryCategory::Decision, content)
                        .with_source(MemorySource::from_message(msg_idx))
                        .with_relevance(*relevance)
                        .with_tags(vec!["decision".to_string()]),
                );
                break;
            }
        }

        for (indicator, relevance) in &preference_indicators {
            if lower.contains(indicator) {
                let content = truncate_str(trimmed, MAX_MEMORY_CONTENT_LEN);
                entries.push(
                    MemoryEntry::new(MemoryCategory::Preference, content)
                        .with_source(MemorySource::from_message(msg_idx))
                        .with_relevance(*relevance)
                        .with_tags(vec!["preference".to_string()]),
                );
                break;
            }
        }
    }

    entries
}

fn extract_code_patterns(text: &str, msg_idx: usize) -> Vec<MemoryEntry> {
    let mut entries = Vec::new();

    if !text.contains("fn ")
        && !text.contains("function ")
        && !text.contains("class ")
        && !text.contains("impl ")
        && !text.contains("struct ")
        && !text.contains("trait ")
        && !text.contains("pub ")
        && !text.contains("async ")
        && !text.contains("use ")
    {
        return entries;
    }

    let code_indicators = [
        ("// ", "comment"),
        ("/// ", "doc_comment"),
        ("/*", "block_comment"),
        ("#pragma", "pragma"),
        ("#define", "define"),
        ("#[", "attribute"),
        ("derive(", "derive"),
    ];

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        for (indicator, tag) in &code_indicators {
            if trimmed.contains(indicator) {
                let content = truncate_str(trimmed, MAX_MEMORY_CONTENT_LEN);
                entries.push(
                    MemoryEntry::new(MemoryCategory::Code, content)
                        .with_source(MemorySource::from_message(msg_idx))
                        .with_relevance(0.4)
                        .with_tags(vec!["code".to_string(), tag.to_string()]),
                );
                break;
            }
        }
    }

    entries
}

pub fn extract_memories_from_tool_results(
    text: &str,
    _msg_idx: usize,
    turn_number: u32,
) -> Vec<MemoryEntry> {
    let mut entries = Vec::new();

    let significant_patterns = [
        ("Error:", 0.4),
        ("error:", 0.4),
        ("failed:", 0.4),
        ("Warning:", 0.5),
        ("warning:", 0.5),
        ("success", 0.3),
        ("created", 0.35),
        ("deleted", 0.4),
        ("modified", 0.35),
        ("installed", 0.4),
        ("compilation", 0.5),
    ];

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.len() < 15 {
            continue;
        }
        for (pattern, relevance) in &significant_patterns {
            if trimmed.contains(pattern) {
                let content = truncate_str(trimmed, MAX_MEMORY_CONTENT_LEN);
                entries.push(
                    MemoryEntry::new(MemoryCategory::ToolResult, content)
                        .with_source(MemorySource::from_turn(turn_number))
                        .with_relevance(*relevance)
                        .with_tags(vec!["tool_result".to_string()]),
                );
                break;
            }
        }
    }

    entries
}

pub fn merge_memories(entries: Vec<MemoryEntry>) -> Vec<MemoryEntry> {
    let mut seen: HashMap<String, MemoryEntry> = HashMap::new();

    for entry in entries {
        let mut hasher = sha2::Sha256::new();
        sha2::Digest::update(&mut hasher, entry.content.to_lowercase().trim().as_bytes());
        sha2::Digest::update(&mut hasher, entry.category.to_string().as_bytes());
        let fingerprint = format!("{:x}", sha2::Digest::finalize(hasher));

        match seen.get_mut(&fingerprint) {
            Some(existing) => {
                existing.relevance_score = existing.relevance_score.max(entry.relevance_score);
                for tag in entry.tags {
                    if !existing.tags.contains(&tag) {
                        existing.tags.push(tag);
                    }
                }
                existing.access_count += 1;
            }
            None => {
                seen.insert(fingerprint, entry);
            }
        }
    }

    let mut merged: Vec<MemoryEntry> = seen.into_values().collect();
    merged.sort_by(|a, b| {
        b.relevance_score
            .partial_cmp(&a.relevance_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    merged
}

pub fn summarize_memories(entries: &[MemoryEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }

    let mut summary = String::from("[Memory Summary]\n");

    let mut by_category: HashMap<MemoryCategory, Vec<&MemoryEntry>> = HashMap::new();
    for entry in entries {
        by_category.entry(entry.category).or_default().push(entry);
    }

    let category_order = [
        MemoryCategory::Decision,
        MemoryCategory::Preference,
        MemoryCategory::Annotation,
        MemoryCategory::Pattern,
        MemoryCategory::Code,
        MemoryCategory::ToolResult,
        MemoryCategory::Error,
        MemoryCategory::Summary,
    ];

    for category in &category_order {
        if let Some(cat_entries) = by_category.get(category) {
            if cat_entries.is_empty() {
                continue;
            }
            summary.push_str(&format!("## {}\n", category));
            for entry in cat_entries.iter().take(10) {
                summary.push_str(&format!("- {}\n", entry.content));
            }
        }
    }

    summary.push_str(&format!("\n[{} memories total]", entries.len()));
    summary
}

pub fn extract_memories_to_store(
    messages: &[ConversationMessage],
    store: &mut MemoryStore,
    config: &ExtractMemoriesConfig,
) -> Vec<MemoryEntry> {
    let entries = extract_memories(messages, config);
    let result = entries.clone();
    store.add_many(entries);
    store.consolidate();
    result
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let truncated = &s[..max_len];
        if let Some(last_space) = truncated.rfind(' ') {
            format!("{}...", &s[..last_space])
        } else {
            format!("{}...", truncated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_user_msg(text: &str) -> ConversationMessage {
        ConversationMessage::user(text)
    }

    fn make_assistant_msg(text: &str) -> ConversationMessage {
        ConversationMessage::assistant_text(text)
    }

    #[test]
    fn test_memory_entry_creation() {
        let entry = MemoryEntry::new(MemoryCategory::Annotation, "NOTE: something important");
        assert_eq!(entry.category, MemoryCategory::Annotation);
        assert_eq!(entry.content, "NOTE: something important");
        assert_eq!(entry.relevance_score, 0.5);
        assert!(entry.id.len() == 8);
    }

    #[test]
    fn test_memory_entry_with_builders() {
        let entry = MemoryEntry::new(MemoryCategory::Pattern, "I always use tabs")
            .with_source(MemorySource::from_message(3))
            .with_tags(vec!["indentation".to_string()])
            .with_relevance(0.9);
        assert_eq!(entry.source.message_index, Some(3));
        assert_eq!(entry.tags, vec!["indentation"]);
        assert_eq!(entry.relevance_score, 0.9);
    }

    #[test]
    fn test_memory_entry_relevance_clamped() {
        let entry = MemoryEntry::new(MemoryCategory::Decision, "test").with_relevance(1.5);
        assert_eq!(entry.relevance_score, 1.0);
        let entry = MemoryEntry::new(MemoryCategory::Decision, "test").with_relevance(-0.5);
        assert_eq!(entry.relevance_score, 0.0);
    }

    #[test]
    fn test_memory_entry_touch() {
        let mut entry = MemoryEntry::new(MemoryCategory::Annotation, "test");
        assert_eq!(entry.access_count, 0);
        entry.touch();
        entry.touch();
        assert_eq!(entry.access_count, 2);
    }

    #[test]
    fn test_memory_entry_fingerprint() {
        let e1 = MemoryEntry::new(MemoryCategory::Annotation, "NOTE: same thing");
        let e2 = MemoryEntry::new(MemoryCategory::Annotation, "NOTE: same thing");
        assert_eq!(e1.content_fingerprint(), e2.content_fingerprint());

        let e3 = MemoryEntry::new(MemoryCategory::Pattern, "NOTE: same thing");
        assert_ne!(e1.content_fingerprint(), e3.content_fingerprint());

        let e4 = MemoryEntry::new(MemoryCategory::Annotation, "note: same thing");
        assert_eq!(e1.content_fingerprint(), e4.content_fingerprint());
    }

    #[test]
    fn test_memory_store_add_dedup() {
        let mut store = MemoryStore::new();
        store.add(MemoryEntry::new(MemoryCategory::Annotation, "NOTE: hello"));
        store.add(MemoryEntry::new(MemoryCategory::Annotation, "NOTE: hello"));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_memory_store_add_merge() {
        let mut store = MemoryStore::new();
        store.add(
            MemoryEntry::new(MemoryCategory::Annotation, "NOTE: hello")
                .with_relevance(0.5)
                .with_tags(vec!["a".to_string()]),
        );
        store.add(
            MemoryEntry::new(MemoryCategory::Annotation, "NOTE: hello")
                .with_relevance(0.9)
                .with_tags(vec!["b".to_string()]),
        );
        assert_eq!(store.len(), 1);
        let entry = store.get(&store.memories[0].id).unwrap();
        assert_eq!(entry.relevance_score, 0.9);
        assert!(entry.tags.contains(&"a".to_string()));
        assert!(entry.tags.contains(&"b".to_string()));
        assert!(entry.updated_at.is_some());
    }

    #[test]
    fn test_memory_store_get_remove() {
        let mut store = MemoryStore::new();
        store.add(MemoryEntry::new(
            MemoryCategory::Decision,
            "DECISION: use Rust",
        ));
        let id = store.memories[0].id.clone();
        assert!(store.get(&id).is_some());
        assert!(store.get("nonexistent").is_none());

        let removed = store.remove(&id).unwrap();
        assert_eq!(removed.content, "DECISION: use Rust");
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_memory_store_clear() {
        let mut store = MemoryStore::new();
        store.add(MemoryEntry::new(MemoryCategory::Annotation, "test"));
        store.add(MemoryEntry::new(MemoryCategory::Decision, "test2"));
        assert_eq!(store.len(), 2);
        store.clear();
        assert!(store.is_empty());
    }

    #[test]
    fn test_memory_store_by_category() {
        let mut store = MemoryStore::new();
        store.add(MemoryEntry::new(MemoryCategory::Annotation, "a"));
        store.add(MemoryEntry::new(MemoryCategory::Decision, "d"));
        store.add(MemoryEntry::new(MemoryCategory::Annotation, "b"));

        let annotations = store.by_category(MemoryCategory::Annotation);
        assert_eq!(annotations.len(), 2);
        let decisions = store.by_category(MemoryCategory::Decision);
        assert_eq!(decisions.len(), 1);
    }

    #[test]
    fn test_memory_store_consolidate() {
        let mut store = MemoryStore::new();
        store.add(MemoryEntry::new(MemoryCategory::Annotation, "NOTE: foo"));
        store.memories.push(
            MemoryEntry::new(MemoryCategory::Annotation, "NOTE: foo")
                .with_relevance(0.1)
                .with_tags(vec!["extra".to_string()]),
        );
        assert_eq!(store.len(), 2);
        store.consolidate();
        assert_eq!(store.len(), 1);
        assert!(store.memories[0].tags.contains(&"extra".to_string()));
    }

    #[test]
    fn test_memory_store_consolidate_sorts_by_relevance() {
        let mut store = MemoryStore::new();
        store.add(MemoryEntry::new(MemoryCategory::Annotation, "low").with_relevance(0.1));
        store.add(MemoryEntry::new(MemoryCategory::Decision, "high").with_relevance(0.9));
        store.add(MemoryEntry::new(MemoryCategory::Pattern, "mid").with_relevance(0.5));
        store.consolidate();
        assert_eq!(store.memories[0].content, "high");
        assert_eq!(store.memories[1].content, "mid");
        assert_eq!(store.memories[2].content, "low");
    }

    #[test]
    fn test_memory_store_get_memories_for_context() {
        let mut store = MemoryStore::new();
        store.add(
            MemoryEntry::new(MemoryCategory::Decision, "DECISION: use Rust for backend")
                .with_relevance(0.8),
        );
        store.add(
            MemoryEntry::new(MemoryCategory::Pattern, "I prefer spaces over tabs")
                .with_relevance(0.5),
        );
        store.add(
            MemoryEntry::new(MemoryCategory::Annotation, "NOTE: the API uses REST")
                .with_tags(vec!["api".to_string()])
                .with_relevance(0.6),
        );

        let results = store.get_memories_for_context("Rust backend architecture", 2);
        assert!(results.len() <= 2);
        assert!(results[0].content.contains("Rust"));
    }

    #[test]
    fn test_memory_store_save_load() {
        let dir = std::env::temp_dir().join("claude_test_memories");
        std::fs::create_dir_all(&dir).ok();

        let mut store = MemoryStore::new();
        store.add(MemoryEntry::new(
            MemoryCategory::Annotation,
            "NOTE: test memory",
        ));
        store.add(MemoryEntry::new(
            MemoryCategory::Decision,
            "DECISION: use async",
        ));

        store.save_to_dir(&dir).expect("save failed");
        let loaded = MemoryStore::load_from_dir(&dir).expect("load failed");
        assert_eq!(loaded.len(), 2);
        assert!(loaded
            .memories
            .iter()
            .any(|m| m.content.contains("test memory")));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_memory_store_load_or_create() {
        let dir = std::env::temp_dir().join("claude_test_memories_new");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok();

        let store = MemoryStore::load_or_create(&dir.join(MEMORY_FILE_NAME)).expect("load failed");
        assert!(store.is_empty());

        let mut store2 =
            MemoryStore::load_or_create(&dir.join(MEMORY_FILE_NAME)).expect("load2 failed");
        store2.add(MemoryEntry::new(MemoryCategory::Annotation, "test"));
        store2.save_to_dir(&dir).expect("save failed");

        let store3 = MemoryStore::load_from_dir(&dir).expect("load3 failed");
        assert_eq!(store3.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_extract_annotations() {
        let msgs = vec![
            make_user_msg("hello"),
            make_assistant_msg(
                "NOTE: parser uses regex\nTODO: fix edge case\nIMPORTANT: check bounds first",
            ),
            make_user_msg("ok"),
        ];
        let config = ExtractMemoriesConfig {
            extract_patterns: false,
            extract_decisions: false,
            extract_code: false,
            extract_tool_results: false,
            extract_annotations: true,
            extract_errors: false,
            ..Default::default()
        };
        let entries = extract_memories(&msgs, &config);
        assert!(entries
            .iter()
            .any(|e| e.content.contains("NOTE: parser uses regex")));
        assert!(entries
            .iter()
            .any(|e| e.content.contains("TODO: fix edge case")));
        assert!(entries
            .iter()
            .any(|e| e.content.contains("IMPORTANT: check bounds first")));
    }

    #[test]
    fn test_extract_decision_patterns() {
        let msgs = vec![
            make_user_msg("hello"),
            make_assistant_msg(
                "DECISION: use PostgreSQL for the database\nI prefer spaces over tabs",
            ),
            make_user_msg("ok"),
        ];
        let config = ExtractMemoriesConfig {
            extract_patterns: false,
            extract_decisions: true,
            extract_code: false,
            extract_tool_results: false,
            extract_annotations: false,
            extract_errors: false,
            ..Default::default()
        };
        let entries = extract_memories(&msgs, &config);
        assert!(entries
            .iter()
            .any(|e| e.category == MemoryCategory::Decision));
        assert!(entries
            .iter()
            .any(|e| e.category == MemoryCategory::Preference));
    }

    #[test]
    fn test_extract_code_patterns() {
        let msgs = vec![
            make_user_msg("hello"),
            make_assistant_msg("Here's the impl:\n```rust\n// This is a comment\nfn main() {}\n/// Doc comment\npub struct Foo;\n```"),
            make_user_msg("ok"),
        ];
        let config = ExtractMemoriesConfig {
            extract_patterns: false,
            extract_decisions: false,
            extract_code: true,
            extract_tool_results: false,
            extract_annotations: false,
            extract_errors: false,
            ..Default::default()
        };
        let entries = extract_memories(&msgs, &config);
        assert!(!entries.is_empty());
        assert!(entries.iter().all(|e| e.category == MemoryCategory::Code));
    }

    #[test]
    fn test_extract_memory_patterns() {
        let msgs = vec![
            make_user_msg("hello"),
            make_assistant_msg(
                "I always use this pattern for error handling. By default, we log errors.",
            ),
            make_user_msg("ok"),
        ];
        let config = ExtractMemoriesConfig {
            extract_patterns: true,
            extract_decisions: false,
            extract_code: false,
            extract_tool_results: false,
            extract_annotations: false,
            extract_errors: false,
            ..Default::default()
        };
        let entries = extract_memories(&msgs, &config);
        assert!(entries.iter().any(|e| e.content.contains("I always use")));
        assert!(entries.iter().any(|e| e.content.contains("By default")));
    }

    #[test]
    fn test_extract_tool_results() {
        let tool_result = ConversationMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                content: "Warning: deprecated API used\nError: compilation failed".to_string(),
                is_error: Some(false),
            }],
            model: None,
        };
        let msgs = vec![make_user_msg("hello"), tool_result, make_user_msg("ok")];
        let config = ExtractMemoriesConfig {
            extract_patterns: false,
            extract_decisions: false,
            extract_code: false,
            extract_tool_results: true,
            extract_annotations: false,
            extract_errors: false,
            ..Default::default()
        };
        let entries = extract_memories(&msgs, &config);
        assert!(entries
            .iter()
            .any(|e| e.content.contains("Warning: deprecated")));
    }

    #[test]
    fn test_extract_error_from_tool_result() {
        let tool_result = ConversationMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                content: "Error: file not found, check the path".to_string(),
                is_error: Some(true),
            }],
            model: None,
        };
        let msgs = vec![make_user_msg("hello"), tool_result];
        let config = ExtractMemoriesConfig {
            extract_patterns: false,
            extract_decisions: false,
            extract_code: false,
            extract_tool_results: false,
            extract_annotations: false,
            extract_errors: true,
            ..Default::default()
        };
        let entries = extract_memories(&msgs, &config);
        assert!(entries.iter().any(|e| e.category == MemoryCategory::Error));
    }

    #[test]
    fn test_merge_memories_dedup() {
        let entries = vec![
            MemoryEntry::new(MemoryCategory::Annotation, "NOTE: same"),
            MemoryEntry::new(MemoryCategory::Annotation, "NOTE: same"),
            MemoryEntry::new(MemoryCategory::Decision, "DECISION: use Rust"),
        ];
        let merged = merge_memories(entries);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn test_merge_memories_keeps_higher_relevance() {
        let entries = vec![
            MemoryEntry::new(MemoryCategory::Annotation, "NOTE: test").with_relevance(0.3),
            MemoryEntry::new(MemoryCategory::Annotation, "NOTE: test").with_relevance(0.9),
        ];
        let merged = merge_memories(entries);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].relevance_score, 0.9);
    }

    #[test]
    fn test_merge_memories_sorted_by_relevance() {
        let entries = vec![
            MemoryEntry::new(MemoryCategory::Annotation, "NOTE: low").with_relevance(0.1),
            MemoryEntry::new(MemoryCategory::Decision, "DECISION: high").with_relevance(0.9),
            MemoryEntry::new(MemoryCategory::Pattern, "PATTERN: mid").with_relevance(0.5),
        ];
        let merged = merge_memories(entries);
        assert_eq!(merged[0].content, "DECISION: high");
        assert_eq!(merged[2].content, "NOTE: low");
    }

    #[test]
    fn test_summarize_memories_empty() {
        let summary = summarize_memories(&[]);
        assert!(summary.is_empty());
    }

    #[test]
    fn test_summarize_memories() {
        let entries = vec![
            MemoryEntry::new(MemoryCategory::Decision, "DECISION: use Rust"),
            MemoryEntry::new(MemoryCategory::Annotation, "NOTE: important"),
            MemoryEntry::new(MemoryCategory::Pattern, "I always use tabs"),
        ];
        let summary = summarize_memories(&entries);
        assert!(summary.contains("[Memory Summary]"));
        assert!(summary.contains("decision"));
        assert!(summary.contains("annotation"));
        assert!(summary.contains("[3 memories total]"));
    }

    #[test]
    fn test_extract_memories_to_store() {
        let msgs = vec![
            make_user_msg("hello"),
            make_assistant_msg("NOTE: something\nDECISION: use async"),
            make_user_msg("ok"),
        ];
        let mut store = MemoryStore::new();
        let config = ExtractMemoriesConfig::default();
        let entries = extract_memories_to_store(&msgs, &mut store, &config);
        assert!(!entries.is_empty());
        assert_eq!(store.len(), entries.len());
    }

    #[test]
    fn test_extract_memories_max_entries() {
        let mut msgs = vec![make_user_msg("start")];
        for i in 0..60 {
            msgs.push(make_assistant_msg(&format!("NOTE: memory item {i}")));
        }
        msgs.push(make_user_msg("end"));

        let config = ExtractMemoriesConfig {
            max_entries_per_pass: 5,
            ..Default::default()
        };
        let entries = extract_memories(&msgs, &config);
        assert!(entries.len() <= 5);
    }

    #[test]
    fn test_full_extraction_pipeline() {
        let msgs = vec![
            make_user_msg("Let's build a web server"),
            make_assistant_msg(
                "DECISION: We'll use Axum for the web framework\nNOTE: Axum requires tokio runtime\nI always use this pattern for route handlers",
            ),
            make_user_msg("sounds good"),
            make_assistant_msg(
                "TODO: Add middleware for auth\nIMPORTANT: validate all inputs\n// handler comment\nfn handle_request() {}",
            ),
            make_user_msg("done"),
        ];
        let config = ExtractMemoriesConfig::default();
        let entries = extract_memories(&msgs, &config);

        assert!(entries
            .iter()
            .any(|e| matches!(e.category, MemoryCategory::Decision)));
        assert!(entries
            .iter()
            .any(|e| matches!(e.category, MemoryCategory::Annotation)));
        assert!(entries
            .iter()
            .any(|e| matches!(e.category, MemoryCategory::Pattern)));
        assert!(entries
            .iter()
            .any(|e| matches!(e.category, MemoryCategory::Code)));
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        let long = "a ".repeat(100);
        let truncated = truncate_str(&long, 50);
        assert!(truncated.len() <= 53);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_memory_source() {
        let s1 = MemorySource::from_message(5);
        assert_eq!(s1.message_index, Some(5));

        let s2 = MemorySource::from_tool("BashTool", 3);
        assert_eq!(s2.tool_name, Some("BashTool".to_string()));
        assert_eq!(s2.message_index, Some(3));

        let s3 = MemorySource::from_turn(7);
        assert_eq!(s3.turn_number, Some(7));
    }
}
