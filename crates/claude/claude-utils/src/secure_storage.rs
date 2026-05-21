//! Secure storage abstraction for sensitive data.
//!
//! Provides a trait-based abstraction for storing and retrieving sensitive
//! data like API keys and tokens, with a plaintext fallback implementation.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// StorageBackend enum
// ---------------------------------------------------------------------------

/// Available storage backends.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackend {
    /// OS keychain (Keychain on macOS, Credential Manager on Windows, Secret Service on Linux).
    Keychain,
    /// Encrypted file on disk.
    EncryptedFile,
    /// Plaintext file (fallback, not secure).
    PlainText,
    /// In-memory only (for testing).
    Memory,
}

impl StorageBackend {
    /// Return the name of the backend.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::EncryptedFile => "encrypted_file",
            Self::PlainText => "plaintext",
            Self::Memory => "memory",
        }
    }

    /// Check if this backend provides real security.
    #[must_use]
    pub fn is_secure(self) -> bool {
        matches!(self, Self::Keychain | Self::EncryptedFile)
    }
}

impl std::fmt::Display for StorageBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

// ---------------------------------------------------------------------------
// SecureStorage trait
// ---------------------------------------------------------------------------

/// Trait for secure storage operations.
///
/// Implementations must support storing, retrieving, deleting, and listing
/// key-value pairs where values are sensitive data.
pub trait SecureStorage: Send + Sync {
    /// Store a value under the given key.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage operation fails.
    fn store(&self, key: &str, value: &str) -> Result<()>;

    /// Retrieve a value by key.
    ///
    /// # Errors
    ///
    /// Returns an error if the key is not found or retrieval fails.
    fn retrieve(&self, key: &str) -> Result<String>;

    /// Delete a value by key.
    ///
    /// # Errors
    ///
    /// Returns an error if the deletion fails.
    fn delete(&self, key: &str) -> Result<()>;

    /// List all stored keys.
    ///
    /// # Errors
    ///
    /// Returns an error if listing fails.
    fn list(&self) -> Result<Vec<String>>;

    /// Check if a key exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the check fails.
    fn exists(&self, key: &str) -> Result<bool> {
        Ok(self
            .list()
            .map(|keys| keys.iter().any(|k| k == key))
            .unwrap_or(false))
    }

    /// Return the backend type.
    fn backend(&self) -> StorageBackend;
}

// ---------------------------------------------------------------------------
// PlainTextStorage implementation
// ---------------------------------------------------------------------------

/// Plaintext storage backend (fallback, not secure).
///
/// Stores values in memory as a `HashMap`. Useful for testing and
/// environments where no secure storage is available.
pub struct PlainTextStorage {
    data: std::sync::Mutex<HashMap<String, String>>,
}

