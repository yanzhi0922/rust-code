//! `/privacy` command — show privacy settings.

use claude_config::RuntimeConfig;

/// Dispatch the `/privacy` command.
pub fn render(config: &RuntimeConfig) {
    println!("Privacy Settings");
    println!("────────────────");
    println!(
        "Verbose logging:      {}",
        if config.verbose { "Yes" } else { "No" }
    );
    println!(
        "Setting sources:      {}",
        if config.setting_sources.is_empty() {
            "None".to_owned()
        } else {
            config.setting_sources.join(", ")
        }
    );
    println!("Allowed tools:        {}", config.allowed_tools.len());
    println!("Disallowed tools:     {}", config.disallowed_tools.len());
    println!(
        "Permission mode:      {}",
        config.permission_mode.as_legacy_str()
    );
    println!();
    println!("Privacy controls:");
    println!("  /config set verbose false            - Disable verbose logging");
    println!("  /permissions deny <tool>              - Deny a specific tool");
    println!("  /permissions reset                    - Reset to default rules");
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
    fn render_no_panic() {
        let config = build_test_config();
        render(&config);
    }
}
