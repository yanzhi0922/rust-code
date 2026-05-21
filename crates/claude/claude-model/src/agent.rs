//! Agent model configuration.
//!
//! Determines the effective model for sub-agents, handling:
//! - Environment variable overrides (`CLAUDE_CODE_SUBAGENT_MODEL`)
//! - `"inherit"` mode (sub-agent uses the parent's model)
//! - Bedrock region prefix inheritance for cross-region inference
//! - Alias-to-parent-tier matching (prevents surprising downgrades)

use crate::bedrock::{apply_bedrock_region_prefix, get_bedrock_region_prefix, is_bedrock_provider};
use crate::model::ResolveContext;
use crate::model::parse_user_specified_model_with_ctx;
use crate::providers::ModelProvider;
use crate::validate::get_canonical_name;

// ── Types ────────────────────────────────────────────────────────────────

/// Available model options for agent configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentModelAlias {
    /// Anthropic Sonnet family.
    Sonnet,
    /// Anthropic Opus family.
    Opus,
    /// Anthropic Haiku family.
    Haiku,
    /// Inherit the model from the parent conversation.
    Inherit,
}

impl AgentModelAlias {
    /// All available agent model aliases.
    pub const ALL: &[Self] = &[Self::Sonnet, Self::Opus, Self::Haiku, Self::Inherit];

    /// Parse from a string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "sonnet" => Some(Self::Sonnet),
            "opus" => Some(Self::Opus),
            "haiku" => Some(Self::Haiku),
            "inherit" => Some(Self::Inherit),
            _ => None,
        }
    }

    /// Return the string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sonnet => "sonnet",
            Self::Opus => "opus",
            Self::Haiku => "haiku",
            Self::Inherit => "inherit",
        }
    }
}

impl std::fmt::Display for AgentModelAlias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A model option for the agent model picker.
#[derive(Debug, Clone)]
pub struct AgentModelOption {
    /// The alias value.
    pub value: AgentModelAlias,
    /// Human-readable label.
    pub label: &'static str,
    /// Description for the picker UI.
    pub description: &'static str,
}

// ── Default ──────────────────────────────────────────────────────────────

/// The default sub-agent model: `"inherit"`.
pub fn get_default_subagent_model() -> &'static str {
    "inherit"
}

// ── Agent model resolution ───────────────────────────────────────────────

/// Context for agent model resolution.
#[derive(Debug, Clone)]
pub struct AgentModelContext {
    /// The active API provider.
    pub provider: ModelProvider,
    /// Full resolution context (for env var overrides, etc.).
    pub resolve_ctx: ResolveContext,
    /// Environment variable override for the subagent model.
    pub env_subagent_model: Option<String>,
}

/// Get the effective model string for an agent.
///
/// # Priority
/// 1. `CLAUDE_CODE_SUBAGENT_MODEL` environment variable override
/// 2. Tool-specified model (if provided)
/// 3. Agent model setting (`"inherit"` or alias)
///
/// For Bedrock, the parent's cross-region inference prefix is inherited by
/// subagents using alias models to ensure they use the same region.
pub fn get_agent_model(
    agent_model: Option<&str>,
    parent_model: &str,
    tool_specified_model: Option<&str>,
    ctx: &AgentModelContext,
) -> String {
    // 1. Environment variable override takes highest priority.
    if let Some(ref env_model) = ctx.env_subagent_model {
        return parse_user_specified_model_with_ctx(env_model, &ctx.resolve_ctx);
    }

    // Extract Bedrock region prefix from parent model.
    let parent_region_prefix = get_bedrock_region_prefix(parent_model);
    let is_bedrock = is_bedrock_provider(&ctx.provider);

    // Helper to apply parent region prefix for Bedrock models.
    let apply_parent_prefix = |resolved: &str, original_spec: &str| -> String {
        if let Some(prefix) = parent_region_prefix
            && is_bedrock
        {
            // If the original spec already has its own region prefix, preserve it.
            if get_bedrock_region_prefix(original_spec).is_some() {
                return resolved.to_owned();
            }
            return apply_bedrock_region_prefix(resolved, prefix);
        }
        resolved.to_owned()
    };

    // 2. Tool-specified model takes second priority.
    if let Some(tool_model) = tool_specified_model {
        if alias_matches_parent_tier(tool_model, parent_model) {
            return parent_model.to_owned();
        }
        let model = parse_user_specified_model_with_ctx(tool_model, &ctx.resolve_ctx);
        return apply_parent_prefix(&model, tool_model);
    }

    // 3. Agent model setting.
    let effective = agent_model.unwrap_or(get_default_subagent_model());

    if effective == "inherit" {
        // Inherit the parent model directly.
        return parent_model.to_owned();
    }

    if alias_matches_parent_tier(effective, parent_model) {
        return parent_model.to_owned();
    }

    let model = parse_user_specified_model_with_ctx(effective, &ctx.resolve_ctx);
    apply_parent_prefix(&model, effective)
}

