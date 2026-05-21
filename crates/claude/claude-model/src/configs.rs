//! Model configuration definitions.
//!
//! Maps each model version to its provider-specific ID string.  The
//! [`ALL_MODEL_CONFIGS`] table is the single source of truth for canonical
//! model IDs across Anthropic first-party, AWS Bedrock, GCP Vertex, and
//! Azure Foundry providers.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::providers::ModelProvider;

// ── Per-provider model ID ────────────────────────────────────────────────

/// Provider-specific model ID for a single model version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Anthropic first-party API model ID.
    pub first_party: &'static str,
    /// AWS Bedrock model ID (cross-region inference profile).
    pub bedrock: &'static str,
    /// GCP Vertex AI model ID.
    pub vertex: &'static str,
    /// Azure Foundry / OpenAI-compatible model ID.
    pub foundry: &'static str,
}

// ── Individual model configs ─────────────────────────────────────────────

pub static CLAUDE_3_7_SONNET_CONFIG: LazyLock<ModelConfig> = LazyLock::new(|| ModelConfig {
    first_party: "claude-3-7-sonnet-20250219",
    bedrock: "us.anthropic.claude-3-7-sonnet-20250219-v1:0",
    vertex: "claude-3-7-sonnet@20250219",
    foundry: "claude-3-7-sonnet",
});

pub static CLAUDE_3_5_V2_SONNET_CONFIG: LazyLock<ModelConfig> = LazyLock::new(|| ModelConfig {
    first_party: "claude-3-5-sonnet-20241022",
    bedrock: "anthropic.claude-3-5-sonnet-20241022-v2:0",
    vertex: "claude-3-5-sonnet-v2@20241022",
    foundry: "claude-3-5-sonnet",
});

pub static CLAUDE_3_5_HAIKU_CONFIG: LazyLock<ModelConfig> = LazyLock::new(|| ModelConfig {
    first_party: "claude-3-5-haiku-20241022",
    bedrock: "us.anthropic.claude-3-5-haiku-20241022-v1:0",
    vertex: "claude-3-5-haiku@20241022",
    foundry: "claude-3-5-haiku",
});

pub static CLAUDE_HAIKU_4_5_CONFIG: LazyLock<ModelConfig> = LazyLock::new(|| ModelConfig {
    first_party: "claude-haiku-4-5-20251001",
    bedrock: "us.anthropic.claude-haiku-4-5-20251001-v1:0",
    vertex: "claude-haiku-4-5@20251001",
    foundry: "claude-haiku-4-5",
});

pub static CLAUDE_SONNET_4_CONFIG: LazyLock<ModelConfig> = LazyLock::new(|| ModelConfig {
    first_party: "claude-sonnet-4-20250514",
    bedrock: "us.anthropic.claude-sonnet-4-20250514-v1:0",
    vertex: "claude-sonnet-4@20250514",
    foundry: "claude-sonnet-4",
});

pub static CLAUDE_SONNET_4_5_CONFIG: LazyLock<ModelConfig> = LazyLock::new(|| ModelConfig {
    first_party: "claude-sonnet-4-5-20250929",
    bedrock: "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
    vertex: "claude-sonnet-4-5@20250929",
    foundry: "claude-sonnet-4-5",
});

pub static CLAUDE_OPUS_4_CONFIG: LazyLock<ModelConfig> = LazyLock::new(|| ModelConfig {
    first_party: "claude-opus-4-20250514",
    bedrock: "us.anthropic.claude-opus-4-20250514-v1:0",
    vertex: "claude-opus-4@20250514",
    foundry: "claude-opus-4",
});

pub static CLAUDE_OPUS_4_1_CONFIG: LazyLock<ModelConfig> = LazyLock::new(|| ModelConfig {
    first_party: "claude-opus-4-1-20250805",
    bedrock: "us.anthropic.claude-opus-4-1-20250805-v1:0",
    vertex: "claude-opus-4-1@20250805",
    foundry: "claude-opus-4-1",
});

pub static CLAUDE_OPUS_4_5_CONFIG: LazyLock<ModelConfig> = LazyLock::new(|| ModelConfig {
    first_party: "claude-opus-4-5-20251101",
    bedrock: "us.anthropic.claude-opus-4-5-20251101-v1:0",
    vertex: "claude-opus-4-5@20251101",
    foundry: "claude-opus-4-5",
});

pub static CLAUDE_OPUS_4_6_CONFIG: LazyLock<ModelConfig> = LazyLock::new(|| ModelConfig {
    first_party: "claude-opus-4-7",
    bedrock: "us.anthropic.claude-opus-4-7-v1",
    vertex: "claude-opus-4-7",
    foundry: "claude-opus-4-7",
});

pub static CLAUDE_SONNET_4_6_CONFIG: LazyLock<ModelConfig> = LazyLock::new(|| ModelConfig {
    first_party: "claude-sonnet-4-6",
    bedrock: "us.anthropic.claude-sonnet-4-6",
    vertex: "claude-sonnet-4-6",
    foundry: "claude-sonnet-4-6",
});

// ── Aggregate table ──────────────────────────────────────────────────────

/// Short key identifying a model version within [`ALL_MODEL_CONFIGS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelKey {
    Haiku35,
    Haiku45,
    Sonnet35,
    Sonnet37,
    Sonnet40,
    Sonnet45,
    Sonnet46,
    Opus40,
    Opus41,
    Opus45,
    Opus46,
}

