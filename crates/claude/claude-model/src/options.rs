//! Model options for the model picker UI.
//!
//! Generates the list of available model options based on user tier,
//! provider, and subscription status.  This is the Rust equivalent of the
//! TypeScript `modelOptions.ts` module.

use crate::allowlist::is_model_allowed;
use crate::configs::{ModelKey, model_id_for_provider};
use crate::providers::ModelProvider;
use crate::validate::get_public_model_display_name;

// ── Types ────────────────────────────────────────────────────────────────

/// A single option in the model picker.
#[derive(Debug, Clone)]
pub struct ModelOption {
    /// The value to set when this option is selected.
    pub value: String,
    /// Human-readable label.
    pub label: String,
    /// Description shown in the picker UI.
    pub description: String,
    /// Optional extended description for the model detail view.
    pub description_for_model: Option<String>,
}

/// Context for generating model options.
#[derive(Debug, Clone)]
pub struct OptionsContext {
    /// The active API provider.
    pub provider: ModelProvider,
    /// Whether the user is on a Max / Team Premium plan.
    pub is_max_or_team_premium: bool,
    /// Whether the user is a Claude.ai subscriber (Pro/Max/Team).
    pub is_subscriber: bool,
    /// Whether the user is an internal "Ant" user.
    pub is_ant: bool,
    /// Whether the user has 1M context access for Opus.
    pub has_opus_1m: bool,
    /// Whether the user has 1M context access for Sonnet.
    pub has_sonnet_1m: bool,
    /// Whether Opus 1M merge is enabled.
    pub opus_1m_merge_enabled: bool,
    /// Whether to show fast-mode pricing.
    pub fast_mode: bool,
    /// Custom model option from environment variable.
    pub custom_model: Option<String>,
    /// Additional model options from bootstrap config.
    pub additional_options: Vec<ModelOption>,
    /// Currently selected model setting (if any).
    pub current_model: Option<String>,
    /// Allowlist of available models (if configured).
    pub available_models: Option<Vec<String>>,
}

impl Default for OptionsContext {
    fn default() -> Self {
        Self {
            provider: ModelProvider::Anthropic,
            is_max_or_team_premium: false,
            is_subscriber: false,
            is_ant: false,
            has_opus_1m: false,
            has_sonnet_1m: false,
            opus_1m_merge_enabled: false,
            fast_mode: false,
            custom_model: None,
            additional_options: vec![],
            current_model: None,
            available_models: None,
        }
    }
}

// ── Default option ───────────────────────────────────────────────────────

/// Get the default (recommended) option for the user.
pub fn get_default_option(ctx: &OptionsContext) -> ModelOption {
    let current = if ctx.is_max_or_team_premium {
        "Opus"
    } else {
        "Sonnet"
    };
    ModelOption {
        value: String::new(), // Empty value = use default
        label: "Default (recommended)".into(),
        description: format!("Use the default model (currently {current})"),
        description_for_model: Some(format!("Default model (currently {current})")),
    }
}

// ── Individual model options ─────────────────────────────────────────────

fn sonnet_option(ctx: &OptionsContext) -> ModelOption {
    let is_3p = !matches!(ctx.provider, ModelProvider::Anthropic);
    let value = if is_3p {
        model_id_for_provider(ModelKey::Sonnet46, &ctx.provider)
            .unwrap_or("claude-sonnet-4-6")
            .to_owned()
    } else {
        "sonnet".into()
    };
    ModelOption {
        value,
        label: "Sonnet".into(),
        description: "Sonnet 4.6 · Best for everyday tasks".into(),
        description_for_model: Some(
            "Sonnet 4.6 - best for everyday tasks. Generally recommended for most coding tasks"
                .into(),
        ),
    }
}

fn opus_option(ctx: &OptionsContext) -> ModelOption {
    let is_3p = !matches!(ctx.provider, ModelProvider::Anthropic);
    let value = if is_3p {
        model_id_for_provider(ModelKey::Opus46, &ctx.provider)
            .unwrap_or("claude-opus-4-7")
            .to_owned()
    } else {
        "opus".into()
    };
    ModelOption {
        value,
        label: "Opus".into(),
        description: "Opus 4.7 · Most capable for complex work".into(),
        description_for_model: Some("Opus 4.7 - most capable for complex work".into()),
    }
}

fn haiku_option() -> ModelOption {
    ModelOption {
        value: "haiku".into(),
        label: "Haiku".into(),
        description: "Haiku 4.5 · Fastest for quick answers".into(),
        description_for_model: Some(
            "Haiku 4.5 - fastest for quick answers. Lower cost but less capable than Sonnet 4.6."
                .into(),
        ),
    }
}

