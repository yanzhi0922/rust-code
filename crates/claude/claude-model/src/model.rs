//! Model definitions and selection logic.
//!
//! Central types for representing AI models and resolving user-specified
//! model names (aliases, `[1m]` tags, provider-specific IDs) into concrete
//! model identifiers.

use serde::{Deserialize, Serialize};

use crate::aliases::{is_model_alias, resolve_alias};
use crate::check_1m::{has_1m_tag, strip_1m_tag};
use crate::providers::{
    ModelProvider, default_haiku_model, default_opus_model, default_sonnet_model,
};

// ── Core types ──────────────────────────────────────────────────────────

/// Well-known model variants.
///
/// The `Custom` variant allows arbitrary model IDs for providers or
/// deployments not covered by the built-in list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Model {
    // ── Opus family ──────────────────────────────────────────────────
    ClaudeOpus4_7,
    ClaudeOpus4_6,
    ClaudeOpus4_5_20251101,
    ClaudeOpus4_1_20250805,
    ClaudeOpus4_20250514,
    // ── Sonnet family ────────────────────────────────────────────────
    ClaudeSonnet4_6,
    ClaudeSonnet4_5_20250929,
    ClaudeSonnet4_20250514,
    ClaudeSonnet3_7_20250219,
    ClaudeSonnet3_5_20241022,
    // ── Haiku family ─────────────────────────────────────────────────
    ClaudeHaiku4_5_20251001,
    ClaudeHaiku3_5_20241022,
    // ── Catch-all ────────────────────────────────────────────────────
    Custom(String),
}

impl Model {
    /// Return the canonical first-party model ID for this variant.
    pub fn model_id(&self) -> &str {
        match self {
            Self::ClaudeOpus4_7 => "claude-opus-4-7",
            Self::ClaudeOpus4_6 => "claude-opus-4-6",
            Self::ClaudeOpus4_5_20251101 => "claude-opus-4-5-20251101",
            Self::ClaudeOpus4_1_20250805 => "claude-opus-4-1-20250805",
            Self::ClaudeOpus4_20250514 => "claude-opus-4-20250514",
            Self::ClaudeSonnet4_6 => "claude-sonnet-4-6",
            Self::ClaudeSonnet4_5_20250929 => "claude-sonnet-4-5-20250929",
            Self::ClaudeSonnet4_20250514 => "claude-sonnet-4-20250514",
            Self::ClaudeSonnet3_7_20250219 => "claude-3-7-sonnet-20250219",
            Self::ClaudeSonnet3_5_20241022 => "claude-3-5-sonnet-20241022",
            Self::ClaudeHaiku4_5_20251001 => "claude-haiku-4-5-20251001",
            Self::ClaudeHaiku3_5_20241022 => "claude-3-5-haiku-20241022",
            Self::Custom(id) => id,
        }
    }

    /// Parse a canonical model ID into a [`Model`] variant.
    ///
    /// Returns `Model::Custom(id)` for unknown IDs.
    pub fn from_id(id: &str) -> Self {
        match id {
            "claude-opus-4-7" => Self::ClaudeOpus4_7,
            "claude-opus-4-6" => Self::ClaudeOpus4_6,
            "claude-opus-4-5-20251101" => Self::ClaudeOpus4_5_20251101,
            "claude-opus-4-1-20250805" => Self::ClaudeOpus4_1_20250805,
            "claude-opus-4-20250514" => Self::ClaudeOpus4_20250514,
            "claude-sonnet-4-6" => Self::ClaudeSonnet4_6,
            "claude-sonnet-4-5-20250929" => Self::ClaudeSonnet4_5_20250929,
            "claude-sonnet-4-20250514" => Self::ClaudeSonnet4_20250514,
            "claude-3-7-sonnet-20250219" => Self::ClaudeSonnet3_7_20250219,
            "claude-3-5-sonnet-20241022" => Self::ClaudeSonnet3_5_20241022,
            "claude-haiku-4-5-20251001" => Self::ClaudeHaiku4_5_20251001,
            "claude-3-5-haiku-20241022" => Self::ClaudeHaiku3_5_20241022,
            other => Self::Custom(other.to_owned()),
        }
    }

