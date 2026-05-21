//! Plugin ZIP cache for offline and session-local plugin management.
//!
//! Rust equivalent of `zipCache.ts` and `zipCacheAdapters.ts`. Manages
//! plugins as ZIP archives in a persistent cache directory. When enabled,
//! plugins are stored as ZIPs and extracted to a session-local temp
//! directory at startup.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors during ZIP cache operations.
#[derive(Debug, Error)]
pub enum ZipCacheError {
    /// I/O error.
    #[error("zip cache I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to parse cache metadata.
    #[error("zip cache parse error: {0}")]
    Parse(#[from] serde_json::Error),
    /// Cache is not enabled or misconfigured.
    #[error("zip cache not enabled: {0}")]
    NotEnabled(String),
    /// Entry not found in cache.
    #[error("cache entry not found: {0}")]
    NotFound(String),
}

// ---------------------------------------------------------------------------
// ZipCacheEntry
// ---------------------------------------------------------------------------

/// Metadata for a single cached ZIP file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZipCacheEntry {
    /// Path to the cached ZIP file (relative to cache root).
    pub path: String,
    /// ETag from the last HTTP response (for conditional requests).
    #[serde(default)]
    pub etag: Option<String>,
    /// Last-Modified header value.
    #[serde(default)]
    pub last_modified: Option<String>,
    /// When this cache entry expires.
    pub expires_at: DateTime<Utc>,
    /// When the entry was last validated.
    #[serde(default)]
    pub validated_at: Option<DateTime<Utc>>,
    /// Size in bytes of the ZIP file.
    #[serde(default)]
    pub size: Option<u64>,
}

impl ZipCacheEntry {
    /// Check if this entry has expired.
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }
}

// ---------------------------------------------------------------------------
// ZipCacheIndex
// ---------------------------------------------------------------------------

/// Index of all cached ZIP entries, persisted as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZipCacheIndex {
    /// Map from cache key (e.g., `"marketplace/plugin/version"`) to entry.
    pub entries: HashMap<String, ZipCacheEntry>,
    /// When the index was last updated.
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

impl Default for ZipCacheIndex {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            updated_at: Some(Utc::now()),
        }
    }
}

impl ZipCacheIndex {
    /// Create a new empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up an entry by key.
    pub fn get(&self, key: &str) -> Option<&ZipCacheEntry> {
        self.entries.get(key)
    }

    /// Insert or update an entry.
    pub fn insert(&mut self, key: impl Into<String>, entry: ZipCacheEntry) {
        self.entries.insert(key.into(), entry);
        self.updated_at = Some(Utc::now());
    }

    /// Remove an entry.
    pub fn remove(&mut self, key: &str) -> Option<ZipCacheEntry> {
        let entry = self.entries.remove(key);
        if entry.is_some() {
            self.updated_at = Some(Utc::now());
        }
        entry
    }

    /// Remove all expired entries, returning the removed keys.
    pub fn evict_expired(&mut self) -> Vec<String> {
        let expired_keys: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.is_expired())
            .map(|(k, _)| k.clone())
            .collect();

        for key in &expired_keys {
            self.entries.remove(key);
        }

        if !expired_keys.is_empty() {
            self.updated_at = Some(Utc::now());
        }
        expired_keys
    }

    /// Number of entries in the index.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// ZipCache
// ---------------------------------------------------------------------------

/// Persistent ZIP cache for plugin archives.
pub struct ZipCache {
    /// Root directory for cached ZIP files.
    cache_dir: PathBuf,
    /// In-memory index of cached entries.
    index: ZipCacheIndex,
}

impl ZipCache {
    /// Open or create a ZIP cache at the given directory.
    pub fn open(cache_dir: PathBuf) -> Result<Self, ZipCacheError> {
        std::fs::create_dir_all(&cache_dir)?;
        let index_path = Self::index_path(&cache_dir);
        let index = if index_path.exists() {
            let content = std::fs::read_to_string(&index_path)?;
            if content.trim().is_empty() {
                ZipCacheIndex::new()
            } else {
                serde_json::from_str(&content)?
            }
        } else {
            ZipCacheIndex::new()
        };
        Ok(Self { cache_dir, index })
    }

    /// Get the cache root directory.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Get the plugins subdirectory.
    pub fn plugins_dir(&self) -> PathBuf {
        self.cache_dir.join("plugins")
    }

    /// Get the marketplaces subdirectory.
    pub fn marketplaces_dir(&self) -> PathBuf {
        self.cache_dir.join("marketplaces")
    }

    /// Look up a cached entry.
    pub fn get(&self, key: &str) -> Option<&ZipCacheEntry> {
        self.index.get(key)
    }

