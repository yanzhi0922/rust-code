use claude_runtime::session::TokenUsage;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct PromptCacheStats {
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub hits: u64,
    pub misses: u64,
}

#[derive(Debug, Clone)]
pub struct PromptCache {
    entries: HashMap<String, CacheEntry>,
    stats: PromptCacheStats,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct CacheEntry {
    content_hash: String,
    tokens: u32,
}

impl PromptCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            stats: PromptCacheStats::default(),
        }
    }

    pub fn get(&self, key: &str) -> Option<(String, u32)> {
        self.entries
            .get(key)
            .map(|e| (e.content_hash.clone(), e.tokens))
    }

    pub fn put(&mut self, key: String, content: &str, tokens: u32) {
        let hash = Self::hash_content(content);
        self.entries.insert(
            key,
            CacheEntry {
                content_hash: hash,
                tokens,
            },
        );
    }

    pub fn record_usage(&mut self, usage: &TokenUsage) {
        if usage.cache_read_input_tokens > 0 {
            self.stats.cache_read_tokens += usage.cache_read_input_tokens as u64;
            self.stats.hits += 1;
        }
        if usage.cache_creation_input_tokens > 0 {
            self.stats.cache_creation_tokens += usage.cache_creation_input_tokens as u64;
            self.stats.misses += 1;
        }
    }

    pub fn stats(&self) -> &PromptCacheStats {
        &self.stats
    }

    fn hash_content(content: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let hash = hasher.finalize();
        hex::encode(hash)
    }
}

impl Default for PromptCache {
    fn default() -> Self {
        Self::new()
    }
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }
}
