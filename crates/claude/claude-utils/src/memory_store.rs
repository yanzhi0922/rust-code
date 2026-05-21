//! Memory store — file-system-backed persistent memory storage.
//!
//! Provides a [`MemoryStore`] that persists memory entries to the local
//! file system under `.remote-code/memory/`. Each entry is stored as a
//! JSON file keyed by a user-provided name.

use std::{collections::HashMap, fs, path::PathBuf};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::memory_types::{MemoryEntry, MemoryType, memory_dir};

// ---------------------------------------------------------------------------
// Store-level entry (adds key and filename tracking)
// ---------------------------------------------------------------------------

/// A stored memory entry with its key and file path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredMemory {
    /// Unique key for this memory entry.
    pub key: String,
    /// The memory content.
    pub entry: MemoryEntry,
    /// File path where this memory is stored.
    #[serde(skip)]
    pub path: PathBuf,
}

// ---------------------------------------------------------------------------
// Memory Store
// ---------------------------------------------------------------------------

/// File-system-backed memory store.
///
/// Persists memory entries as JSON files under the memory directory
/// for a given scope.
#[derive(Debug, Clone)]
pub struct MemoryStore {
    /// Base directory for memory storage.
    base_dir: PathBuf,
}

impl MemoryStore {
    /// Create a new memory store rooted at the given base directory.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Return the memory directory for a given scope.
    #[must_use]
    pub fn scope_dir(&self, scope: MemoryType) -> PathBuf {
        memory_dir(&self.base_dir, scope)
    }

    // -- CRUD operations ----------------------------------------------------

    /// Save a memory entry under the given key.
    pub fn save_memory(
        &self,
        key: &str,
        content: &str,
        scope: MemoryType,
        tags: Vec<String>,
    ) -> Result<StoredMemory> {
        let dir = self.scope_dir(scope);
        fs::create_dir_all(&dir).map_err(|e| anyhow!("failed to create memory dir: {e}"))?;

        let entry = MemoryEntry::new(content.to_owned(), scope).with_tags(tags);
        let path = dir.join(format!("{key}.json"));
        let json = serde_json::to_string_pretty(&entry)
            .map_err(|e| anyhow!("failed to serialize memory entry: {e}"))?;
        fs::write(&path, json).map_err(|e| anyhow!("failed to write memory file: {e}"))?;

        Ok(StoredMemory {
            key: key.to_owned(),
            entry,
            path,
        })
    }

    /// Load a memory entry by key and scope.
    pub fn load_memory(&self, key: &str, scope: MemoryType) -> Result<StoredMemory> {
        let path = self.scope_dir(scope).join(format!("{key}.json"));
        let content =
            fs::read_to_string(&path).map_err(|e| anyhow!("failed to read memory `{key}`: {e}"))?;
        let entry: MemoryEntry = serde_json::from_str(&content)
            .map_err(|e| anyhow!("failed to parse memory `{key}`: {e}"))?;

        Ok(StoredMemory {
            key: key.to_owned(),
            entry,
            path,
        })
    }

    /// Delete a memory entry by key and scope.
    pub fn delete_memory(&self, key: &str, scope: MemoryType) -> Result<()> {
        let path = self.scope_dir(scope).join(format!("{key}.json"));
        if !path.exists() {
            return Err(anyhow!("memory `{key}` not found in {} scope", scope));
        }
        fs::remove_file(&path).map_err(|e| anyhow!("failed to delete memory `{key}`: {e}"))?;
        Ok(())
    }

    /// List all memory entries in a given scope.
    pub fn list_memories(&self, scope: MemoryType) -> Result<Vec<StoredMemory>> {
        let dir = self.scope_dir(scope);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| anyhow!("failed to read memory dir: {e}"))? {
            let entry = entry.map_err(|e| anyhow!("failed to read dir entry: {e}"))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            let content = fs::read_to_string(&path)
                .map_err(|e| anyhow!("failed to read memory file: {e}"))?;
            let mem_entry: MemoryEntry = serde_json::from_str(&content)
                .map_err(|e| anyhow!("failed to parse memory `{stem}`: {e}"))?;

            entries.push(StoredMemory {
                key: stem.to_owned(),
                entry: mem_entry,
                path,
            });
        }