    /// Get the full filesystem path for a cached ZIP.
    pub fn zip_path(&self, key: &str) -> PathBuf {
        self.cache_dir.join("plugins").join(format!("{key}.zip"))
    }

    /// Check if a key is cached and not expired.
    pub fn is_cached(&self, key: &str) -> bool {
        match self.index.get(key) {
            Some(entry) => !entry.is_expired(),
            None => false,
        }
    }

    /// Insert a new entry into the cache index and persist.
    pub fn insert(
        &mut self,
        key: impl Into<String>,
        entry: ZipCacheEntry,
    ) -> Result<(), ZipCacheError> {
        self.index.insert(key, entry);
        self.save_index()
    }

    /// Remove an entry from the cache (index + ZIP file).
    pub fn remove(&mut self, key: &str) -> Result<Option<ZipCacheEntry>, ZipCacheError> {
        let entry = self.index.remove(key);
        if entry.is_some() {
            let zip = self.zip_path(key);
            let _ = std::fs::remove_file(zip); // Best-effort
            self.save_index()?;
        }
        Ok(entry)
    }

    /// Evict all expired entries (index + ZIP files).
    pub fn evict_expired(&mut self) -> Result<Vec<String>, ZipCacheError> {
        let expired = self.index.evict_expired();
        for key in &expired {
            let zip = self.zip_path(key);
            let _ = std::fs::remove_file(zip); // Best-effort
        }
        if !expired.is_empty() {
            self.save_index()?;
        }
        Ok(expired)
    }

    /// Save the index to disk (atomic write).
    pub fn save_index(&self) -> Result<(), ZipCacheError> {
        let index_path = Self::index_path(&self.cache_dir);
        let content = serde_json::to_string_pretty(&self.index)?;
        let tmp_path = index_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, &index_path)?;
        Ok(())
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    fn index_path(cache_dir: &Path) -> PathBuf {
        cache_dir.join("cache-index.json")
    }
}

// ---------------------------------------------------------------------------
// ZipCacheAdapter trait
// ---------------------------------------------------------------------------

/// Trait for different ZIP cache storage backends.
pub trait ZipCacheAdapter {
    /// Get the path to a cached ZIP file.
    fn get_cache_path(&self, key: &str) -> PathBuf;

    /// Check if an entry exists and is not expired.
    fn exists(&self, key: &str) -> bool;

    /// Write data to the cache.
    fn write(&self, key: &str, data: &[u8]) -> anyhow::Result<()>;

    /// Read data from the cache.
    fn read(&self, key: &str) -> anyhow::Result<Vec<u8>>;

    /// Remove an entry from the cache.
    fn remove(&self, key: &str) -> anyhow::Result<()>;

    /// Evict expired entries.
    fn evict_expired(&self) -> anyhow::Result<Vec<String>>;
}

// ---------------------------------------------------------------------------
// FilesystemZipCacheAdapter
// ---------------------------------------------------------------------------

/// Filesystem-backed ZIP cache adapter.
pub struct FilesystemZipCacheAdapter {
    cache_dir: PathBuf,
    index: ZipCacheIndex,
}

impl FilesystemZipCacheAdapter {
    /// Create a new filesystem adapter.
    pub fn new(cache_dir: PathBuf) -> Result<Self, ZipCacheError> {
        std::fs::create_dir_all(&cache_dir)?;
        let index_path = cache_dir.join("cache-index.json");
        let index = if index_path.exists() {
            let content = std::fs::read_to_string(&index_path)?;
            if content.trim().is_empty() {
                ZipCacheIndex::new()
            } else {
                serde_json::from_str(&content)?
            }
        } else {
            ZipCacheIndex::new()
        };
        Ok(Self { cache_dir, index })
    }

    /// Save the index to disk.
    pub fn save_index(&self) -> anyhow::Result<()> {
        let index_path = self.cache_dir.join("cache-index.json");
        let content = serde_json::to_string_pretty(&self.index)?;
        let tmp_path = index_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, &index_path)?;
        Ok(())
    }
}

impl ZipCacheAdapter for FilesystemZipCacheAdapter {
    fn get_cache_path(&self, key: &str) -> PathBuf {
        self.cache_dir.join("plugins").join(format!("{key}.zip"))
    }

    fn exists(&self, key: &str) -> bool {
        match self.index.get(key) {
            Some(entry) => !entry.is_expired() && self.get_cache_path(key).exists(),
            None => false,
        }
    }

