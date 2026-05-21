//! Memory type system for persistent memory storage.
//!
//! Defines memory types, entries, and directory layout for the
//! project, user, and agent memory scopes.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// MemoryType enum
// ---------------------------------------------------------------------------

/// The scope of a memory entry.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// Project-scoped memory (stored in `.remote-code/memory/`).
    Project,
    /// User-scoped memory (stored in `~/.remote-code/memory/`).
    User,
    /// Agent-scoped memory (stored in agent-specific directory).
    Agent,
}

impl MemoryType {
    /// Return the directory name for this memory type.
    #[must_use]
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
            Self::Agent => "agent",
        }
    }

    /// All known memory type values.
    #[must_use]
    pub fn all_values() -> &'static [MemoryType] {
        &[MemoryType::Project, MemoryType::User, MemoryType::Agent]
    }

    /// Parse from string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "project" => Some(Self::Project),
            "user" => Some(Self::User),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.dir_name())
    }
}

/// All valid memory type values as string slices.
pub const MEMORY_TYPE_VALUES: &[&str] = &["project", "user", "agent"];

// ---------------------------------------------------------------------------
// MemoryEntry
// ---------------------------------------------------------------------------

/// A single memory entry with content, scope, and timestamp.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryEntry {
    /// The memory content text.
    pub content: String,
    /// The scope of this memory.
    pub scope: MemoryType,
    /// When this memory was created (ISO 8601).
    pub timestamp: String,
    /// Optional tags for categorization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl MemoryEntry {
    /// Create a new memory entry with the current timestamp.
    #[must_use]
    pub fn new(content: String, scope: MemoryType) -> Self {
        Self {
            content,
            scope,
            timestamp: chrono::Utc::now().to_rfc3339(),
            tags: vec![],
        }
    }

    /// Add tags to this memory entry.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Check if this entry has a specific tag.
    #[must_use]
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

// ---------------------------------------------------------------------------
// Memory directory
// ---------------------------------------------------------------------------

/// Get the memory directory path for a given scope.
///
/// # Arguments
///
/// * `base_dir` — The base directory (project root or home directory).
/// * `scope` — The memory scope.
///
/// # Returns
///
/// The path to the memory directory.
#[must_use]
pub fn memory_dir(base_dir: &Path, scope: MemoryType) -> PathBuf {
    base_dir
        .join(".remote-code")
        .join("memory")
        .join(scope.dir_name())
}

/// Get the file path for a specific memory entry.
///
/// # Arguments
///
/// * `base_dir` — The base directory.
/// * `scope` — The memory scope.
/// * `filename` — The memory filename.
///
/// # Returns
///
/// The full path to the memory file.
#[must_use]
pub fn memory_file_path(base_dir: &Path, scope: MemoryType, filename: &str) -> PathBuf {
    memory_dir(base_dir, scope).join(filename)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- MemoryType ---

    #[test]
    fn memory_type_dir_name() {
        assert_eq!(MemoryType::Project.dir_name(), "project");
        assert_eq!(MemoryType::User.dir_name(), "user");
        assert_eq!(MemoryType::Agent.dir_name(), "agent");
    }

    #[test]
    fn memory_type_display() {
        assert_eq!(MemoryType::Project.to_string(), "project");
    }

    #[test]
    fn memory_type_all_values() {
        assert_eq!(MemoryType::all_values().len(), 3);
    }

    #[test]
    fn memory_type_from_str_opt() {
        assert_eq!(
            MemoryType::from_str_opt("project"),
            Some(MemoryType::Project)
        );
        assert_eq!(MemoryType::from_str_opt("user"), Some(MemoryType::User));
        assert_eq!(MemoryType::from_str_opt("agent"), Some(MemoryType::Agent));
        assert_eq!(MemoryType::from_str_opt("unknown"), None);
    }

    #[test]
    fn memory_type_values_constant() {
        assert_eq!(MEMORY_TYPE_VALUES, &["project", "user", "agent"]);
    }

    #[test]
    fn memory_type_serialization_roundtrip() {
        for mt in MemoryType::all_values() {
            let json = serde_json::to_string(mt).expect("serialize");
            let deserialized: MemoryType = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*mt, deserialized);
        }
    }

    // --- MemoryEntry ---

    #[test]
    fn memory_entry_new() {
        let entry = MemoryEntry::new("test content".to_string(), MemoryType::Project);
        assert_eq!(entry.content, "test content");
        assert_eq!(entry.scope, MemoryType::Project);
        assert!(!entry.timestamp.is_empty());
        assert!(entry.tags.is_empty());
    }

    #[test]
    fn memory_entry_with_tags() {
        let entry = MemoryEntry::new("content".to_string(), MemoryType::User)
            .with_tags(vec!["important".to_string(), "coding".to_string()]);
        assert_eq!(entry.tags.len(), 2);
        assert!(entry.has_tag("important"));
        assert!(entry.has_tag("coding"));
        assert!(!entry.has_tag("other"));
    }

    #[test]
    fn memory_entry_serialization_roundtrip() {
        let entry = MemoryEntry::new("test".to_string(), MemoryType::Agent)
            .with_tags(vec!["tag1".to_string()]);
        let json = serde_json::to_string(&entry).expect("serialize");
        let deserialized: MemoryEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(entry.content, deserialized.content);
        assert_eq!(entry.scope, deserialized.scope);
        assert_eq!(entry.tags, deserialized.tags);
    }

    // --- memory_dir ---

    #[test]
    fn memory_dir_project() {
        let dir = memory_dir(Path::new("/project"), MemoryType::Project);
        assert_eq!(dir, PathBuf::from("/project/.remote-code/memory/project"));
    }

    #[test]
    fn memory_dir_user() {
        let dir = memory_dir(Path::new("/home"), MemoryType::User);
        assert_eq!(dir, PathBuf::from("/home/.remote-code/memory/user"));
    }

    #[test]
    fn memory_dir_agent() {
        let dir = memory_dir(Path::new("/project"), MemoryType::Agent);
        assert_eq!(dir, PathBuf::from("/project/.remote-code/memory/agent"));
    }

    // --- memory_file_path ---

    #[test]
    fn memory_file_path_test() {
        let path = memory_file_path(Path::new("/project"), MemoryType::Project, "notes.md");
        assert_eq!(
            path,
            PathBuf::from("/project/.remote-code/memory/project/notes.md")
        );
    }
}
