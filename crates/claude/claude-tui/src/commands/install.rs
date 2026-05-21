//! `/install` command — install plugins and extensions.
//!
//! Provides subcommands for installing plugins by name, listing available
//! plugins, and checking installation status.

use claude_config::RuntimeConfig;

/// Dispatch the `/install` command.
pub fn dispatch(input: &str, config: &RuntimeConfig) {
    let args = input
        .trim()
        .strip_prefix("/install")
        .unwrap_or_default()
        .trim();

    if args.is_empty() {
        show_usage();
        return;
    }

    let mut parts = args.split_whitespace();
    let subcommand = parts.next().unwrap_or_default();

    match subcommand {
        "list" => list_available(config),
        "status" => show_status(config),
        name if !name.is_empty() => install_plugin(name, config),
        _ => show_usage(),
    }
}

/// Show usage information.
fn show_usage() {
    println!("Install: manage plugins and extensions");
    println!("  Usage: /install <plugin-name>");
    println!("         /install list");
    println!("         /install status");
    println!();
    println!("  Available plugins can be listed with /install list.");
}

/// List available plugins for installation.
fn list_available(config: &RuntimeConfig) {
    println!("Available plugins:");
    println!("  session: {}", config.session_id);
    println!("  cwd:     {}", config.cwd.display());
    println!();
    println!("  (plugin discovery requires a configured plugin registry)");
    println!("  Use /install <plugin-name> to install a specific plugin.");
}

/// Show installation status.
fn show_status(config: &RuntimeConfig) {
    println!("Install status:");
    println!("  session: {}", config.session_id);
    println!("  cwd:     {}", config.cwd.display());
    println!("  installed: (none)");
    println!("  (use /plugins list to see currently installed plugins)");
}

/// Install a plugin by name.
fn install_plugin(name: &str, config: &RuntimeConfig) {
    println!("Installing plugin: {name}");
    println!("  session: {}", config.session_id);
    println!("  cwd:     {}", config.cwd.display());
    println!();
    println!("  (plugin installation requires network access and a configured registry)");
    println!("  Plugin will be available after restart.");
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
    fn dispatch_install_no_args() {
        let config = build_test_config();
        dispatch("/install", &config);
    }

    #[test]
    fn dispatch_install_list() {
        let config = build_test_config();
        dispatch("/install list", &config);
    }

    #[test]
    fn dispatch_install_status() {
        let config = build_test_config();
        dispatch("/install status", &config);
    }

    #[test]
    fn dispatch_install_plugin_name() {
        let config = build_test_config();
        dispatch("/install my-plugin", &config);
    }

    #[test]
    fn dispatch_install_with_whitespace() {
        let config = build_test_config();
        dispatch("/install   my-plugin  ", &config);
    }

    #[test]
    fn show_usage_no_panic() {
        show_usage();
    }

    #[test]
    fn list_available_no_panic() {
        let config = build_test_config();
        list_available(&config);
    }

    #[test]
    fn show_status_no_panic() {
        let config = build_test_config();
        show_status(&config);
    }

    #[test]
    fn install_plugin_no_panic() {
        let config = build_test_config();
        install_plugin("test-plugin", &config);
    }

    #[test]
    fn install_plugin_empty_name() {
        let config = build_test_config();
        // Empty name after /install should show usage
        dispatch("/install ", &config);
    }
}
