//! Remote commands: `/remoteControlServer`, `/remote-setup`.

use claude_config::RuntimeConfig;

/// Dispatch `/remoteControlServer` — show remote control server status.
pub fn render_remote_control_server(config: &RuntimeConfig) {
    println!("Remote control server:");
    println!("  session: {}", config.session_id);
    println!("  cwd:     {}", config.cwd.display());
    println!("  status:  not running");
    println!("  (remote control server requires a configured control plane)");
    println!("  Use /remote-setup to configure remote access.");
}

/// Dispatch `/remote-setup` — remote setup wizard.
pub fn dispatch_remote_setup(input: &str, config: &RuntimeConfig) {
    let step = input
        .trim()
        .strip_prefix("/remote-setup")
        .unwrap_or_default()
        .trim();

    match step {
        "" => {
            println!("Remote setup wizard:");
            println!("  Step 1: Configure control plane URL");
            println!("  Step 2: Authenticate with the control plane");
            println!("  Step 3: Register this device");
            println!("  Step 4: Start remote control server");
            println!();
            println!("  Current configuration:");
            println!("    profile dir: {}", config.paths.profile_dir.display());
            println!("    provider:     {}", config.provider.name);
            println!(
                "    base URL:     {}",
                config.provider.base_url.as_deref().unwrap_or("(default)")
            );
            println!();
            println!("  Usage: /remote-setup [1|2|3|4]");
        }
        "1" => {
            println!("Step 1: Configure control plane URL");
            println!("  Set REMOTE_CODE_CONTROL_PLANE_URL in your environment or settings.");
        }
        "2" => {
            println!("Step 2: Authenticate");
            println!("  Use /login to authenticate with the control plane.");
        }
        "3" => {
            println!("Step 3: Register device");
            println!("  Device registration happens automatically on first connection.");
        }
        "4" => {
            println!("Step 4: Start server");
            println!("  Use the remote-code-control-plane binary to start the server.");
        }
        other => {
            println!("Unknown step '{other}'.");
            println!("Usage: /remote-setup [1|2|3|4]");
        }
    }
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
    fn remote_control_server_shows_status() {
        let config = build_test_config();
        render_remote_control_server(&config);
    }

    #[test]
    fn remote_setup_default_shows_wizard() {
        let config = build_test_config();
        dispatch_remote_setup("/remote-setup", &config);
    }

    #[test]
    fn remote_setup_step_1() {
        let config = build_test_config();
        dispatch_remote_setup("/remote-setup 1", &config);
    }

    #[test]
    fn remote_setup_step_2() {
        let config = build_test_config();
        dispatch_remote_setup("/remote-setup 2", &config);
    }

    #[test]
    fn remote_setup_step_3() {
        let config = build_test_config();
        dispatch_remote_setup("/remote-setup 3", &config);
    }

    #[test]
    fn remote_setup_step_4() {
        let config = build_test_config();
        dispatch_remote_setup("/remote-setup 4", &config);
    }

    #[test]
    fn remote_setup_unknown_step() {
        let config = build_test_config();
        dispatch_remote_setup("/remote-setup 99", &config);
    }
}