// ── Alias matching ───────────────────────────────────────────────────────

/// Check if a bare family alias (opus/sonnet/haiku) matches the parent
/// model's tier.  When it does, the subagent inherits the parent's exact
/// model string instead of resolving the alias to a provider default.
///
/// This prevents surprising downgrades: a Vertex user on Opus 4.6 who
/// spawns a subagent with `model: opus` should get Opus 4.6, not whatever
/// the 3P default is.
///
/// Only bare family aliases match.  `opus[1m]`, `best`, `opusplan` fall
/// through since they carry semantics beyond "same tier as parent".
pub fn alias_matches_parent_tier(alias: &str, parent_model: &str) -> bool {
    let canonical = get_canonical_name(parent_model);
    match alias.to_lowercase().as_str() {
        "opus" => canonical.contains("opus"),
        "sonnet" => canonical.contains("sonnet"),
        "haiku" => canonical.contains("haiku"),
        _ => false,
    }
}

// ── Display helpers ──────────────────────────────────────────────────────

/// Get a display string for an agent model setting.
pub fn get_agent_model_display(model: Option<&str>) -> String {
    match model {
        None => "Inherit from parent (default)".to_owned(),
        Some("inherit") => "Inherit from parent".to_owned(),
        Some(m) => {
            let mut chars = m.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        }
    }
}