        entries.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(entries)
    }

    /// List memories across all scopes.
    pub fn list_all_memories(&self) -> Result<HashMap<MemoryType, Vec<StoredMemory>>> {
        let mut result = HashMap::new();
        for scope in MemoryType::all_values() {
            result.insert(*scope, self.list_memories(*scope)?);
        }
        Ok(result)
    }

    /// Search memories by content or tags across all scopes.
    pub fn search_memory(&self, query: &str) -> Result<Vec<StoredMemory>> {
        let lower = query.to_ascii_lowercase();
        let mut results = Vec::new();

        for scope in MemoryType::all_values() {
            let entries = self.list_memories(*scope)?;
            for stored in entries {
                let content_match = stored.entry.content.to_ascii_lowercase().contains(&lower);
                let tag_match = stored
                    .entry
                    .tags
                    .iter()
                    .any(|t| t.to_ascii_lowercase().contains(&lower));
                let key_match = stored.key.to_ascii_lowercase().contains(&lower);

                if content_match || tag_match || key_match {
                    results.push(stored);
                }
            }
        }

        results.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(results)
    }

    /// Check if a memory entry exists.
    #[must_use]
    pub fn exists(&self, key: &str, scope: MemoryType) -> bool {
        self.scope_dir(scope).join(format!("{key}.json")).exists()
    }

    /// Count memory entries in a given scope.
    pub fn count(&self, scope: MemoryType) -> Result<usize> {
        Ok(self.list_memories(scope)?.len())
    }

    /// Update an existing memory entry (upsert semantics).
    pub fn update_memory(
        &self,
        key: &str,
        content: &str,
        scope: MemoryType,
        tags: Vec<String>,
    ) -> Result<StoredMemory> {
        if !self.exists(key, scope) {
            return Err(anyhow!("memory `{key}` not found in {} scope", scope));
        }
        self.save_memory(key, content, scope, tags)
    }

    /// Get all tags used across all memories.
    pub fn all_tags(&self) -> Result<Vec<String>> {
        let mut tags = Vec::new();
        for scope in MemoryType::all_values() {
            for stored in self.list_memories(*scope)? {
                tags.extend(stored.entry.tags);
            }
        }
        tags.sort();
        tags.dedup();
        Ok(tags)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("unexpected error: {error}"),
        }
    }

    // --- save & load ---

    #[test]
    fn save_and_load_memory() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        let stored = ok(store.save_memory(
            "test-key",
            "Hello, memory!",
            MemoryType::Project,
            vec!["test".to_owned()],
        ));
        assert_eq!(stored.key, "test-key");
        assert_eq!(stored.entry.content, "Hello, memory!");
        assert_eq!(stored.entry.scope, MemoryType::Project);
        assert!(stored.entry.has_tag("test"));

        let loaded = ok(store.load_memory("test-key", MemoryType::Project));
        assert_eq!(loaded.key, "test-key");
        assert_eq!(loaded.entry.content, "Hello, memory!");
    }

    #[test]
    fn save_creates_directory() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        assert!(!store.scope_dir(MemoryType::User).exists());
        ok(store.save_memory("x", "content", MemoryType::User, vec![]));
        assert!(store.scope_dir(MemoryType::User).exists());
    }

    #[test]
    fn load_nonexistent_fails() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        let err = store
            .load_memory("nope", MemoryType::Project)
            .expect_err("missing memory should return an error");
        assert!(err.to_string().contains("nope"));
    }

    // --- delete ---

    #[test]
    fn delete_memory() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        ok(store.save_memory("del-me", "bye", MemoryType::Project, vec![]));
        assert!(store.exists("del-me", MemoryType::Project));
        ok(store.delete_memory("del-me", MemoryType::Project));
        assert!(!store.exists("del-me", MemoryType::Project));
    }

    #[test]
    fn delete_nonexistent_fails() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        let err = store
            .delete_memory("ghost", MemoryType::Project)
            .expect_err("deleting a missing memory should return an error");
        assert!(err.to_string().contains("ghost"));
    }

    // --- list ---

    #[test]
    fn list_memories_empty() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        let list = ok(store.list_memories(MemoryType::Project));
        assert!(list.is_empty());
    }

    #[test]
    fn list_memories_multiple() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        ok(store.save_memory("alpha", "A", MemoryType::Project, vec![]));
        ok(store.save_memory("beta", "B", MemoryType::Project, vec![]));
        ok(store.save_memory("gamma", "C", MemoryType::Project, vec![]));

        let list = ok(store.list_memories(MemoryType::Project));
        assert_eq!(list.len(), 3);
        // Sorted by key
        assert_eq!(list[0].key, "alpha");
        assert_eq!(list[1].key, "beta");
        assert_eq!(list[2].key, "gamma");
    }

    #[test]
    fn list_memories_isolated_by_scope() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        ok(store.save_memory("shared-key", "project content", MemoryType::Project, vec![]));
        ok(store.save_memory("shared-key", "user content", MemoryType::User, vec![]));

        let project = ok(store.list_memories(MemoryType::Project));
        let user = ok(store.list_memories(MemoryType::User));
        assert_eq!(project.len(), 1);
        assert_eq!(user.len(), 1);
        assert_eq!(project[0].entry.content, "project content");
        assert_eq!(user[0].entry.content, "user content");
    }

    // --- list_all ---

    #[test]
    fn list_all_memories() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        ok(store.save_memory("p1", "P1", MemoryType::Project, vec![]));
        ok(store.save_memory("u1", "U1", MemoryType::User, vec![]));

        let all = ok(store.list_all_memories());
        assert_eq!(all[&MemoryType::Project].len(), 1);
        assert_eq!(all[&MemoryType::User].len(), 1);
        assert_eq!(all[&MemoryType::Agent].len(), 0);
    }

    // --- search ---

    #[test]
    fn search_by_content() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        ok(store.save_memory(
            "note1",
            "Rust programming tips",
            MemoryType::Project,
            vec![],
        ));
        ok(store.save_memory("note2", "Python data science", MemoryType::Project, vec![]));

        let results = ok(store.search_memory("rust"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "note1");
    }

    #[test]
    fn search_by_tag() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        ok(store.save_memory(
            "tagged",
            "content",
            MemoryType::Project,
            vec!["important".to_owned()],
        ));
        ok(store.save_memory("untagged", "content", MemoryType::Project, vec![]));

        let results = ok(store.search_memory("important"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "tagged");
    }

    #[test]
    fn search_by_key() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        ok(store.save_memory(
            "deploy-notes",
            "deploy content",
            MemoryType::Project,
            vec![],
        ));
        ok(store.save_memory("test-notes", "test content", MemoryType::Project, vec![]));

        let results = ok(store.search_memory("deploy"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "deploy-notes");
    }

    #[test]
    fn search_case_insensitive() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        ok(store.save_memory("upper", "UPPERCASE CONTENT", MemoryType::Project, vec![]));

        let results = ok(store.search_memory("uppercase"));
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_no_results() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        ok(store.save_memory("x", "content", MemoryType::Project, vec![]));

        let results = ok(store.search_memory("nonexistent"));
        assert!(results.is_empty());
    }

    // --- exists & count ---

    #[test]
    fn exists_check() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        assert!(!store.exists("nope", MemoryType::Project));
        ok(store.save_memory("nope", "now exists", MemoryType::Project, vec![]));
        assert!(store.exists("nope", MemoryType::Project));
    }

    #[test]
    fn count_entries() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        assert_eq!(ok(store.count(MemoryType::Project)), 0);
        ok(store.save_memory("a", "A", MemoryType::Project, vec![]));
        ok(store.save_memory("b", "B", MemoryType::Project, vec![]));
        assert_eq!(ok(store.count(MemoryType::Project)), 2);
    }

    // --- update ---

    #[test]
    fn update_memory() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        ok(store.save_memory("upd", "original", MemoryType::Project, vec![]));
        ok(store.update_memory(
            "upd",
            "updated",
            MemoryType::Project,
            vec!["new-tag".to_owned()],
        ));

        let loaded = ok(store.load_memory("upd", MemoryType::Project));
        assert_eq!(loaded.entry.content, "updated");
        assert!(loaded.entry.has_tag("new-tag"));
    }

    #[test]
    fn update_nonexistent_fails() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        let err = store
            .update_memory("ghost", "content", MemoryType::Project, vec![])
            .expect_err("updating a missing memory should return an error");
        assert!(err.to_string().contains("ghost"));
    }

    // --- all_tags ---

    #[test]
    fn all_tags_deduped() {
        let temp = ok(tempdir());
        let store = MemoryStore::new(temp.path());
        ok(store.save_memory(
            "a",
            "A",
            MemoryType::Project,
            vec!["rust".to_owned(), "code".to_owned()],
        ));
        ok(store.save_memory(
            "b",
            "B",
            MemoryType::User,
            vec!["rust".to_owned(), "tips".to_owned()],
        ));

        let tags = ok(store.all_tags());
        assert_eq!(
            tags,
            vec!["code".to_owned(), "rust".to_owned(), "tips".to_owned()]
        );
    }

    // --- scope_dir ---

    #[test]
    fn scope_dir_path() {
        let store = MemoryStore::new("/tmp/test");
        assert_eq!(
            store.scope_dir(MemoryType::Project),
            PathBuf::from("/tmp/test/.remote-code/memory/project")
        );
        assert_eq!(
            store.scope_dir(MemoryType::User),
            PathBuf::from("/tmp/test/.remote-code/memory/user")
        );
    }

    // --- StoredMemory serialization ---

    #[test]
    fn stored_memory_serialization_roundtrip() {
        let entry = MemoryEntry::new("test".to_owned(), MemoryType::Project)
            .with_tags(vec!["tag1".to_owned()]);
        let stored = StoredMemory {
            key: "test-key".to_owned(),
            entry,
            path: PathBuf::from("/tmp/test.json"),
        };
        let json = serde_json::to_string(&stored).expect("serialize");
        let deserialized: StoredMemory = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(stored.key, deserialized.key);
        assert_eq!(stored.entry, deserialized.entry);
    }
}
