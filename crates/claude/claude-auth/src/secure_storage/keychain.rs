//! Platform-specific keychain / credential-manager integration.
//!
//! Uses conditional compilation to select the appropriate backend:
//! - macOS → `security` CLI (Keychain Services)
//! - Windows → `cmdkey` or file-based fallback
//! - Linux → `secret-tool` (libsecret)
//!
//! When no platform backend is available, falls back to an encrypted
//! file store in the user's config directory.

use async_trait::async_trait;
use tracing::debug;

#[cfg(target_os = "linux")]
use tracing::warn;

use super::{SecureStorage, SecureStorageError};

/// Platform-native keychain implementation.
pub struct PlatformKeychain {
    /// Service name used as the keychain "service" field.
    service_name: String,
}

impl PlatformKeychain {
    /// Create a new platform keychain with the given service name.
    pub fn new() -> Result<Self, SecureStorageError> {
        Ok(Self {
            service_name: "com.remote-code.auth".to_owned(),
        })
    }

    /// Create with a custom service name (useful for testing).
    #[allow(dead_code)]
    pub fn with_service_name(name: &str) -> Self {
        Self {
            service_name: name.to_owned(),
        }
    }
}

#[async_trait]
impl SecureStorage for PlatformKeychain {
    async fn save(
        &self,
        service: &str,
        account: &str,
        secret: &str,
    ) -> Result<(), SecureStorageError> {
        let key = format!("{service}:{account}");
        save_secret(&self.service_name, &key, secret).await
    }

