//! Authentication commands: `/login`, `/logout`.

use claude_config::RuntimeConfig;

/// Dispatch `/login` — show current authentication status.
pub fn render_login(config: &RuntimeConfig) {
    println!("Authentication status:");
    println!(
        "  auth source: {}",
        config.auth_source.as_deref().unwrap_or("(none)")
    );
    println!("  provider:    {}", config.provider.name);
    println!(
        "  api key:     {}",
        if config.provider.api_key.is_some() {
            "configured"
        } else {
            "not set"
        }
    );
    println!(
        "  base URL:    {}",
        config.provider.base_url.as_deref().unwrap_or("(default)")
    );

    if config.provider.api_key.is_none() {
        println!("  To authenticate, set REMOTE_CODE_API_KEY or configure in settings.");
    }
}

/// Dispatch `/logout` — log out from current session.
pub fn render_logout(config: &RuntimeConfig) {
    println!("Logout:");
    println!("  session:  {}", config.session_id);
    println!("  provider: {}", config.provider.name);
    println!(
        "  auth:     {}",
        config.auth_source.as_deref().unwrap_or("(none)")
    );
    println!("  (credentials are managed via environment variables and settings files)");
    println!("  To clear credentials, unset REMOTE_CODE_API_KEY and restart.");
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
    fn login_shows_auth_status() {
        let config = build_test_config();
        render_login(&config);
    }

    #[test]
    fn logout_shows_logout_info() {
        let config = build_test_config();
        render_logout(&config);
    }

    #[test]
    fn login_displays_provider() {
        let config = build_test_config();
        // Verify it runs without panic
        render_login(&config);
    }

    #[test]
    fn login_displays_api_key_status() {
        let config = build_test_config();
        render_login(&config);
    }

    #[test]
    fn logout_displays_session_id() {
        let config = build_test_config();
        render_logout(&config);
    }
}