    /// Returns `true` for any Opus-family model (excluding `Custom`).
    pub fn is_opus(&self) -> bool {
        matches!(
            self,
            Self::ClaudeOpus4_6
                | Self::ClaudeOpus4_7
                | Self::ClaudeOpus4_5_20251101
                | Self::ClaudeOpus4_1_20250805
                | Self::ClaudeOpus4_20250514
        )
    }

    /// Returns `true` for any Sonnet-family model (excluding `Custom`).
    pub fn is_sonnet(&self) -> bool {
        matches!(
            self,
            Self::ClaudeSonnet4_6
                | Self::ClaudeSonnet4_5_20250929
                | Self::ClaudeSonnet4_20250514
                | Self::ClaudeSonnet3_7_20250219
                | Self::ClaudeSonnet3_5_20241022
        )
    }

    /// Returns `true` for any Haiku-family model (excluding `Custom`).
    pub fn is_haiku(&self) -> bool {
        matches!(
            self,
            Self::ClaudeHaiku4_5_20251001 | Self::ClaudeHaiku3_5_20241022
        )
    }
}

impl std::fmt::Display for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.model_id())
    }
}

// ── Model setting (user configuration) ──────────────────────────────────

/// How the user configured the model — either a specific ID / alias or
/// `Auto` to use the built-in default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSetting {
    /// Use the built-in default model selection.
    #[default]
    Auto,
    /// Use a specific model ID or alias.
    Specific(String),
}

// ── Resolution ──────────────────────────────────────────────────────────

/// Context needed to resolve a [`ModelSetting`] into a concrete model ID.
#[derive(Debug, Clone)]
pub struct ResolveContext {
    /// The active API provider.
    pub provider: ModelProvider,
    /// Whether the user is a Max subscriber (influences default model).
    pub is_max_subscriber: bool,
    /// Whether the user is a Team Premium subscriber.
    pub is_team_premium_subscriber: bool,
    /// Environment-variable or flag override for the default Opus model.
    pub env_default_opus: Option<String>,
    /// Environment-variable or flag override for the default Sonnet model.
    pub env_default_sonnet: Option<String>,
    /// Environment-variable or flag override for the default Haiku model.
    pub env_default_haiku: Option<String>,
    /// Environment-variable or flag override for the main model.
    pub env_model: Option<String>,
}

impl Default for ResolveContext {
    fn default() -> Self {
        Self {
            provider: ModelProvider::Anthropic,
            is_max_subscriber: false,
            is_team_premium_subscriber: false,
            env_default_opus: None,
            env_default_sonnet: None,
            env_default_haiku: None,
            env_model: None,
        }
    }
}

/// Resolve a [`ModelSetting`] into a concrete model ID string.
///
/// Priority:
/// 1. `ModelSetting::Specific` — resolved via [`parse_user_specified_model`].
/// 2. Environment variable override (`env_model`).
/// 3. Built-in default (Opus for Max/Team Premium, Sonnet otherwise).
pub fn resolve_model(setting: &ModelSetting, ctx: &ResolveContext) -> String {
    match setting {
        ModelSetting::Specific(s) => parse_user_specified_model_with_ctx(s, ctx),
        ModelSetting::Auto => {
            if let Some(ref env) = ctx.env_model {
                return parse_user_specified_model_with_ctx(env, ctx);
            }
            default_main_loop_model(ctx)
        }
    }
}

/// Resolve the default main-loop model based on subscription tier.
pub fn default_main_loop_model(ctx: &ResolveContext) -> String {
    if ctx.is_max_subscriber || ctx.is_team_premium_subscriber {
        default_opus_model(&ctx.provider).to_owned()
    } else {
        default_sonnet_model(&ctx.provider).to_owned()
    }
}

/// Parse a user-specified model string (alias, `[1m]` tag, or raw ID) into
/// a concrete model ID.
///
/// This is the primary entry point for model resolution.
pub fn parse_user_specified_model(input: &str) -> String {
    let ctx = ResolveContext::default();
    parse_user_specified_model_with_ctx(input, &ctx)
}

