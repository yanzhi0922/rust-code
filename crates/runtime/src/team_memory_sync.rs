use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[allow(dead_code)]
const MAX_FILE_SIZE_BYTES: usize = 250_000;
#[allow(dead_code)]
const MAX_PUT_BODY_BYTES: usize = 200_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMemoryData {
    pub organization_id: String,
    pub repo: String,
    pub version: u64,
    pub last_modified: String,
    pub checksum: String,
    pub content: TeamMemoryContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMemoryContent {
    pub entries: HashMap<String, String>,
    pub entry_checksums: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone)]
pub struct SkippedSecretFile {
    pub path: String,
    pub rule_id: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct SyncState {
    pub last_known_checksum: Option<String>,
    pub server_checksums: HashMap<String, String>,
    pub server_max_entries: Option<usize>,
}

pub fn create_sync_state() -> SyncState {
    SyncState {
        last_known_checksum: None,
        server_checksums: HashMap::new(),
        server_max_entries: None,
    }
}

pub fn hash_content(content: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(content.as_bytes()))
}

pub struct TeamMemorySync {
    #[allow(dead_code)]
    state: Arc<RwLock<SyncState>>,
    #[allow(dead_code)]
    config: TeamMemorySyncConfig,
}

#[derive(Debug, Clone)]
pub struct TeamMemorySyncConfig {
    pub api_url: String,
    pub timeout_ms: u64,
}

impl Default for TeamMemorySyncConfig {
    fn default() -> Self {
        Self {
            api_url: "https://claude.ai".to_string(),
            timeout_ms: 30_000,
        }
    }
}

impl TeamMemorySync {
    pub fn new(config: TeamMemorySyncConfig) -> Self {
        Self {
            state: Arc::new(RwLock::new(create_sync_state())),
            config,
        }
    }

    pub async fn is_available(&self) -> bool {
        false
    }

    pub async fn pull(&self) -> Result<TeamMemorySyncPullResult> {
        Ok(TeamMemorySyncPullResult {
            success: true,
            files_written: 0,
            entry_count: 0,
            not_modified: false,
            error: None,
        })
    }

    pub async fn push(&self) -> Result<TeamMemorySyncPushResult> {
        Ok(TeamMemorySyncPushResult {
            success: true,
            files_uploaded: 0,
            checksum: None,
            skipped_secrets: vec![],
            error: None,
            error_type: None,
            http_status: None,
        })
    }

    pub async fn sync(&self) -> Result<TeamMemorySyncSyncResult> {
        let pull_result = self.pull().await?;
        let push_result = self.push().await?;

        Ok(TeamMemorySyncSyncResult {
            success: pull_result.success && push_result.success,
            files_pulled: pull_result.files_written,
            files_pushed: push_result.files_uploaded,
            error: None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct TeamMemorySyncPullResult {
    pub success: bool,
    pub files_written: usize,
    pub entry_count: usize,
    pub not_modified: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TeamMemorySyncPushResult {
    pub success: bool,
    pub files_uploaded: usize,
    pub checksum: Option<String>,
    pub skipped_secrets: Vec<SkippedSecretFile>,
    pub error: Option<String>,
    pub error_type: Option<String>,
    pub http_status: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct TeamMemorySyncSyncResult {
    pub success: bool,
    pub files_pulled: usize,
    pub files_pushed: usize,
    pub error: Option<String>,
}

pub fn batch_delta_by_bytes(delta: HashMap<String, String>) -> Vec<HashMap<String, String>> {
    if delta.is_empty() {
        return vec![];
    }

    let empty_body_bytes = b"{}".len();
    let mut batches: Vec<HashMap<String, String>> = vec![];
    let mut current: HashMap<String, String> = HashMap::new();
    let mut current_bytes = empty_body_bytes;

    for (key, value) in delta {
        let entry_bytes = key.len() + value.len() + 2;
        if current_bytes + entry_bytes > MAX_PUT_BODY_BYTES && !current.is_empty() {
            batches.push(current);
            current = HashMap::new();
            current_bytes = empty_body_bytes;
        }
        current.insert(key, value);
        current_bytes += entry_bytes;
    }

    if !current.is_empty() {
        batches.push(current);
    }

    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_sync_state() {
        let state = create_sync_state();
        assert!(state.last_known_checksum.is_none());
        assert!(state.server_checksums.is_empty());
        assert!(state.server_max_entries.is_none());
    }

    #[test]
    fn test_hash_content() {
        let content = "test content";
        let hash = hash_content(content);
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash.len(), 71);
    }

    #[test]
    fn test_batch_delta_by_bytes_empty() {
        let delta: HashMap<String, String> = HashMap::new();
        let batches = batch_delta_by_bytes(delta);
        assert!(batches.is_empty());
    }

    #[test]
    fn test_batch_delta_by_bytes_single() {
        let mut delta = HashMap::new();
        delta.insert("key1".to_string(), "value1".to_string());
        let batches = batch_delta_by_bytes(delta);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 1);
    }

    #[tokio::test]
    async fn test_team_memory_sync_creation() {
        let sync = TeamMemorySync::new(TeamMemorySyncConfig::default());
        assert!(!sync.is_available().await);
    }

    #[tokio::test]
    async fn test_team_memory_sync_pull() {
        let sync = TeamMemorySync::new(TeamMemorySyncConfig::default());
        let result = sync.pull().await.unwrap();
        assert!(result.success);
    }
}