/// Entry in the aggregate model config table.
#[derive(Debug)]
pub struct ModelConfigEntry {
    /// Short key.
    pub key: ModelKey,
    /// Per-provider model IDs.
    pub config: &'static LazyLock<ModelConfig>,
}

/// All known model configs, ordered by family.
pub static ALL_MODEL_CONFIGS: LazyLock<Vec<ModelConfigEntry>> = LazyLock::new(|| {
    vec![
        ModelConfigEntry {
            key: ModelKey::Haiku35,
            config: &CLAUDE_3_5_HAIKU_CONFIG,
        },
        ModelConfigEntry {
            key: ModelKey::Haiku45,
            config: &CLAUDE_HAIKU_4_5_CONFIG,
        },
        ModelConfigEntry {
            key: ModelKey::Sonnet35,
            config: &CLAUDE_3_5_V2_SONNET_CONFIG,
        },
        ModelConfigEntry {
            key: ModelKey::Sonnet37,
            config: &CLAUDE_3_7_SONNET_CONFIG,
        },
        ModelConfigEntry {
            key: ModelKey::Sonnet40,
            config: &CLAUDE_SONNET_4_CONFIG,
        },
        ModelConfigEntry {
            key: ModelKey::Sonnet45,
            config: &CLAUDE_SONNET_4_5_CONFIG,
        },
        ModelConfigEntry {
            key: ModelKey::Sonnet46,
            config: &CLAUDE_SONNET_4_6_CONFIG,
        },
        ModelConfigEntry {
            key: ModelKey::Opus40,
            config: &CLAUDE_OPUS_4_CONFIG,
        },
        ModelConfigEntry {
            key: ModelKey::Opus41,
            config: &CLAUDE_OPUS_4_1_CONFIG,
        },
        ModelConfigEntry {
            key: ModelKey::Opus45,
            config: &CLAUDE_OPUS_4_5_CONFIG,
        },
        ModelConfigEntry {
            key: ModelKey::Opus46,
            config: &CLAUDE_OPUS_4_6_CONFIG,
        },
    ]
});

// ── Derived look-ups ─────────────────────────────────────────────────────

/// All canonical first-party model IDs.
pub static CANONICAL_MODEL_IDS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    ALL_MODEL_CONFIGS
        .iter()
        .map(|e| e.config.first_party)
        .collect()
});

/// Map canonical first-party ID → [`ModelKey`].
pub static CANONICAL_ID_TO_KEY: LazyLock<Vec<(&'static str, ModelKey)>> = LazyLock::new(|| {
    ALL_MODEL_CONFIGS
        .iter()
        .map(|e| (e.config.first_party, e.key))
        .collect()
});

// ── Helpers ──────────────────────────────────────────────────────────────

/// Return the provider-specific model ID for a given [`ModelKey`].
pub fn model_id_for_provider(key: ModelKey, provider: &ModelProvider) -> Option<&'static str> {
    let entry = ALL_MODEL_CONFIGS.iter().find(|e| e.key == key)?;
    let cfg = entry.config;
    Some(match provider {
        ModelProvider::Anthropic => cfg.first_party,
        ModelProvider::AwsBedrock { .. } => cfg.bedrock,
        ModelProvider::GcpVertex { .. } => cfg.vertex,
        ModelProvider::OpenAiCompatible { .. } => cfg.foundry,
    })
}

/// Look up a [`ModelKey`] by its canonical first-party model ID.
pub fn key_for_canonical_id(canonical_id: &str) -> Option<ModelKey> {
    CANONICAL_ID_TO_KEY
        .iter()
        .find(|(id, _)| *id == canonical_id)
        .map(|(_, key)| *key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_configs_have_entries() {
        assert!(ALL_MODEL_CONFIGS.len() >= 11);
    }

    #[test]
    fn canonical_ids_match_keys() {
        assert_eq!(
            key_for_canonical_id("claude-opus-4-7"),
            Some(ModelKey::Opus46)
        );
        assert_eq!(
            key_for_canonical_id("claude-sonnet-4-6"),
            Some(ModelKey::Sonnet46)
        );
        assert_eq!(
            key_for_canonical_id("claude-haiku-4-5-20251001"),
            Some(ModelKey::Haiku45)
        );
        assert_eq!(key_for_canonical_id("nonexistent"), None);
    }

    #[test]
    fn provider_lookup() {
        let provider = ModelProvider::Anthropic;
        assert_eq!(
            model_id_for_provider(ModelKey::Opus46, &provider),
            Some("claude-opus-4-7")
        );
        let bedrock = ModelProvider::AwsBedrock { region: None };
        assert_eq!(
            model_id_for_provider(ModelKey::Opus46, &bedrock),
            Some("us.anthropic.claude-opus-4-7-v1")
        );
    }

    #[test]
    fn canonical_ids_populated() {
        assert!(CANONICAL_MODEL_IDS.contains(&"claude-opus-4-7"));
        assert!(CANONICAL_MODEL_IDS.contains(&"claude-sonnet-4-6"));
        assert!(CANONICAL_MODEL_IDS.contains(&"claude-haiku-4-5-20251001"));
    }

    #[test]
    fn first_party_ids_are_unique() {
        let ids: Vec<&str> = CANONICAL_MODEL_IDS.iter().copied().collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len());
    }
}
