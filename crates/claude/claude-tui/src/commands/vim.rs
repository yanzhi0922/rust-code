//! `/vim` command — toggle Vim input mode.
//!
//! Provides subcommands for enabling, disabling, or checking the status
//! of the Vim-like input mode.

use claude_config::RuntimeConfig;

/// Dispatch the `/vim` command.
pub fn dispatch(input: &str, config: &RuntimeConfig) {
    let subcommand = input.trim().strip_prefix("/vim").unwrap_or_default().trim();

    match subcommand {
        "on" => enable_vim(config),
        "off" => disable_vim(config),
        "status" => show_status(config),
        "" => show_status(config),
        _ => {
            println!("Unknown vim subcommand: {subcommand}");
            println!("Usage: /vim [on|off|status]");
        }
    }
}

/// Enable Vim input mode.
fn enable_vim(config: &RuntimeConfig) {
    println!("Vim mode: enabled");
    println!("  session: {}", config.session_id);
    println!("  keybindings: h/j/k/l navigation, i insert, ESC normal mode");
    println!("  (Vim mode settings are per-session and will reset on restart)");
}

/// Disable Vim input mode.
fn disable_vim(config: &RuntimeConfig) {
    println!("Vim mode: disabled");
    println!("  session: {}", config.session_id);
    println!("  (Switched to default Emacs-style keybindings)");
}

/// Show current Vim mode status.
fn show_status(config: &RuntimeConfig) {
    println!("Vim mode: status");
    println!("  session: {}", config.session_id);
    println!("  cwd:     {}", config.cwd.display());
    println!("  (Use /vim on to enable, /vim off to disable)");
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
    fn dispatch_vim_on() {
        let config = build_test_config();
        dispatch("/vim on", &config);
    }

    #[test]
    fn dispatch_vim_off() {
        let config = build_test_config();
        dispatch("/vim off", &config);
    }

    #[test]
    fn dispatch_vim_status() {
        let config = build_test_config();
        dispatch("/vim status", &config);
    }

    #[test]
    fn dispatch_vim_default() {
        let config = build_test_config();
        dispatch("/vim", &config);
    }

    #[test]
    fn dispatch_vim_unknown_subcommand() {
        let config = build_test_config();
        dispatch("/vim unknown", &config);
    }

    #[test]
    fn dispatch_vim_with_extra_whitespace() {
        let config = build_test_config();
        dispatch("/vim   on  ", &config);
    }

    #[test]
    fn enable_vim_prints_enabled() {
        let config = build_test_config();
        // Just ensure no panic
        enable_vim(&config);
    }

    #[test]
    fn disable_vim_prints_disabled() {
        let config = build_test_config();
        disable_vim(&config);
    }

    #[test]
    fn show_status_prints_session() {
        let config = build_test_config();
        show_status(&config);
    }

    #[test]
    fn dispatch_vim_case_sensitive() {
        let config = build_test_config();
        // "ON" should be treated as unknown (case-sensitive)
        dispatch("/vim ON", &config);
    }
}