impl PlainTextStorage {
    /// Create a new empty plaintext storage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            data: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

impl Default for PlainTextStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl SecureStorage for PlainTextStorage {
    fn store(&self, key: &str, value: &str) -> Result<()> {
        let mut data = self
            .data
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {e}"))?;
        data.insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn retrieve(&self, key: &str) -> Result<String> {
        let data = self
            .data
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {e}"))?;
        data.get(key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Key not found: {key}"))
    }

    fn delete(&self, key: &str) -> Result<()> {
        let mut data = self
            .data
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {e}"))?;
        data.remove(key)
            .ok_or_else(|| anyhow::anyhow!("Key not found: {key}"))?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<String>> {
        let data = self
            .data
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {e}"))?;
        Ok(data.keys().cloned().collect())
    }

    fn backend(&self) -> StorageBackend {
        StorageBackend::PlainText
    }
}

// ---------------------------------------------------------------------------
// MemoryStorage implementation (testing only)
// ---------------------------------------------------------------------------

/// In-memory storage for testing purposes.
pub struct MemoryStorage {
    data: std::sync::Mutex<HashMap<String, String>>,
}

impl MemoryStorage {
    /// Create a new empty memory storage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            data: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl SecureStorage for MemoryStorage {
    fn store(&self, key: &str, value: &str) -> Result<()> {
        let mut data = self
            .data
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {e}"))?;
        data.insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn retrieve(&self, key: &str) -> Result<String> {
        let data = self
            .data
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {e}"))?;
        data.get(key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Key not found: {key}"))
    }

    fn delete(&self, key: &str) -> Result<()> {
        let mut data = self
            .data
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {e}"))?;
        data.remove(key)
            .ok_or_else(|| anyhow::anyhow!("Key not found: {key}"))?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<String>> {
        let data = self
            .data
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock poisoned: {e}"))?;
        Ok(data.keys().cloned().collect())
    }

    fn backend(&self) -> StorageBackend {
        StorageBackend::Memory
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- StorageBackend ---

    #[test]
    fn storage_backend_names() {
        assert_eq!(StorageBackend::Keychain.name(), "keychain");
        assert_eq!(StorageBackend::EncryptedFile.name(), "encrypted_file");
        assert_eq!(StorageBackend::PlainText.name(), "plaintext");
        assert_eq!(StorageBackend::Memory.name(), "memory");
    }

    #[test]
    fn storage_backend_is_secure() {
        assert!(StorageBackend::Keychain.is_secure());
        assert!(StorageBackend::EncryptedFile.is_secure());
        assert!(!StorageBackend::PlainText.is_secure());
        assert!(!StorageBackend::Memory.is_secure());
    }

    #[test]
    fn storage_backend_display() {
        assert_eq!(StorageBackend::Keychain.to_string(), "keychain");
    }

    #[test]
    fn storage_backend_serialization_roundtrip() {
        let backend = StorageBackend::EncryptedFile;
        let json = serde_json::to_string(&backend).expect("serialize");
        let deserialized: StorageBackend = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(backend, deserialized);
    }

    // --- PlainTextStorage ---

    #[test]
    fn plaintext_store_and_retrieve() {
        let storage = PlainTextStorage::new();
        storage.store("api_key", "secret123").expect("store");
        let value = storage.retrieve("api_key").expect("retrieve");
        assert_eq!(value, "secret123");
    }

    #[test]
    fn plaintext_retrieve_missing() {
        let storage = PlainTextStorage::new();
        assert!(storage.retrieve("nonexistent").is_err());
    }

    #[test]
    fn plaintext_delete() {
        let storage = PlainTextStorage::new();
        storage.store("key1", "value1").expect("store");
        storage.delete("key1").expect("delete");
        assert!(storage.retrieve("key1").is_err());
    }

    #[test]
    fn plaintext_delete_missing() {
        let storage = PlainTextStorage::new();
        assert!(storage.delete("nonexistent").is_err());
    }

    #[test]
    fn plaintext_list() {
        let storage = PlainTextStorage::new();
        storage.store("key1", "v1").expect("store");
        storage.store("key2", "v2").expect("store");
        let mut keys = storage.list().expect("list");
        keys.sort();
        assert_eq!(keys, vec!["key1", "key2"]);
    }

    #[test]
    fn plaintext_exists() {
        let storage = PlainTextStorage::new();
        storage.store("key1", "v1").expect("store");
        assert!(storage.exists("key1").expect("exists"));
        assert!(!storage.exists("key2").expect("exists"));
    }

    #[test]
    fn plaintext_overwrite() {
        let storage = PlainTextStorage::new();
        storage.store("key1", "v1").expect("store");
        storage.store("key1", "v2").expect("store");
        assert_eq!(storage.retrieve("key1").expect("retrieve"), "v2");
    }

    #[test]
    fn plaintext_backend() {
        let storage = PlainTextStorage::new();
        assert_eq!(storage.backend(), StorageBackend::PlainText);
    }

    // --- MemoryStorage ---

    #[test]
    fn memory_store_and_retrieve() {
        let storage = MemoryStorage::new();
        storage.store("token", "abc").expect("store");
        assert_eq!(storage.retrieve("token").expect("retrieve"), "abc");
    }

    #[test]
    fn memory_delete() {
        let storage = MemoryStorage::new();
        storage.store("key", "val").expect("store");
        storage.delete("key").expect("delete");
        assert!(storage.retrieve("key").is_err());
    }

    #[test]
    fn memory_list() {
        let storage = MemoryStorage::new();
        storage.store("a", "1").expect("store");
        storage.store("b", "2").expect("store");
        assert_eq!(storage.list().expect("list").len(), 2);
    }

    #[test]
    fn memory_backend() {
        let storage = MemoryStorage::new();
        assert_eq!(storage.backend(), StorageBackend::Memory);
    }
}