/// Get available model options for agents.
pub fn get_agent_model_options() -> Vec<AgentModelOption> {
    vec![
        AgentModelOption {
            value: AgentModelAlias::Sonnet,
            label: "Sonnet",
            description: "Balanced performance - best for most agents",
        },
        AgentModelOption {
            value: AgentModelAlias::Opus,
            label: "Opus",
            description: "Most capable for complex reasoning tasks",
        },
        AgentModelOption {
            value: AgentModelAlias::Haiku,
            label: "Haiku",
            description: "Fast and efficient for simple tasks",
        },
        AgentModelOption {
            value: AgentModelAlias::Inherit,
            label: "Inherit from parent",
            description: "Use the same model as the main conversation",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_ctx() -> AgentModelContext {
        AgentModelContext {
            provider: ModelProvider::Anthropic,
            resolve_ctx: ResolveContext::default(),
            env_subagent_model: None,
        }
    }

    #[test]
    fn default_is_inherit() {
        assert_eq!(get_default_subagent_model(), "inherit");
    }

    #[test]
    fn inherit_returns_parent_model() {
        let ctx = default_ctx();
        let model = get_agent_model(Some("inherit"), "claude-opus-4-6", None, &ctx);
        assert_eq!(model, "claude-opus-4-6");
    }

    #[test]
    fn none_returns_parent_model() {
        let ctx = default_ctx();
        let model = get_agent_model(None, "claude-opus-4-6", None, &ctx);
        assert_eq!(model, "claude-opus-4-6");
    }

    #[test]
    fn tool_specified_model_priority() {
        let ctx = default_ctx();
        let model = get_agent_model(Some("inherit"), "claude-opus-4-6", Some("sonnet"), &ctx);
        assert!(model.contains("sonnet"));
    }

    #[test]
    fn env_override_highest_priority() {
        let ctx = AgentModelContext {
            provider: ModelProvider::Anthropic,
            resolve_ctx: ResolveContext::default(),
            env_subagent_model: Some("claude-haiku-4-5-20251001".into()),
        };
        let model = get_agent_model(Some("inherit"), "claude-opus-4-6", Some("sonnet"), &ctx);
        assert_eq!(model, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn alias_matches_parent_tier_opus() {
        assert!(alias_matches_parent_tier("opus", "claude-opus-4-6"));
        assert!(alias_matches_parent_tier(
            "opus",
            "claude-opus-4-5-20251101"
        ));
        assert!(!alias_matches_parent_tier("opus", "claude-sonnet-4-6"));
    }

    #[test]
    fn alias_matches_parent_tier_sonnet() {
        assert!(alias_matches_parent_tier("sonnet", "claude-sonnet-4-6"));
        assert!(!alias_matches_parent_tier("sonnet", "claude-opus-4-6"));
    }

    #[test]
    fn alias_matches_parent_tier_haiku() {
        assert!(alias_matches_parent_tier(
            "haiku",
            "claude-haiku-4-5-20251001"
        ));
        assert!(!alias_matches_parent_tier("haiku", "claude-opus-4-6"));
    }

    #[test]
    fn alias_matches_parent_tier_non_family() {
        assert!(!alias_matches_parent_tier("best", "claude-opus-4-6"));
        assert!(!alias_matches_parent_tier("opusplan", "claude-opus-4-6"));
    }

    #[test]
    fn alias_matching_returns_parent_model() {
        let ctx = default_ctx();
        // "opus" matches parent's opus tier → return parent exactly.
        let model = get_agent_model(Some("opus"), "claude-opus-4-6", None, &ctx);
        assert_eq!(model, "claude-opus-4-6");
    }

    #[test]
    fn display_for_none() {
        assert_eq!(
            get_agent_model_display(None),
            "Inherit from parent (default)"
        );
    }

    #[test]
    fn display_for_inherit() {
        assert_eq!(
            get_agent_model_display(Some("inherit")),
            "Inherit from parent"
        );
    }

    #[test]
    fn display_for_model_name() {
        assert_eq!(get_agent_model_display(Some("sonnet")), "Sonnet");
    }

    #[test]
    fn agent_model_options_count() {
        let options = get_agent_model_options();
        assert_eq!(options.len(), 4);
        assert_eq!(options[0].value, AgentModelAlias::Sonnet);
        assert_eq!(options[3].value, AgentModelAlias::Inherit);
    }

    #[test]
    fn bedrock_region_prefix_inheritance() {
        let ctx = AgentModelContext {
            provider: ModelProvider::AwsBedrock {
                region: Some("us-east-1".into()),
            },
            resolve_ctx: ResolveContext::default(),
            env_subagent_model: None,
        };
        // Parent uses EU prefix, sub-agent with "sonnet" alias.
        // The resolved model is "claude-sonnet-4-6" (first-party format).
        // apply_bedrock_region_prefix only works on Bedrock-format IDs
        // (starting with "anthropic."), so the first-party ID passes
        // through unchanged.  In production, the ResolveContext would
        // have env_default_sonnet set to the Bedrock-format ID.
        let model = get_agent_model(
            Some("sonnet"),
            "eu.anthropic.claude-opus-4-6-v1",
            None,
            &ctx,
        );
        // With default ResolveContext, sonnet resolves to first-party format.
        assert!(model.contains("sonnet"));
    }

    #[test]
    fn bedrock_region_prefix_with_bedrock_model() {
        let ctx = AgentModelContext {
            provider: ModelProvider::AwsBedrock {
                region: Some("us-east-1".into()),
            },
            resolve_ctx: ResolveContext {
                provider: ModelProvider::AwsBedrock {
                    region: Some("us-east-1".into()),
                },
                env_default_sonnet: Some("us.anthropic.claude-sonnet-4-6".into()),
                ..Default::default()
            },
            env_subagent_model: None,
        };
        // Parent uses EU prefix, sub-agent resolves to Bedrock-format sonnet.
        // The prefix should be applied to replace "us." with "eu.".
        let model = get_agent_model(
            Some("sonnet"),
            "eu.anthropic.claude-opus-4-6-v1",
            None,
            &ctx,
        );
        assert!(model.starts_with("eu.anthropic."));
        assert!(model.contains("sonnet"));
    }

    #[test]
    fn agent_alias_parse() {
        assert_eq!(
            AgentModelAlias::parse("sonnet"),
            Some(AgentModelAlias::Sonnet)
        );
        assert_eq!(AgentModelAlias::parse("OPUS"), Some(AgentModelAlias::Opus));
        assert_eq!(
            AgentModelAlias::parse("inherit"),
            Some(AgentModelAlias::Inherit)
        );
        assert_eq!(AgentModelAlias::parse("unknown"), None);
    }
}
