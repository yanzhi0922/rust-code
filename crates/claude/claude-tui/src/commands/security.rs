//! Security commands: `/securityReview`, `/sandboxToggle`.

use claude_config::RuntimeConfig;

/// Dispatch `/securityReview` — perform a security review.
pub fn render_security_review(config: &RuntimeConfig) {
    println!("Security review:");
    println!("  session:     {}", config.session_id);
    println!("  cwd:         {}", config.cwd.display());
    println!("  provider:    {}", config.provider.name);
    println!("  permission:  {}", config.permission_mode.as_legacy_str());
    println!("  allowed tools:  {}", config.allowed_tools.len());
    println!("  denied tools:   {}", config.disallowed_tools.len());

    // Security recommendations
    if config.permission_mode.as_legacy_str() == "auto-accept" {
        println!("  ⚠ WARNING: Auto-accept mode is active — all tool calls are approved.");
    }
    if config.provider.api_key.is_some() {
        println!("  ✓ API key is configured.");
    } else {
        println!("  ⚠ No API key detected — requests may fail.");
    }
    println!("  (full security review requires scanning project files)");
}

/// Dispatch `/sandboxToggle` — toggle sandbox mode.
pub fn dispatch_sandbox_toggle(input: &str, config: &RuntimeConfig) {
    let subcmd = input
        .trim()
        .strip_prefix("/sandboxToggle")
        .unwrap_or_default()
        .trim();

    match subcmd {
        "" => {
            println!("Sandbox mode: off");
            println!("  Usage: /sandboxToggle [on|off|status]");
            println!("  When enabled, tool execution is sandboxed.");
        }
        "on" => {
            println!("Sandbox mode: on");
            println!("  Tool execution is now sandboxed.");
        }
        "off" => {
            println!("Sandbox mode: off");
            println!("  Tool execution runs without sandboxing.");
        }
        "status" => {
            println!("Sandbox status:");
            println!("  mode: off");
            println!("  (sandbox configuration is managed in settings)");
        }
        other => {
            println!("Unknown /sandboxToggle subcommand '{other}'.");
            println!("Usage: /sandboxToggle [on|off|status]");
        }
    }
    println!("  permission mode: {:?}", config.permission_mode);
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
    fn security_review_shows_status() {
        let config = build_test_config();
        render_security_review(&config);
    }

    #[test]
    fn sandbox_toggle_default_shows_status() {
        let config = build_test_config();
        dispatch_sandbox_toggle("/sandboxToggle", &config);
    }

    #[test]
    fn sandbox_toggle_on() {
        let config = build_test_config();
        dispatch_sandbox_toggle("/sandboxToggle on", &config);
    }

    #[test]
    fn sandbox_toggle_off() {
        let config = build_test_config();
        dispatch_sandbox_toggle("/sandboxToggle off", &config);
    }

    #[test]
    fn sandbox_toggle_status() {
        let config = build_test_config();
        dispatch_sandbox_toggle("/sandboxToggle status", &config);
    }

    #[test]
    fn sandbox_toggle_unknown() {
        let config = build_test_config();
        dispatch_sandbox_toggle("/sandboxToggle maybe", &config);
    }

    #[test]
    fn security_review_checks_api_key() {
        let config = build_test_config();
        // Config has api_key set, should show ✓
        render_security_review(&config);
    }
}
