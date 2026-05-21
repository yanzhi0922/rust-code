//! Runtime read-file state cache.
//!
//! This mirrors Claude Code's `readFileState`: Read records the exact file
//! content and mtime the model saw; edit/write tools require that state before
//! mutating existing files.

use std::collections::{HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

const DEFAULT_MAX_ENTRIES: usize = 100;
const DEFAULT_MAX_SIZE_BYTES: usize = 25 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileState {
    pub content: String,
    pub timestamp: u128,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub is_partial_view: bool,
}

impl FileState {
    #[must_use]
    pub fn read(content: String, timestamp: u128, offset: usize, limit: Option<usize>) -> Self {
        Self {
            content,
            timestamp,
            offset: Some(offset),
            limit,
            is_partial_view: false,
        }
    }

    #[must_use]
    pub fn post_write(content: String, timestamp: u128) -> Self {
        Self {
            content,
            timestamp,
            offset: None,
            limit: None,
            is_partial_view: false,
        }
    }

    #[must_use]
    pub fn partial(content: String, timestamp: u128, limit: Option<usize>) -> Self {
        Self {
            content,
            timestamp,
            offset: None,
            limit,
            is_partial_view: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileStateCache {
    inner: Arc<Mutex<FileStateCacheInner>>,
}

#[derive(Debug, Clone)]
struct FileStateCacheInner {
    entries: HashMap<String, FileState>,
    lru: VecDeque<String>,
    max_entries: usize,
    max_size_bytes: usize,
    size_bytes: usize,
}

impl Default for FileStateCache {
    fn default() -> Self {
        Self::new()
    }
}

impl FileStateCache {
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_SIZE_BYTES)
    }

    #[must_use]
    pub fn with_limits(max_entries: usize, max_size_bytes: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FileStateCacheInner {
                entries: HashMap::new(),
                lru: VecDeque::new(),
                max_entries,
                max_size_bytes,
                size_bytes: 0,
            })),
        }
    }

    #[must_use]
    pub fn get(&self, path: impl AsRef<Path>) -> Option<FileState> {
        let key = normalize_key(path.as_ref());
        let mut inner = self.lock_inner();
        let state = inner.entries.get(&key).cloned()?;
        inner.touch(&key);
        Some(state)
    }

    pub fn set(&self, path: impl AsRef<Path>, state: FileState) {
        let key = normalize_key(path.as_ref());
        let mut inner = self.lock_inner();
        inner.insert(key, state);
    }

    pub fn delete(&self, path: impl AsRef<Path>) -> bool {
        let key = normalize_key(path.as_ref());
        let mut inner = self.lock_inner();
        inner.remove(&key).is_some()
    }

    pub fn clear(&self) {
        let mut inner = self.lock_inner();
        inner.entries.clear();
        inner.lru.clear();
        inner.size_bytes = 0;
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.lock_inner().entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn calculated_size(&self) -> usize {
        self.lock_inner().size_bytes
    }

    #[must_use]
    pub fn clone_isolated(&self) -> Self {
        let inner = self.lock_inner().clone();
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    fn lock_inner(&self) -> parking_lot::MutexGuard<'_, FileStateCacheInner> {
        self.inner.lock()
    }
}

impl FileStateCacheInner {
    fn insert(&mut self, key: String, state: FileState) {
        if let Some(existing) = self.entries.remove(&key) {
            self.size_bytes = self.size_bytes.saturating_sub(state_size(&existing));
            self.lru.retain(|candidate| candidate != &key);
        }
        self.size_bytes = self.size_bytes.saturating_add(state_size(&state));
        self.entries.insert(key.clone(), state);
        self.lru.push_back(key);
        self.evict_until_within_limits();
    }

    fn remove(&mut self, key: &str) -> Option<FileState> {
        let removed = self.entries.remove(key)?;
        self.size_bytes = self.size_bytes.saturating_sub(state_size(&removed));
        self.lru.retain(|candidate| candidate != key);
        Some(removed)
    }

    fn touch(&mut self, key: &str) {
        self.lru.retain(|candidate| candidate != key);
        self.lru.push_back(key.to_owned());
    }

    fn evict_until_within_limits(&mut self) {
        while self.entries.len() > self.max_entries || self.size_bytes > self.max_size_bytes {
            let Some(oldest) = self.lru.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.size_bytes = self.size_bytes.saturating_sub(state_size(&removed));
            }
        }
    }
}

fn state_size(state: &FileState) -> usize {
    state.content.len().max(1)
}

fn normalize_key(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    let rendered = normalized.to_string_lossy();
    let rendered = rendered.strip_prefix(r"\\?\").unwrap_or(&rendered);
    rendered.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_normalizes_paths_and_clones_isolated() {
        let cache = FileStateCache::with_limits(100, 1024);
        cache.set(
            Path::new("a").join(".").join("b.txt"),
            FileState::read("one".to_owned(), 10, 1, None),
        );
        assert_eq!(
            cache
                .get(Path::new("a").join("b.txt"))
                .expect("state")
                .content,
            "one"
        );

        let clone = cache.clone_isolated();
        clone.set(
            Path::new("a").join("b.txt"),
            FileState::post_write("two".to_owned(), 11),
        );

        assert_eq!(
            cache
                .get(Path::new("a").join("b.txt"))
                .expect("original cache should have entry")
                .content,
            "one"
        );
        assert_eq!(
            clone
                .get(Path::new("a").join("b.txt"))
                .expect("cloned cache should have entry")
                .content,
            "two"
        );
    }

    #[test]
    fn cache_evicts_by_size_and_entry_count() {
        let cache = FileStateCache::with_limits(2, 8);
        cache.set("a.txt", FileState::read("aaaa".to_owned(), 1, 1, None));
        cache.set("b.txt", FileState::read("bbbb".to_owned(), 1, 1, None));
        cache.set("c.txt", FileState::read("cccc".to_owned(), 1, 1, None));

        assert!(cache.get("a.txt").is_none());
        assert!(cache.get("b.txt").is_some());
        assert!(cache.get("c.txt").is_some());

        cache.set("d.txt", FileState::read("ddddddddd".to_owned(), 1, 1, None));
        assert!(cache.calculated_size() <= 8 || cache.len() <= 1);
    }
}