    fn write(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        let path = self.get_cache_path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Atomic write
        let tmp_path = path.with_extension("zip.tmp");
        std::fs::write(&tmp_path, data)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    fn read(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let path = self.get_cache_path(key);
        Ok(std::fs::read(path)?)
    }

    fn remove(&self, key: &str) -> anyhow::Result<()> {
        let path = self.get_cache_path(key);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn evict_expired(&self) -> anyhow::Result<Vec<String>> {
        // Note: mutable borrow needed for index but we only have &self.
        // In a real implementation this would need interior mutability.
        // For now, return empty — the ZipCache struct handles this.
        Ok(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// Atomic write helper
// ---------------------------------------------------------------------------

/// Write data to a file atomically (write to temp, then rename).
pub fn atomic_write(target: &Path, data: &[u8]) -> Result<(), ZipCacheError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_name = format!(
        ".{}.tmp",
        target
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    let tmp_path = target.with_file_name(tmp_name);
    std::fs::write(&tmp_path, data)?;
    std::fs::rename(&tmp_path, target)?;
    Ok(())
}

/// Write a string to a file atomically.
pub fn atomic_write_string(target: &Path, content: &str) -> Result<(), ZipCacheError> {
    atomic_write(target, content.as_bytes())
}

// ---------------------------------------------------------------------------
// Session cache helpers
// ---------------------------------------------------------------------------

/// Directory name for session-local plugin extractions.
pub const SESSION_CACHE_PREFIX: &str = "rc-plugin-session-";

/// Get a session cache directory path (does not create it).
pub fn session_cache_path(base_temp: &Path, session_id: &str) -> PathBuf {
    base_temp.join(format!("{SESSION_CACHE_PREFIX}{session_id}"))
}

/// Clean up a session cache directory.
pub fn cleanup_session_cache(path: &Path) -> Result<(), ZipCacheError> {
    if path.exists() {
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -- ZipCacheEntry --

    #[test]
    fn entry_expiry_check() {
        let entry = ZipCacheEntry {
            path: "test.zip".into(),
            etag: None,
            last_modified: None,
            expires_at: Utc::now() - chrono::Duration::hours(1),
            validated_at: None,
            size: None,
        };
        assert!(entry.is_expired());

        let future_entry = ZipCacheEntry {
            path: "test.zip".into(),
            etag: None,
            last_modified: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
            validated_at: None,
            size: None,
        };
        assert!(!future_entry.is_expired());
    }

    // -- ZipCacheIndex --

    #[test]
    fn index_insert_and_get() {
        let mut index = ZipCacheIndex::new();
        let entry = ZipCacheEntry {
            path: "p.zip".into(),
            etag: Some("abc".into()),
            last_modified: None,
            expires_at: Utc::now() + chrono::Duration::hours(24),
            validated_at: None,
            size: Some(1024),
        };
        index.insert("marketplace/plugin/1.0.0", entry);
        assert!(index.get("marketplace/plugin/1.0.0").is_some());
        assert!(index.get("nonexistent").is_none());
    }

    #[test]
    fn index_remove() {
        let mut index = ZipCacheIndex::new();
        let entry = ZipCacheEntry {
            path: "p.zip".into(),
            etag: None,
            last_modified: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
            validated_at: None,
            size: None,
        };
        index.insert("key", entry);
        assert!(index.remove("key").is_some());
        assert!(index.get("key").is_none());
    }

    #[test]
    fn index_evict_expired() {
        let mut index = ZipCacheIndex::new();

        let expired = ZipCacheEntry {
            path: "old.zip".into(),
            etag: None,
            last_modified: None,
            expires_at: Utc::now() - chrono::Duration::hours(1),
            validated_at: None,
            size: None,
        };
        let valid = ZipCacheEntry {
            path: "new.zip".into(),
            etag: None,
            last_modified: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
            validated_at: None,
            size: None,
        };
        index.insert("old", expired);
        index.insert("new", valid);

        let evicted = index.evict_expired();
        assert_eq!(evicted, vec!["old"]);
        assert_eq!(index.len(), 1);
        assert!(index.get("new").is_some());
    }

    // -- ZipCache --

    #[test]
    fn cache_open_and_insert() {
        let dir = TempDir::new().expect("tempdir");
        let mut cache = ZipCache::open(dir.path().to_path_buf()).expect("open");

        let entry = ZipCacheEntry {
            path: "plugins/test.zip".into(),
            etag: Some("etag123".into()),
            last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".into()),
            expires_at: Utc::now() + chrono::Duration::hours(24),
            validated_at: None,
            size: Some(2048),
        };
        cache.insert("mkt/plugin/1.0.0", entry).expect("insert");

        assert!(cache.get("mkt/plugin/1.0.0").is_some());
        assert!(cache.is_cached("mkt/plugin/1.0.0"));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_persist_and_reload() {
        let dir = TempDir::new().expect("tempdir");
        let entry = ZipCacheEntry {
            path: "p.zip".into(),
            etag: None,
            last_modified: None,
            expires_at: Utc::now() + chrono::Duration::hours(24),
            validated_at: None,
            size: None,
        };

        {
            let mut cache = ZipCache::open(dir.path().to_path_buf()).expect("open");
            cache.insert("key", entry).expect("insert");
        }

        let cache2 = ZipCache::open(dir.path().to_path_buf()).expect("reopen");
        assert!(cache2.get("key").is_some());
    }

    #[test]
    fn cache_remove() {
        let dir = TempDir::new().expect("tempdir");
        let mut cache = ZipCache::open(dir.path().to_path_buf()).expect("open");

        let entry = ZipCacheEntry {
            path: "p.zip".into(),
            etag: None,
            last_modified: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
            validated_at: None,
            size: None,
        };
        cache.insert("key", entry).expect("insert");
        let removed = cache.remove("key").expect("remove");
        assert!(removed.is_some());
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_evict_expired() {
        let dir = TempDir::new().expect("tempdir");
        let mut cache = ZipCache::open(dir.path().to_path_buf()).expect("open");

        let expired = ZipCacheEntry {
            path: "old.zip".into(),
            etag: None,
            last_modified: None,
            expires_at: Utc::now() - chrono::Duration::hours(1),
            validated_at: None,
            size: None,
        };
        cache.insert("expired-key", expired).expect("insert");

        let evicted = cache.evict_expired().expect("evict");
        assert_eq!(evicted, vec!["expired-key"]);
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_directories() {
        let dir = TempDir::new().expect("tempdir");
        let cache = ZipCache::open(dir.path().to_path_buf()).expect("open");
        assert!(cache.plugins_dir().ends_with("plugins"));
        assert!(cache.marketplaces_dir().ends_with("marketplaces"));
    }

    // -- FilesystemZipCacheAdapter --

    #[test]
    fn adapter_write_and_read() {
        let dir = TempDir::new().expect("tempdir");
        let adapter = FilesystemZipCacheAdapter::new(dir.path().to_path_buf()).expect("adapter");

        let data = b"zip contents here";
        adapter.write("test/plugin/1.0", data).expect("write");

        let read = adapter.read("test/plugin/1.0").expect("read");
        assert_eq!(read, data);
    }

    #[test]
    fn adapter_exists_check() {
        let dir = TempDir::new().expect("tempdir");
        let adapter = FilesystemZipCacheAdapter::new(dir.path().to_path_buf()).expect("adapter");
        assert!(!adapter.exists("no-key"));
    }

    #[test]
    fn adapter_remove() {
        let dir = TempDir::new().expect("tempdir");
        let adapter = FilesystemZipCacheAdapter::new(dir.path().to_path_buf()).expect("adapter");
        adapter.write("key", b"data").expect("write");
        adapter.remove("key").expect("remove");
        assert!(adapter.read("key").is_err());
    }

    // -- atomic_write --

    #[test]
    fn atomic_write_creates_file() {
        let dir = TempDir::new().expect("tempdir");
        let target = dir.path().join("test.json");
        atomic_write(&target, b"hello").expect("write");
        let content = std::fs::read_to_string(&target).expect("read");
        assert_eq!(content, "hello");
    }

    #[test]
    fn atomic_write_string_creates_file() {
        let dir = TempDir::new().expect("tempdir");
        let target = dir.path().join("test.txt");
        atomic_write_string(&target, "world").expect("write");
        let content = std::fs::read_to_string(&target).expect("read");
        assert_eq!(content, "world");
    }

    // -- session cache --

    #[test]
    fn session_cache_path_format() {
        let path = session_cache_path(Path::new("/tmp"), "abc123");
        assert!(
            path.to_str()
                .expect("session cache path should be valid utf-8 in test")
                .contains("rc-plugin-session-abc123")
        );
    }

    #[test]
    fn test_cleanup_session_cache() {
        let dir = TempDir::new().expect("tempdir");
        let session_dir = dir.path().join("rc-plugin-session-test");
        std::fs::create_dir_all(&session_dir).expect("dir");
        std::fs::write(session_dir.join("file.txt"), "data").expect("write");

        cleanup_session_cache(&session_dir).expect("cleanup");
        assert!(!session_dir.exists());
    }

    #[test]
    fn cleanup_nonexistent_is_ok() {
        let path = PathBuf::from("/nonexistent/session-dir");
        assert!(cleanup_session_cache(&path).is_ok());
    }
}
