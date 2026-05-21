//! Authentication state cache with file persistence and TTL.
//!
//! Caches the "needs-auth" state for MCP servers so that the connection
//! manager can skip servers that recently required authentication, avoiding
//! unnecessary connection attempts. The cache is persisted to a JSON file
//! and entries expire after a configurable TTL (default: 15 minutes).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Default TTL for auth cache entries (15 minutes).
const DEFAULT_TTL_SECS: u64 = 15 * 60;
/// Default cache file name.
const DEFAULT_CACHE_FILE: &str = "mcp-needs-auth-cache.json";

/// Authentication state cache.
///
/// Tracks which servers recently required authentication. Entries older
/// than the TTL are considered expired and will not prevent a reconnect.
#[derive(Debug)]
pub struct McpAuthCache {
    /// Directory where the cache file is stored.
    cache_path: PathBuf,
    /// Time-to-live for cache entries.
    ttl: Duration,
    /// In-memory cache: server name → Unix timestamp (ms) when marked.
    data: HashMap<String, i64>,
}

/// Serializable representation of the auth cache.
#[derive(Debug, Serialize, Deserialize, Default)]
struct AuthCacheData {
    /// Server name → timestamp (milliseconds since epoch).
    entries: HashMap<String, i64>,
}

impl McpAuthCache {
    /// Create a new auth cache rooted at the given directory.
    ///
    /// The cache file will be stored at `{cache_dir}/mcp-needs-auth-cache.json`.
    pub fn new(cache_dir: impl AsRef<Path>) -> Self {
        Self {
            cache_path: cache_dir.as_ref().join(DEFAULT_CACHE_FILE),
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
            data: HashMap::new(),
        }
    }

    /// Create a new auth cache with a custom TTL.
    pub fn with_ttl(cache_dir: impl AsRef<Path>, ttl: Duration) -> Self {
        Self {
            cache_path: cache_dir.as_ref().join(DEFAULT_CACHE_FILE),
            ttl,
            data: HashMap::new(),
        }
    }

    /// Check whether a server is in the auth cache and the entry has not expired.
    ///
    /// Returns `true` if the server was recently marked as needing auth and
    /// the entry is still within the TTL window.
    pub fn is_cached(&self, server_name: &str) -> bool {
        let Some(&timestamp_ms) = self.data.get(server_name) else {
            return false;
        };
        let now_ms = now_millis();
        let elapsed = Duration::from_millis((now_ms - timestamp_ms).unsigned_abs());
        elapsed < self.ttl
    }

    /// Mark a server as needing authentication (current timestamp).
    pub fn mark_needs_auth(&mut self, server_name: &str) {
        let now_ms = now_millis();
        self.data.insert(server_name.to_owned(), now_ms);
    }

    /// Clear the auth cache entry for a specific server.
    pub fn clear_server(&mut self, server_name: &str) {
        self.data.remove(server_name);
    }

    /// Clear all auth cache entries.
    pub fn clear_all(&mut self) {
        self.data.clear();
    }

    /// Load the cache from the on-disk file.
    ///
    /// If the file does not exist, the cache remains empty.
    pub async fn load(&mut self) -> Result<(), std::io::Error> {
        let content = match tokio::fs::read_to_string(&self.cache_path).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let parsed: AuthCacheData = serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        self.data = parsed.entries;
        Ok(())
    }

    /// Save the cache to the on-disk file.
    ///
    /// Creates parent directories if they do not exist.
    pub async fn save(&self) -> Result<(), std::io::Error> {
        if let Some(parent) = self.cache_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let data = AuthCacheData {
            entries: self.data.clone(),
        };
        let content = serde_json::to_string_pretty(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        tokio::fs::write(&self.cache_path, content).await
    }

    /// Return the number of entries in the cache (including possibly expired ones).
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Return `true` if the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Return the cache file path.
    #[must_use]
    pub fn cache_path(&self) -> &Path {
        &self.cache_path
    }

    /// Return the configured TTL.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        self.ttl
    }
}

/// Return the current time as milliseconds since the Unix epoch.
fn now_millis() -> i64 {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(duration.as_millis()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_cache_is_empty() {
        let cache = McpAuthCache::new("/tmp/test-cache");
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn mark_and_check_cached() {
        let mut cache = McpAuthCache::new("/tmp/test-cache");
        cache.mark_needs_auth("my-server");
        assert!(cache.is_cached("my-server"));
        assert!(!cache.is_cached("other-server"));
    }

    #[test]
    fn clear_server_removes_entry() {
        let mut cache = McpAuthCache::new("/tmp/test-cache");
        cache.mark_needs_auth("srv-a");
        cache.mark_needs_auth("srv-b");
        cache.clear_server("srv-a");
        assert!(!cache.is_cached("srv-a"));
        assert!(cache.is_cached("srv-b"));
    }

    #[test]
    fn clear_all_removes_everything() {
        let mut cache = McpAuthCache::new("/tmp/test-cache");
        cache.mark_needs_auth("srv-a");
        cache.mark_needs_auth("srv-b");
        cache.clear_all();
        assert!(cache.is_empty());
        assert!(!cache.is_cached("srv-a"));
        assert!(!cache.is_cached("srv-b"));
    }

    #[test]
    fn expired_entry_is_not_cached() {
        // TTL of 0 means everything is immediately expired.
        let mut cache = McpAuthCache::with_ttl("/tmp/test-cache", Duration::from_secs(0));
        cache.mark_needs_auth("srv");
        // The entry was just created, but with TTL=0 it should be expired.
        // (Due to timing, this might be flaky with exactly 0, so use a very small TTL)
        // Actually with TTL=0, elapsed >= TTL is always true.
        assert!(!cache.is_cached("srv"));
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cache = McpAuthCache::new(dir.path());
        cache.mark_needs_auth("server-1");
        cache.mark_needs_auth("server-2");
        cache.save().await.expect("save");

        let mut loaded = McpAuthCache::new(dir.path());
        loaded.load().await.expect("load");
        assert!(loaded.is_cached("server-1"));
        assert!(loaded.is_cached("server-2"));
        assert!(!loaded.is_cached("server-3"));
    }

    #[tokio::test]
    async fn load_missing_file_is_ok() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cache = McpAuthCache::new(dir.path().join("nonexistent"));
        cache
            .load()
            .await
            .expect("load should succeed on missing file");
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_path_includes_filename() {
        let cache = McpAuthCache::new("/tmp/my-dir");
        assert!(cache.cache_path().ends_with(DEFAULT_CACHE_FILE));
    }

    #[test]
    fn default_ttl_is_15_minutes() {
        let cache = McpAuthCache::new("/tmp/test");
        assert_eq!(cache.ttl(), Duration::from_secs(15 * 60));
    }
}