fn sonnet_1m_option(ctx: &OptionsContext) -> ModelOption {
    let is_3p = !matches!(ctx.provider, ModelProvider::Anthropic);
    let value = if is_3p {
        let base =
            model_id_for_provider(ModelKey::Sonnet46, &ctx.provider).unwrap_or("claude-sonnet-4-6");
        format!("{base}[1m]")
    } else {
        "sonnet[1m]".into()
    };
    ModelOption {
        value,
        label: "Sonnet (1M context)".into(),
        description: "Sonnet 4.6 for long sessions".into(),
        description_for_model: Some(
            "Sonnet 4.6 with 1M context window - for long sessions with large codebases".into(),
        ),
    }
}

fn opus_1m_option(ctx: &OptionsContext) -> ModelOption {
    let is_3p = !matches!(ctx.provider, ModelProvider::Anthropic);
    let value = if is_3p {
        let base =
            model_id_for_provider(ModelKey::Opus46, &ctx.provider).unwrap_or("claude-opus-4-7");
        format!("{base}[1m]")
    } else {
        "opus[1m]".into()
    };
    ModelOption {
        value,
        label: "Opus (1M context)".into(),
        description: "Opus 4.7 for long sessions".into(),
        description_for_model: Some(
            "Opus 4.7 with 1M context window - for long sessions with large codebases".into(),
        ),
    }
}

pub fn opus_plan_option() -> ModelOption {
    ModelOption {
        value: "opusplan".into(),
        label: "Opus Plan Mode".into(),
        description: "Use Opus 4.7 in plan mode, Sonnet 4.6 otherwise".into(),
        description_for_model: None,
    }
}

// ── Full option list ─────────────────────────────────────────────────────

/// Get the complete list of model options for the model picker.
pub fn get_model_options(ctx: &OptionsContext) -> Vec<ModelOption> {
    let mut options = get_model_options_base(ctx);

    // Add custom model from environment variable.
    if let Some(ref custom) = ctx.custom_model
        && !options.iter().any(|o| o.value == *custom)
    {
        options.push(ModelOption {
            value: custom.clone(),
            label: custom.clone(),
            description: format!("Custom model ({custom})"),
            description_for_model: None,
        });
    }

    // Add additional model options from bootstrap config.
    for opt in &ctx.additional_options {
        if !options.iter().any(|o| o.value == opt.value) {
            options.push(opt.clone());
        }
    }

    // Add current model if not already in options.
    if let Some(ref current) = ctx.current_model
        && !current.is_empty()
        && !options.iter().any(|o| o.value == *current)
    {
        if let Some(display) = get_public_model_display_name(current) {
            options.push(ModelOption {
                value: current.clone(),
                label: display,
                description: current.clone(),
                description_for_model: None,
            });
        } else {
            options.push(ModelOption {
                value: current.clone(),
                label: current.clone(),
                description: "Custom model".into(),
                description_for_model: None,
            });
        }
    }

    filter_by_allowlist(options, ctx.available_models.as_deref())
}

/// Base model options by user tier.
fn get_model_options_base(ctx: &OptionsContext) -> Vec<ModelOption> {
    let mut options = vec![get_default_option(ctx)];

    if ctx.is_ant {
        // Ant users get all options.
        options.push(opus_option(ctx));
        options.push(opus_1m_option(ctx));
        options.push(sonnet_option(ctx));
        options.push(sonnet_1m_option(ctx));
        options.push(haiku_option());
        return options;
    }

    if ctx.is_max_or_team_premium {
        // Max / Team Premium: Opus is default, show alternatives.
        if !ctx.opus_1m_merge_enabled && ctx.has_opus_1m {
            options.push(opus_1m_option(ctx));
        }
        options.push(sonnet_option(ctx));
        if ctx.has_sonnet_1m {
            options.push(sonnet_1m_option(ctx));
        }
        options.push(haiku_option());
        return options;
    }

    if ctx.is_subscriber {
        // Pro / Team Standard: Sonnet is default, show Opus as alternative.
        if ctx.has_sonnet_1m {
            options.push(sonnet_1m_option(ctx));
        }
        if ctx.opus_1m_merge_enabled {
            options.push(opus_1m_option(ctx));
        } else {
            options.push(opus_option(ctx));
            if ctx.has_opus_1m {
                options.push(opus_1m_option(ctx));
            }
        }
        options.push(haiku_option());
        return options;
    }

    // PAYG: Default (Sonnet) + Sonnet 1M + Opus + Opus 1M + Haiku
    if ctx.has_sonnet_1m {
        options.push(sonnet_1m_option(ctx));
    }
    if ctx.opus_1m_merge_enabled {
        options.push(opus_1m_option(ctx));
    } else {
        options.push(opus_option(ctx));
        if ctx.has_opus_1m {
            options.push(opus_1m_option(ctx));
        }
    }
    options.push(haiku_option());
    options
}