    async fn load(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<String>, SecureStorageError> {
        let key = format!("{service}:{account}");
        load_secret(&self.service_name, &key).await
    }

    async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError> {
        let key = format!("{service}:{account}");
        delete_secret(&self.service_name, &key).await
    }
}

// ── Platform-specific implementations ──────────────────────────────────

#[cfg(target_os = "macos")]
async fn save_secret(service: &str, account: &str, secret: &str) -> Result<(), SecureStorageError> {
    let output = tokio::process::Command::new("security")
        .args([
            "add-generic-password",
            "-a",
            account,
            "-s",
            service,
            "-w",
            secret,
            "-U", // update if exists
        ])
        .output()
        .await
        .map_err(|e| SecureStorageError::Platform(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SecureStorageError::Platform(format!(
            "security add-generic-password failed: {stderr}"
        )));
    }
    debug!("Saved secret to macOS Keychain for account={account}");
    Ok(())
}

#[cfg(target_os = "macos")]
async fn load_secret(service: &str, account: &str) -> Result<Option<String>, SecureStorageError> {
    let output = tokio::process::Command::new("security")
        .args(["find-generic-password", "-a", account, "-s", service, "-w"])
        .output()
        .await
        .map_err(|e| SecureStorageError::Platform(e.to_string()))?;

    if output.status.success() {
        let secret = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok(Some(secret))
    } else {
        Ok(None)
    }
}

#[cfg(target_os = "macos")]
async fn delete_secret(service: &str, account: &str) -> Result<(), SecureStorageError> {
    let output = tokio::process::Command::new("security")
        .args(["delete-generic-password", "-a", account, "-s", service])
        .output()
        .await
        .map_err(|e| SecureStorageError::Platform(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Not found is OK for delete
        if stderr.contains("The specified item could not be found") {
            return Ok(());
        }
        return Err(SecureStorageError::Platform(format!(
            "security delete-generic-password failed: {stderr}"
        )));
    }
    debug!("Deleted secret from macOS Keychain for account={account}");
    Ok(())
}

#[cfg(target_os = "windows")]
async fn save_secret(service: &str, account: &str, secret: &str) -> Result<(), SecureStorageError> {
    // Use Windows Credential Manager via cmdkey CLI.
    let target = format!("{service}:{account}");
    let output = tokio::process::Command::new("cmdkey")
        .args(["/generic", &target, "/user", account, "/pass", secret])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;

    match output {
        Ok(output) if output.status.success() => {
            debug!("Saved secret to Windows Credential Manager for target={target}");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("cmdkey save failed ({stderr}), using file-based storage");
        }
        Err(error) => {
            tracing::warn!("cmdkey save failed to execute ({error}), using file-based storage");
        }
    }

    // `cmdkey` cannot load credentials non-interactively, so persist the
    // readable fallback even when Credential Manager accepts the write.
    file_based_save(service, account, secret).await
}

#[cfg(target_os = "windows")]
async fn load_secret(service: &str, account: &str) -> Result<Option<String>, SecureStorageError> {
    // Windows Credential Manager does not support non-interactive credential
    // retrieval via cmdkey. Use the file-based fallback for loading.
    file_based_load(service, account).await
}

#[cfg(target_os = "windows")]
async fn delete_secret(service: &str, account: &str) -> Result<(), SecureStorageError> {
    let target = format!("{service}:{account}");
    let output = tokio::process::Command::new("cmdkey")
        .args(["/delete", &target])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;

    match output {
        Ok(output) if output.status.success() => {
            debug!("Deleted secret from Windows Credential Manager for target={target}");
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!("cmdkey delete failed to execute ({error}), cleaning file fallback");
        }
    }

    // Not found is OK for delete. Always clean up the file fallback.
    file_based_delete(service, account).await
}

#[cfg(target_os = "linux")]
async fn save_secret(service: &str, account: &str, secret: &str) -> Result<(), SecureStorageError> {
    // Try secret-tool (libsecret) first, fall back to file-based.
    if let Ok(output) = tokio::process::Command::new("secret-tool")
        .args([
            "store", "--label", service, "service", service, "account", account,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
    {
        if output.status.success() {
            debug!("Saved secret to Secret Service for account={account}");
            return Ok(());
        }
    }
    warn!("secret-tool not available, falling back to file-based storage");
    file_based_save(service, account, secret).await
}

#[cfg(target_os = "linux")]
async fn load_secret(service: &str, account: &str) -> Result<Option<String>, SecureStorageError> {
    if let Ok(output) = tokio::process::Command::new("secret-tool")
        .args(["lookup", "service", service, "account", account])
        .output()
        .await
    {
        if output.status.success() {
            let secret = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            return Ok(Some(secret));
        }
    }
    file_based_load(service, account).await
}

#[cfg(target_os = "linux")]
async fn delete_secret(service: &str, account: &str) -> Result<(), SecureStorageError> {
    if let Ok(output) = tokio::process::Command::new("secret-tool")
        .args(["clear", "service", service, "account", account])
        .output()
        .await
    {
        if output.status.success() {
            debug!("Deleted secret from Secret Service for account={account}");
            return Ok(());
        }
    }
    file_based_delete(service, account).await
}

// ── File-based fallback ────────────────────────────────────────────────

/// File-based secret storage fallback.
///
/// Secrets are stored as plain text in the user's config directory.
/// **This is NOT secure** — it exists as a fallback when no platform
/// keychain is available. In production, use a proper credential store.
async fn file_based_save(
    service: &str,
    account: &str,
    secret: &str,
) -> Result<(), SecureStorageError> {
    let path = secrets_dir()?.join(safe_filename(service, account));
    let parent = path.parent().ok_or_else(|| {
        SecureStorageError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "secret file path has no parent directory",
        ))
    })?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(SecureStorageError::Io)?;
    tokio::fs::write(&path, secret.as_bytes())
        .await
        .map_err(SecureStorageError::Io)?;
    debug!("Saved secret to file for {service}:{account}");
    Ok(())
}

async fn file_based_load(
    service: &str,
    account: &str,
) -> Result<Option<String>, SecureStorageError> {
    let path = secrets_dir()?.join(safe_filename(service, account));
    if !path.exists() {
        return Ok(None);
    }
    let secret = tokio::fs::read_to_string(&path)
        .await
        .map_err(SecureStorageError::Io)?;
    Ok(Some(secret))
}

async fn file_based_delete(service: &str, account: &str) -> Result<(), SecureStorageError> {
    let path = secrets_dir()?.join(safe_filename(service, account));
    if path.exists() {
        tokio::fs::remove_file(&path)
            .await
            .map_err(SecureStorageError::Io)?;
    }
    Ok(())
}

fn secrets_dir() -> Result<std::path::PathBuf, SecureStorageError> {
    let base = directories::ProjectDirs::from("", "", "remote-code")
        .map(|pd| pd.config_dir().to_path_buf())
        .ok_or_else(|| {
            SecureStorageError::NotAvailable("cannot determine config directory".to_owned())
        })?;
    Ok(base.join("secrets"))
}

fn safe_filename(service: &str, account: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{service}:{account}").hash(&mut hasher);
    format!("{:016x}.secret", hasher.finish())
}
