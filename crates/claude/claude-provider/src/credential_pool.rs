//! Credential pool for rotating API keys across subtask executions.
//!
//! [`CredentialPool`] distributes LLM API requests across multiple credentials
//! using round-robin rotation. This helps avoid rate limits when running
//! parallel sub-agents, inspired by hermes-agent's credential pool design.

use std::sync::atomic::{AtomicUsize, Ordering};

/// A single credential entry in the pool.
#[derive(Debug, Clone)]
pub struct CredentialEntry {
    /// API key or bearer token.
    pub api_key: String,
    /// Optional model override for this credential.
    pub model: Option<String>,
}

impl CredentialEntry {
    /// Create a new credential entry with just an API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: None,
        }
    }

    /// Create a credential entry with a model override.
    pub fn with_model(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: Some(model.into()),
        }
    }
}

/// Round-robin credential pool for distributing requests across multiple keys.
///
/// # Thread Safety
///
/// The rotation index uses `AtomicUsize`, so `next()` is safe to call from
/// multiple threads without external synchronization.
///
/// # Example
///
/// ```
/// use claude_provider::credential_pool::CredentialPool;
///
/// let pool = CredentialPool::from_keys(vec!["key1".into(), "key2".into()]);
/// assert_eq!(pool.len(), 2);
///
/// let first = pool.next().unwrap();
/// let second = pool.next().unwrap();
/// let wraps_around = pool.next().unwrap();
/// assert_eq!(first.api_key, "key1");
/// assert_eq!(second.api_key, "key2");
/// assert_eq!(wraps_around.api_key, "key1"); // round-robin
/// ```
pub struct CredentialPool {
    entries: Vec<CredentialEntry>,
    index: AtomicUsize,
}

impl CredentialPool {
    /// Create a pool from a list of credential entries.
    pub fn new(entries: Vec<CredentialEntry>) -> Self {
        let index = AtomicUsize::new(0);
        Self { entries, index }
    }

    /// Create a pool from a list of API key strings.
    pub fn from_keys(keys: Vec<String>) -> Self {
        Self::new(keys.into_iter().map(CredentialEntry::new).collect())
    }

    /// Create a single-entry pool (no rotation).
    pub fn single(api_key: impl Into<String>) -> Self {
        Self::new(vec![CredentialEntry::new(api_key)])
    }

    /// Get the next credential using round-robin rotation.
    ///
    /// Returns `None` if the pool is empty.
    pub fn next(&self) -> Option<&CredentialEntry> {
        if self.entries.is_empty() {
            return None;
        }
        let idx = self.index.fetch_add(1, Ordering::Relaxed) % self.entries.len();
        Some(&self.entries[idx])
    }

    /// Number of credentials in the pool.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if pool is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get a credential by index.
    pub fn get(&self, index: usize) -> Option<&CredentialEntry> {
        self.entries.get(index)
    }

    /// Reset the round-robin index to zero.
    pub fn reset(&self) {
        self.index.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn credential_pool_single_key() {
        let pool = CredentialPool::single("my-key");
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        let cred = pool.next().unwrap();
        assert_eq!(cred.api_key, "my-key");
        assert!(cred.model.is_none());
    }

    #[test]
    fn credential_pool_round_robin() {
        let pool = CredentialPool::from_keys(vec!["key-a".into(), "key-b".into(), "key-c".into()]);

        assert_eq!(pool.next().unwrap().api_key, "key-a");
        assert_eq!(pool.next().unwrap().api_key, "key-b");
        assert_eq!(pool.next().unwrap().api_key, "key-c");
        // Wraps around.
        assert_eq!(pool.next().unwrap().api_key, "key-a");
    }

    #[test]
    fn credential_pool_empty() {
        let pool = CredentialPool::new(vec![]);
        assert!(pool.is_empty());
        assert!(pool.next().is_none());
    }

    #[test]
    fn credential_pool_reset() {
        let pool = CredentialPool::from_keys(vec!["k1".into(), "k2".into()]);

        assert_eq!(pool.next().unwrap().api_key, "k1");
        assert_eq!(pool.next().unwrap().api_key, "k2");

        pool.reset();
        assert_eq!(pool.next().unwrap().api_key, "k1");
    }

    #[test]
    fn credential_entry_with_model() {
        let entry = CredentialEntry::with_model("key", "gpt-4o-mini");
        assert_eq!(entry.api_key, "key");
        assert_eq!(entry.model.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn credential_pool_get_by_index() {
        let pool = CredentialPool::from_keys(vec!["a".into(), "b".into()]);
        assert_eq!(pool.get(0).unwrap().api_key, "a");
        assert_eq!(pool.get(1).unwrap().api_key, "b");
        assert!(pool.get(2).is_none());
    }
}