// ── Allowlist filtering ──────────────────────────────────────────────────

/// Filter model options by the available models allowlist.
/// Always preserves the "Default" option (empty value).
fn filter_by_allowlist(
    options: Vec<ModelOption>,
    allowlist: Option<&[String]>,
) -> Vec<ModelOption> {
    match allowlist {
        None => options,
        Some(list) => options
            .into_iter()
            .filter(|opt| opt.value.is_empty() || is_model_allowed(&opt.value, Some(list)))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_option_label() {
        let ctx = OptionsContext::default();
        let opt = get_default_option(&ctx);
        assert_eq!(opt.label, "Default (recommended)");
        assert!(opt.value.is_empty());
    }

    #[test]
    fn payg_options() {
        let ctx = OptionsContext::default();
        let options = get_model_options(&ctx);
        assert!(!options.is_empty());
        // Default + Opus + Haiku at minimum.
        assert!(options.len() >= 3);
        assert_eq!(options[0].label, "Default (recommended)");
    }

    #[test]
    fn max_subscriber_gets_opus_default() {
        let ctx = OptionsContext {
            is_max_or_team_premium: true,
            ..Default::default()
        };
        let options = get_model_options(&ctx);
        assert!(!options.is_empty());
        // Should have Sonnet as alternative.
        assert!(options.iter().any(|o| o.label == "Sonnet"));
    }

    #[test]
    fn subscriber_with_1m() {
        let ctx = OptionsContext {
            is_subscriber: true,
            has_opus_1m: true,
            has_sonnet_1m: true,
            ..Default::default()
        };
        let options = get_model_options(&ctx);
        assert!(options.iter().any(|o| o.label.contains("1M")));
    }

    #[test]
    fn custom_model_added() {
        let ctx = OptionsContext {
            custom_model: Some("my-custom-model-v1".into()),
            ..Default::default()
        };
        let options = get_model_options(&ctx);
        assert!(options.iter().any(|o| o.value == "my-custom-model-v1"));
    }

    #[test]
    fn allowlist_filters_options() {
        let allowlist = vec!["claude-opus-4-6".to_string()];
        let ctx = OptionsContext {
            available_models: Some(allowlist),
            ..Default::default()
        };
        let options = get_model_options(&ctx);
        // Default option should always be preserved.
        assert!(options.iter().any(|o| o.value.is_empty()));
        // Only allowed models should remain (plus default).
        for opt in &options {
            if !opt.value.is_empty() {
                assert!(is_model_allowed(
                    &opt.value,
                    ctx.available_models.as_deref()
                ));
            }
        }
    }

    #[test]
    fn additional_options_appended() {
        let additional = vec![ModelOption {
            value: "extra-model".into(),
            label: "Extra".into(),
            description: "An extra option".into(),
            description_for_model: None,
        }];
        let ctx = OptionsContext {
            additional_options: additional,
            ..Default::default()
        };
        let options = get_model_options(&ctx);
        assert!(options.iter().any(|o| o.value == "extra-model"));
    }

    #[test]
    fn ant_user_gets_all_options() {
        let ctx = OptionsContext {
            is_ant: true,
            ..Default::default()
        };
        let options = get_model_options(&ctx);
        assert!(options.iter().any(|o| o.label == "Opus"));
        assert!(options.iter().any(|o| o.label == "Sonnet"));
        assert!(options.iter().any(|o| o.label == "Haiku"));
    }

    #[test]
    fn test_opus_plan_option() {
        let opt = opus_plan_option();
        assert_eq!(opt.value, "opusplan");
        assert_eq!(opt.label, "Opus Plan Mode");
    }

    #[test]
    fn no_duplicate_custom_model() {
        let ctx = OptionsContext {
            custom_model: Some("sonnet".into()),
            ..Default::default()
        };
        let options = get_model_options(&ctx);
        let sonnet_count = options.iter().filter(|o| o.value == "sonnet").count();
        assert!(sonnet_count <= 1);
    }
}
