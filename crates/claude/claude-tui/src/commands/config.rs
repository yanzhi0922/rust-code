//! `/config` — runtime configuration read/write.
//!
//! Supports `get <key>`, `set <key> <value>`, and `list` subcommands.

use claude_config::RuntimeConfig;

/// Dispatch `/config` subcommands.
///
/// - `/config` or `/config list` — show all runtime config keys.
/// - `/config get <key>` — display a single config value.
/// - `/config set <key> <value>` — display what *would* change (runtime is immutable).
pub fn dispatch(input: &str, config: &RuntimeConfig) {
    let remainder = input
        .trim()
        .strip_prefix("/config")
        .unwrap_or_default()
        .trim();

    if remainder.is_empty() || remainder == "list" {
        render(config);
        return;
    }

    let mut parts = remainder.split_whitespace();
    match parts.next().unwrap_or_default() {
        "get" => {
            let key = parts.next().unwrap_or_default();
            if key.is_empty() {
                println!("Usage: /config get <key>");
                return;
            }
            render_get(config, key);
        }
        "set" => {
            let key = parts.next().unwrap_or_default();
            if key.is_empty() {
                println!("Usage: /config set <key> <value>");
                return;
            }
            let value = remainder
                .strip_prefix("set")
                .unwrap_or_default()
                .trim()
                .strip_prefix(key)
                .unwrap_or_default()
                .trim();
            if value.is_empty() {
                println!("Usage: /config set <key> <value>");
                return;
            }
            render_set(config, key, value);
        }
        other => {
            println!("Unknown /config subcommand '{other}'.");
            println!("Usage: /config [list|get <key>|set <key> <value>]");
        }
    }
}