/// Parse a user-specified model string with explicit provider context.
pub fn parse_user_specified_model_with_ctx(input: &str, ctx: &ResolveContext) -> String {
    let trimmed = input.trim();
    let lower = trimmed.to_lowercase();

    let has_1m = has_1m_tag(&lower);
    let base = if has_1m {
        strip_1m_tag(&lower).trim().to_owned()
    } else {
        lower.clone()
    };

    // Resolve aliases.
    if is_model_alias(&base) {
        let resolved = match base.as_str() {
            "opusplan" => {
                // opusplan: Sonnet by default, Opus in plan mode.
                let model = ctx
                    .env_default_sonnet
                    .as_deref()
                    .unwrap_or_else(|| default_sonnet_model(&ctx.provider));
                return format!("{model}{}", if has_1m { "[1m]" } else { "" });
            }
            "sonnet" => ctx
                .env_default_sonnet
                .as_deref()
                .unwrap_or_else(|| default_sonnet_model(&ctx.provider)),
            "opus" => ctx
                .env_default_opus
                .as_deref()
                .unwrap_or_else(|| default_opus_model(&ctx.provider)),
            "haiku" => ctx
                .env_default_haiku
                .as_deref()
                .unwrap_or_else(|| default_haiku_model(&ctx.provider)),
            "best" => ctx
                .env_default_opus
                .as_deref()
                .unwrap_or_else(|| default_opus_model(&ctx.provider)),
            // 1m-tagged aliases
            "sonnet[1m]" => ctx
                .env_default_sonnet
                .as_deref()
                .unwrap_or_else(|| default_sonnet_model(&ctx.provider)),
            "opus[1m]" => ctx
                .env_default_opus
                .as_deref()
                .unwrap_or_else(|| default_opus_model(&ctx.provider)),
            _ => &base,
        };
        return format!("{resolved}{}", if has_1m { "[1m]" } else { "" });
    }

    // Try the alias table directly.
    if let Some(resolved) = resolve_alias(&base) {
        return format!("{resolved}{}", if has_1m { "[1m]" } else { "" });
    }

    // Pass through custom model names, preserving case for the base name.
    if has_1m {
        let original_base = strip_1m_tag(trimmed).trim();
        format!("{original_base}[1m]")
    } else {
        trimmed.to_owned()
    }
}

/// Returns the "small fast" model ID (used for lightweight queries).
pub fn get_small_fast_model(ctx: &ResolveContext) -> String {
    ctx.env_default_haiku
        .as_deref()
        .unwrap_or_else(|| default_haiku_model(&ctx.provider))
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_from_id_roundtrip() {
        let ids = [
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
        ];
        for id in ids {
            assert_eq!(Model::from_id(id).model_id(), id);
        }
    }

    #[test]
    fn custom_model() {
        let m = Model::from_id("my-deployment-v1");
        assert_eq!(m.model_id(), "my-deployment-v1");
        assert!(matches!(m, Model::Custom(_)));
    }

    #[test]
    fn family_checks() {
        assert!(Model::ClaudeOpus4_7.is_opus());
        assert!(!Model::ClaudeOpus4_7.is_sonnet());
        assert!(Model::ClaudeSonnet4_6.is_sonnet());
        assert!(Model::ClaudeHaiku4_5_20251001.is_haiku());
    }

    #[test]
    fn resolve_auto_default() {
        let ctx = ResolveContext::default();
        let model = resolve_model(&ModelSetting::Auto, &ctx);
        // Default for non-subscribers is Sonnet.
        assert!(model.contains("sonnet"));
    }

    #[test]
    fn resolve_auto_max_subscriber() {
        let ctx = ResolveContext {
            is_max_subscriber: true,
            ..Default::default()
        };
        let model = resolve_model(&ModelSetting::Auto, &ctx);
        assert!(model.contains("opus"));
    }

    #[test]
    fn resolve_specific_alias() {
        let model = resolve_model(
            &ModelSetting::Specific("opus".into()),
            &ResolveContext::default(),
        );
        assert!(model.contains("opus"));
    }

    #[test]
    fn parse_alias_with_1m() {
        let model = parse_user_specified_model("opus[1m]");
        assert!(model.ends_with("[1m]"));
        assert!(model.contains("opus"));
    }

    #[test]
    fn parse_custom_passthrough() {
        let model = parse_user_specified_model("my-custom-model");
        assert_eq!(model, "my-custom-model");
    }

    #[test]
    fn small_fast_model() {
        let ctx = ResolveContext::default();
        let model = get_small_fast_model(&ctx);
        assert!(model.contains("haiku"));
    }
}
