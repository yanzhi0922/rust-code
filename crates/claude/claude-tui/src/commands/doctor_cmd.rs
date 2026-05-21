//! `/doctor` command — run diagnostic checks.
//!
//! Provides subcommands for running different diagnostic categories:
//! `full`, `quick`, `providers`, `mcp`, and `config`.

use claude_config::RuntimeConfig;

/// Dispatch the `/doctor` command.
pub fn dispatch(input: &str, config: &RuntimeConfig) {
    let subcommand = input
        .trim()
        .strip_prefix("/doctor")
        .unwrap_or_default()
        .trim();

    match subcommand {
        "full" => run_full(config),
        "quick" => run_quick(config),
        "providers" => run_providers(config),
        "mcp" => run_mcp(config),
        "config" => run_config(config),
        "" => run_quick(config),
        _ => {
            println!("Unknown doctor subcommand: {subcommand}");
            println!("Usage: /doctor [full|quick|providers|mcp|config]");
        }
    }
}

/// Run a full diagnostic check.
fn run_full(config: &RuntimeConfig) {
    println!("Doctor: full diagnostic");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    check_environment(config);
    check_providers(config);
    check_mcp(config);
    check_config(config);
    check_connectivity(config);

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Full diagnostic complete.");
}

/// Run a quick diagnostic check.
fn run_quick(config: &RuntimeConfig) {
    println!("Doctor: quick check");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    check_environment(config);
    check_providers(config);

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Quick check complete. Use /doctor full for more details.");
}

/// Run provider-specific diagnostics.
fn run_providers(config: &RuntimeConfig) {
    println!("Doctor: provider check");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    check_providers(config);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

/// Run MCP server diagnostics.
fn run_mcp(config: &RuntimeConfig) {
    println!("Doctor: MCP check");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    check_mcp(config);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

/// Run configuration diagnostics.
fn run_config(config: &RuntimeConfig) {
    println!("Doctor: config check");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    check_config(config);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

fn check_environment(config: &RuntimeConfig) {
    println!("[environment]");
    println!("  session:  {}", config.session_id);
    println!("  cwd:      {}", config.cwd.display());
    println!("  platform: {}", std::env::consts::OS);
    println!("  arch:     {}", std::env::consts::ARCH);
}

fn check_providers(config: &RuntimeConfig) {
    println!("[providers]");
    println!("  provider: {}", config.provider.name);
    println!(
        "  model:    {}",
        config.provider.model.as_deref().unwrap_or("(not set)")
    );
    println!(
        "  base_url: {}",
        config.provider.base_url.as_deref().unwrap_or("(not set)")
    );
    println!(
        "  api_key:  {}",
        if config.provider.api_key.is_some() {
            "configured"
        } else {
            "missing"
        }
    );
}

fn check_mcp(config: &RuntimeConfig) {
    println!("[mcp]");
    println!("  cwd: {}", config.cwd.display());
    println!("  (MCP server discovery requires a running session)");
}

fn check_config(config: &RuntimeConfig) {
    println!("[config]");
    println!("  profile_dir: {}", config.paths.profile_dir.display());
    println!("  permission_mode: {:?}", config.permission_mode);
    println!(
        "  effort: {}",
        config.effort.as_deref().unwrap_or("(default)")
    );
}

fn check_connectivity(config: &RuntimeConfig) {
    println!("[connectivity]");
    println!(
        "  base_url: {}",
        config.provider.base_url.as_deref().unwrap_or("(not set)")
    );
    println!("  (connectivity check requires a running session with network access)");
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
    fn dispatch_doctor_full() {
        let config = build_test_config();
        dispatch("/doctor full", &config);
    }

    #[test]
    fn dispatch_doctor_quick() {
        let config = build_test_config();
        dispatch("/doctor quick", &config);
    }

    #[test]
    fn dispatch_doctor_providers() {
        let config = build_test_config();
        dispatch("/doctor providers", &config);
    }

    #[test]
    fn dispatch_doctor_mcp() {
        let config = build_test_config();
        dispatch("/doctor mcp", &config);
    }

    #[test]
    fn dispatch_doctor_config() {
        let config = build_test_config();
        dispatch("/doctor config", &config);
    }

    #[test]
    fn dispatch_doctor_default() {
        let config = build_test_config();
        dispatch("/doctor", &config);
    }

    #[test]
    fn dispatch_doctor_unknown() {
        let config = build_test_config();
        dispatch("/doctor unknown", &config);
    }

    #[test]
    fn dispatch_doctor_with_whitespace() {
        let config = build_test_config();
        dispatch("/doctor   full  ", &config);
    }

    #[test]
    fn run_full_no_panic() {
        let config = build_test_config();
        run_full(&config);
    }

    #[test]
    fn run_quick_no_panic() {
        let config = build_test_config();
        run_quick(&config);
    }
}
