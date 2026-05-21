//! Secure storage module.
//!
//! Provides a cross-platform trait for storing secrets (OAuth tokens, API keys)
//! in platform-native credential stores:
//!
//! - **macOS**: Keychain Services
//! - **Windows**: Credential Manager
//! - **Linux**: Secret Service (libsecret / GNOME Keyring / KDE Wallet)

pub mod keychain;

use async_trait::async_trait;

/// Errors from secure storage operations.
#[derive(Debug, thiserror::Error)]
pub enum SecureStorageError {
    #[error("secure storage not available: {0}")]
    NotAvailable(String),

    #[error("item not found: {0}")]
    NotFound(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("platform error: {0}")]
    Platform(String),
}

/// Trait for secure credential storage.
///
/// Implementations use platform-native credential stores. A mock
/// implementation is available for testing.
#[async_trait]
pub trait SecureStorage: Send + Sync {
    /// Store a secret value under the given key.
    async fn save(
        &self,
        service: &str,
        account: &str,
        secret: &str,
    ) -> Result<(), SecureStorageError>;

    /// Retrieve a secret value.
    async fn load(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<String>, SecureStorageError>;

    /// Delete a stored secret.
    async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError>;
}

/// Return the platform-appropriate secure storage implementation.
pub fn platform_secure_storage() -> Result<Box<dyn SecureStorage>, SecureStorageError> {
    keychain::PlatformKeychain::new().map(|k| Box::new(k) as Box<dyn SecureStorage>)
}

/// An in-memory secure storage for use in tests.
pub struct MockSecureStorage {
    store: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl MockSecureStorage {
    /// Create a new empty mock storage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            store: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for MockSecureStorage {
    fn default() -> Self {
        Self::new()
    }
}

fn storage_key(service: &str, account: &str) -> String {
    format!("{service}:{account}")
}

#[async_trait]
impl SecureStorage for MockSecureStorage {
    async fn save(
        &self,
        service: &str,
        account: &str,
        secret: &str,
    ) -> Result<(), SecureStorageError> {
        let key = storage_key(service, account);
        self.store
            .lock()
            .expect("mock storage lock")
            .insert(key, secret.to_owned());
        Ok(())
    }

    async fn load(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<String>, SecureStorageError> {
        let key = storage_key(service, account);
        Ok(self
            .store
            .lock()
            .expect("mock storage lock")
            .get(&key)
            .cloned())
    }

    async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError> {
        let key = storage_key(service, account);
        self.store.lock().expect("mock storage lock").remove(&key);
        Ok(())
    }
}
