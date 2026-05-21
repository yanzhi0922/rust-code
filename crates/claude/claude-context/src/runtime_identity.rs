use serde::{Deserialize, Serialize};

use crate::fast_mode::{FastModeConfig, FastModeDisabledReason, OrgFastModeStatus};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeUserType {
    Ant,
    External,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSubscriptionContext {
    #[serde(default)]
    pub subscription_type: Option<String>,
    #[serde(default)]
    pub rate_limit_tier: Option<String>,
    #[serde(default)]
    pub billing_type: Option<String>,
    #[serde(default)]
    pub has_extra_usage_enabled: Option<bool>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub account_created_at: Option<String>,
    #[serde(default)]
    pub subscription_created_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeFeatureGates {
    #[serde(default)]
    pub embedded_search_tools: bool,
    #[serde(default)]
    pub explore_plan_agents_enabled: bool,
    #[serde(default)]
    pub verification_agent_enabled: bool,
    #[serde(default)]
    pub code_guide_enabled: bool,
    #[serde(default)]
    pub agent_swarms_enabled: bool,
    #[serde(default)]
    pub show_agent_concurrency_note: bool,
    #[serde(default)]
    pub mcp_instructions_delta_enabled: bool,
    #[serde(default)]
    pub deferred_tools_delta_enabled: bool,
    #[serde(default)]
    pub agent_listing_delta_enabled: bool,
    #[serde(default)]
    pub include_token_budget_prompt: bool,
    #[serde(default)]
    pub sdk_disable_builtin_agents: bool,
    #[serde(default)]
    pub is_fork_subagent_enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeIdentityContext {
    #[serde(default)]
    pub user_type: RuntimeUserType,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub provider_name: Option<String>,
    #[serde(default)]
    pub auth_source: Option<String>,
    #[serde(default)]
    pub is_first_party: bool,
    #[serde(default)]
    pub is_non_interactive: bool,
    #[serde(default)]
    pub kairos_active: bool,
    #[serde(default)]
    pub fast_mode_flag_opt_in: bool,
    #[serde(default)]
    pub fast_mode_per_session_opt_in: bool,
    #[serde(default)]
    pub fast_mode_user_setting: Option<bool>,
    #[serde(default)]
    pub org_fast_mode_enabled: Option<bool>,
    #[serde(default)]
    pub organization_uuid: Option<String>,
    #[serde(default)]
    pub account_uuid: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub subscription: RuntimeSubscriptionContext,
    #[serde(default)]
    pub features: RuntimeFeatureGates,
}

impl RuntimeIdentityContext {
    #[must_use]
    pub fn anonymous() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_legacy_env() -> Self {
        let entrypoint = env_var("CLAUDE_CODE_ENTRYPOINT");
        let user_type = runtime_user_type_from_env(env_var("USER_TYPE").as_deref());
        let is_non_interactive = entrypoint_is_non_interactive(entrypoint.as_deref());
        let subscription_type = env_var("CLAUDE_CODE_SUBSCRIPTION_TYPE")
            .or_else(|| matches!(user_type, RuntimeUserType::Ant).then_some("max".to_owned()));
        let explicit_agent_team_opt_in = env_truthy("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS");
        let embedded_search_tools = embedded_search_tools_enabled(
            entrypoint.as_deref(),
            env_truthy("EMBEDDED_SEARCH_TOOLS"),
        );
        let code_guide_enabled = code_guide_enabled(entrypoint.as_deref());
        let show_agent_concurrency_note = show_agent_concurrency_note(subscription_type.as_deref());

        Self {
            user_type,
            entrypoint,
            is_non_interactive,
            is_first_party: true,
            subscription: RuntimeSubscriptionContext {
                subscription_type,
                rate_limit_tier: env_var("CLAUDE_CODE_RATE_LIMIT_TIER"),
                ..RuntimeSubscriptionContext::default()
            },
            features: RuntimeFeatureGates {
                embedded_search_tools,
                explore_plan_agents_enabled: explore_plan_agents_enabled(true, true),
                verification_agent_enabled: verification_agent_enabled(true, false),
                code_guide_enabled,
                agent_swarms_enabled: agent_swarms_enabled(
                    user_type,
                    explicit_agent_team_opt_in,
                    true,
                ),
                show_agent_concurrency_note,
                mcp_instructions_delta_enabled: env_truthy_or_default(
                    "CLAUDE_CODE_MCP_INSTR_DELTA",
                    true,
                ),
                deferred_tools_delta_enabled: env_truthy_or_default(
                    "CLAUDE_CODE_DEFERRED_TOOLS_DELTA",
                    true,
                ),
                agent_listing_delta_enabled: env_truthy_or_default(
                    "CLAUDE_CODE_AGENT_LIST_IN_MESSAGES",
                    false,
                ),
                include_token_budget_prompt: env_truthy("REMOTE_CODE_TOKEN_BUDGET_PROMPT"),
                sdk_disable_builtin_agents: env_truthy("CLAUDE_AGENT_SDK_DISABLE_BUILTIN_AGENTS"),
                is_fork_subagent_enabled: fork_subagent_enabled(
                    true,
                    env_truthy("CLAUDE_CODE_COORDINATOR_MODE"),
                    is_non_interactive,
                ),
            },
            ..Self::default()
        }
    }

    #[must_use]
    pub fn is_ant_user(&self) -> bool {
        matches!(self.user_type, RuntimeUserType::Ant)
    }

    #[must_use]
    pub fn is_paid_subscriber(&self) -> bool {
        matches!(
            self.subscription.subscription_type.as_deref(),
            Some("pro" | "team" | "enterprise" | "max" | "pay_as_you_go")
        )
    }

    #[must_use]
    pub fn has_extra_usage_enabled(&self) -> Option<bool> {
        self.subscription.has_extra_usage_enabled
    }

    #[must_use]
    pub fn subscription_type_for_targeting(&self) -> Option<&str> {
        self.subscription.subscription_type.as_deref()
    }

    #[must_use]
    pub fn rate_limit_tier_for_targeting(&self) -> Option<&str> {
        self.subscription.rate_limit_tier.as_deref()
    }

    #[must_use]
    pub fn org_fast_mode_status(&self) -> OrgFastModeStatus {
        if self.is_ant_user() {
            return OrgFastModeStatus::Enabled;
        }

        if self.subscription.subscription_type.as_deref() == Some("free") {
            return OrgFastModeStatus::Disabled {
                reason: FastModeDisabledReason::Free,
            };
        }

        if self.subscription.has_extra_usage_enabled == Some(false) {
            return OrgFastModeStatus::Disabled {
                reason: FastModeDisabledReason::ExtraUsageDisabled,
            };
        }

        match self.org_fast_mode_enabled {
            Some(true) => OrgFastModeStatus::Enabled,
            Some(false) => OrgFastModeStatus::Disabled {
                reason: FastModeDisabledReason::Preference,
            },
            None => OrgFastModeStatus::Pending,
        }
    }

    #[must_use]
    pub fn to_fast_mode_config(&self) -> FastModeConfig {
        FastModeConfig {
            enabled: true,
            is_first_party: self.is_first_party,
            is_non_interactive_sdk: self.is_non_interactive,
            kairos_active: self.kairos_active,
            flag_fast_mode: self.fast_mode_flag_opt_in,
            per_session_opt_in: self.fast_mode_per_session_opt_in,
            user_fast_mode_setting: self.fast_mode_user_setting,
        }
    }
}

#[must_use]
pub fn runtime_user_type_from_env(value: Option<&str>) -> RuntimeUserType {
    match value.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value == "ant" => RuntimeUserType::Ant,
        Some(value) if !value.is_empty() => RuntimeUserType::External,
        _ => RuntimeUserType::Unknown,
    }
}

#[must_use]
pub fn entrypoint_is_non_interactive(entrypoint: Option<&str>) -> bool {
    matches!(entrypoint, Some("sdk-ts" | "sdk-py" | "sdk-cli"))
}

#[must_use]
pub fn default_entrypoint(is_non_interactive: bool, github_action: bool) -> &'static str {
    if github_action {
        "claude-code-github-action"
    } else if is_non_interactive {
        "sdk-cli"
    } else {
        "cli"
    }
}

#[must_use]
pub fn embedded_search_tools_enabled(entrypoint: Option<&str>, search_tools_enabled: bool) -> bool {
    if !search_tools_enabled {
        return false;
    }

    !matches!(
        entrypoint,
        Some("sdk-ts" | "sdk-py" | "sdk-cli" | "local-agent")
    )
}

#[must_use]
pub fn code_guide_enabled(entrypoint: Option<&str>) -> bool {
    !matches!(entrypoint, Some("sdk-ts" | "sdk-py" | "sdk-cli"))
}

#[must_use]
pub fn explore_plan_agents_enabled(feature_enabled: bool, rollout_enabled: bool) -> bool {
    feature_enabled && rollout_enabled
}

#[must_use]
pub fn verification_agent_enabled(feature_enabled: bool, rollout_enabled: bool) -> bool {
    feature_enabled && rollout_enabled
}

#[must_use]
pub fn agent_swarms_enabled(
    user_type: RuntimeUserType,
    explicit_opt_in: bool,
    rollout_enabled: bool,
) -> bool {
    matches!(user_type, RuntimeUserType::Ant) || (explicit_opt_in && rollout_enabled)
}

#[must_use]
pub fn show_agent_concurrency_note(subscription_type: Option<&str>) -> bool {
    subscription_type != Some("pro")
}

#[must_use]
pub fn fork_subagent_enabled(
    feature_enabled: bool,
    coordinator_mode: bool,
    is_non_interactive: bool,
) -> bool {
    feature_enabled && !coordinator_mode && !is_non_interactive
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn env_truthy_or_default(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_subscriber_detection_uses_subscription_type() {
        let ctx = RuntimeIdentityContext {
            subscription: RuntimeSubscriptionContext {
                subscription_type: Some("pro".to_owned()),
                ..RuntimeSubscriptionContext::default()
            },
            ..RuntimeIdentityContext::default()
        };

        assert!(ctx.is_paid_subscriber());
        assert_eq!(ctx.subscription_type_for_targeting(), Some("pro"));
    }

    #[test]
    fn org_fast_mode_status_prefers_extra_usage_block() {
        let ctx = RuntimeIdentityContext {
            subscription: RuntimeSubscriptionContext {
                subscription_type: Some("team".to_owned()),
                has_extra_usage_enabled: Some(false),
                ..RuntimeSubscriptionContext::default()
            },
            ..RuntimeIdentityContext::default()
        };

        assert!(matches!(
            ctx.org_fast_mode_status(),
            OrgFastModeStatus::Disabled {
                reason: FastModeDisabledReason::ExtraUsageDisabled,
            }
        ));
    }

    #[test]
    fn to_fast_mode_config_carries_runtime_flags() {
        let ctx = RuntimeIdentityContext {
            is_first_party: false,
            is_non_interactive: true,
            kairos_active: true,
            fast_mode_flag_opt_in: true,
            fast_mode_per_session_opt_in: true,
            fast_mode_user_setting: Some(true),
            ..RuntimeIdentityContext::default()
        };

        let config = ctx.to_fast_mode_config();
        assert!(!config.is_first_party);
        assert!(config.is_non_interactive_sdk);
        assert!(config.kairos_active);
        assert!(config.flag_fast_mode);
        assert!(config.per_session_opt_in);
        assert_eq!(config.user_fast_mode_setting, Some(true));
    }

    #[test]
    fn entrypoint_defaults_match_interactive_and_noninteractive_surfaces() {
        assert_eq!(default_entrypoint(false, false), "cli");
        assert_eq!(default_entrypoint(true, false), "sdk-cli");
        assert_eq!(default_entrypoint(false, true), "claude-code-github-action");
    }

    #[test]
    fn fork_subagent_gate_matches_research_rules() {
        assert!(fork_subagent_enabled(true, false, false));
        assert!(!fork_subagent_enabled(true, true, false));
        assert!(!fork_subagent_enabled(true, false, true));
        assert!(!fork_subagent_enabled(false, false, false));
    }

    #[test]
    fn external_agent_swarms_require_opt_in_but_ant_users_bypass_it() {
        assert!(agent_swarms_enabled(RuntimeUserType::Ant, false, false));
        assert!(!agent_swarms_enabled(
            RuntimeUserType::External,
            false,
            true
        ));
        assert!(agent_swarms_enabled(RuntimeUserType::External, true, true));
        assert!(!agent_swarms_enabled(
            RuntimeUserType::External,
            true,
            false
        ));
    }
}