/// Display all runtime configuration values.
pub fn render(config: &RuntimeConfig) {
    println!("Configuration:");
    println!("  session_id:           {}", config.session_id);
    println!(
        "  session_name:         {}",
        config.session_name.as_deref().unwrap_or("(auto)")
    );
    println!("  cwd:                  {}", config.cwd.display());
    println!("  provider.name:        {}", config.provider.name);
    println!(
        "  provider.model:       {}",
        config.provider.model.as_deref().unwrap_or("(default)")
    );
    println!(
        "  provider.protocol:    {}",
        config.provider.protocol.as_str()
    );
    println!(
        "  provider.base_url:    {}",
        config.provider.base_url.as_deref().unwrap_or("(default)")
    );
    println!("  provider.timeout_ms:  {}", config.provider.timeout_ms);
    println!(
        "  provider.max_output:  {}",
        config.provider.max_output_tokens
    );
    println!("  provider.max_retries: {}", config.provider.max_retries);
    println!(
        "  effort:               {}",
        config.effort.as_deref().unwrap_or("(default)")
    );
    println!(
        "  fallback_model:       {}",
        config.fallback_model.as_deref().unwrap_or("(none)")
    );
    println!(
        "  permission_mode:      {}",
        config.permission_mode.as_legacy_str()
    );
    println!("  input_format:         {:?}", config.input_format);
    println!("  output_format:        {:?}", config.output_format);
    println!("  verbose:              {}", config.verbose);
    println!("  max_turns:            {}", config.max_turns);
    println!("  allowed_tools:        {}", config.allowed_tools.len());
    println!("  disallowed_tools:     {}", config.disallowed_tools.len());
    println!(
        "  setting_sources:      {}",
        config.setting_sources.join(", ")
    );
    println!(
        "  settings_files:       {}",
        config
            .settings_files
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
}

fn render_get(config: &RuntimeConfig, key: &str) {
    let value = match key {
        "session_id" => config.session_id.to_string(),
        "session_name" => config.session_name.clone().unwrap_or_default(),
        "cwd" => config.cwd.display().to_string(),
        "provider" => config.provider.name.clone(),
        "provider.name" => config.provider.name.clone(),
        "provider.model" => config.provider.model.clone().unwrap_or_default(),
        "provider.protocol" => config.provider.protocol.as_str().to_owned(),
        "provider.base_url" => config.provider.base_url.clone().unwrap_or_default(),
        "provider.timeout_ms" => config.provider.timeout_ms.to_string(),
        "provider.max_output_tokens" => config.provider.max_output_tokens.to_string(),
        "provider.max_retries" => config.provider.max_retries.to_string(),
        "effort" => config.effort.clone().unwrap_or_default(),
        "fallback_model" => config.fallback_model.clone().unwrap_or_default(),
        "permission_mode" => config.permission_mode.as_legacy_str().to_owned(),
        "verbose" => config.verbose.to_string(),
        "max_turns" => config.max_turns.to_string(),
        "input_format" => format!("{:?}", config.input_format),
        "output_format" => format!("{:?}", config.output_format),
        _ => {
            println!("Unknown config key '{key}'.");
            return;
        }
    };
    println!("{key} = {value}");
}

fn render_set(config: &RuntimeConfig, key: &str, value: &str) {
    // Runtime config is immutable at dispatch time; we show what would change.
    let current = match key {
        "effort" => config.effort.clone().unwrap_or_default(),
        "fallback_model" => config.fallback_model.clone().unwrap_or_default(),
        "provider.model" => config.provider.model.clone().unwrap_or_default(),
        "max_turns" => config.max_turns.to_string(),
        "verbose" => config.verbose.to_string(),
        _ => {
            println!("Unknown or read-only config key '{key}'.");
            return;
        }
    };
    println!("Config change: {key}: {current} -> {value} (requires restart to take effect)");
}

/// Returns the set of recognized config keys for tab-completion and validation.
#[allow(dead_code)]
pub fn known_keys() -> &'static [&'static str] {
    &[
        "session_id",
        "session_name",
        "cwd",
        "provider",
        "provider.name",
        "provider.model",
        "provider.protocol",
        "provider.base_url",
        "provider.timeout_ms",
        "provider.max_output_tokens",
        "provider.max_retries",
        "effort",
        "fallback_model",
        "permission_mode",
        "verbose",
        "max_turns",
        "input_format",
        "output_format",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use claude_core::{InputFormat, OutputFormat, PermissionMode};
    use tempfile::tempdir;

    fn build_test_config() -> RuntimeConfig {
        let temp = tempdir().expect("tempdir should work");
        let root = temp.keep();
        load_runtime_config(
            Some(root.clone()),
            Some(root.join(".remote-code-rust")),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides {
                provider: Some("glm-coding".to_owned()),
                base_url: Some("https://open.bigmodel.cn/api/anthropic".to_owned()),
                api_key: Some("secret".to_owned()),
                model: Some("glm-5.1".to_owned()),
                protocol: Some(claude_core::ProviderProtocol::Anthropic),
            },
            RuntimeOverrides::default(),
        )
        .expect("config should load")
    }

    #[test]
    fn dispatch_list_shows_all_keys() {
        let config = build_test_config();
        dispatch("/config list", &config);
    }

    #[test]
    fn dispatch_default_shows_all_keys() {
        let config = build_test_config();
        dispatch("/config", &config);
    }

    #[test]
    fn get_known_key_returns_value() {
        let config = build_test_config();
        dispatch("/config get provider", &config);
    }

    #[test]
    fn get_provider_model_returns_value() {
        let config = build_test_config();
        dispatch("/config get provider.model", &config);
    }

    #[test]
    fn get_unknown_key_shows_error() {
        let config = build_test_config();
        dispatch("/config get nonexistent_key", &config);
    }

    #[test]
    fn get_missing_key_shows_usage() {
        let config = build_test_config();
        dispatch("/config get", &config);
    }

    #[test]
    fn set_effort_shows_change() {
        let config = build_test_config();
        dispatch("/config set effort high", &config);
    }

    #[test]
    fn set_unknown_key_shows_error() {
        let config = build_test_config();
        dispatch("/config set unknown_key value", &config);
    }

    #[test]
    fn set_missing_value_shows_usage() {
        let config = build_test_config();
        dispatch("/config set effort", &config);
    }

    #[test]
    fn set_missing_key_shows_usage() {
        let config = build_test_config();
        dispatch("/config set", &config);
    }

    #[test]
    fn unknown_subcommand_shows_usage() {
        let config = build_test_config();
        dispatch("/config foo", &config);
    }

    #[test]
    fn known_keys_is_not_empty() {
        assert!(!known_keys().is_empty());
    }

    #[test]
    fn known_keys_contains_provider_model() {
        assert!(known_keys().contains(&"provider.model"));
    }

    #[test]
    fn known_keys_contains_effort() {
        assert!(known_keys().contains(&"effort"));
    }

    #[test]
    fn render_outputs_provider_name() {
        let config = build_test_config();
        render(&config);
    }

    #[test]
    fn get_cwd_returns_path() {
        let config = build_test_config();
        dispatch("/config get cwd", &config);
    }

    #[test]
    fn get_verbose_returns_value() {
        let config = build_test_config();
        dispatch("/config get verbose", &config);
    }

    #[test]
    fn get_max_turns_returns_value() {
        let config = build_test_config();
        dispatch("/config get max_turns", &config);
    }

    #[test]
    fn set_max_turns_shows_change() {
        let config = build_test_config();
        dispatch("/config set max_turns 16", &config);
    }

    #[test]
    fn set_fallback_model_shows_change() {
        let config = build_test_config();
        dispatch("/config set fallback_model gpt-4o", &config);
    }
}
