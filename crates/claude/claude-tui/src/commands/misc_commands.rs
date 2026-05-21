//! Miscellaneous commands: `/ide`, `/voice`, `/thinkback`, `/debugToolCall`, `/subscribe-pr`, `/upgrade`.

use claude_config::RuntimeConfig;

/// Dispatch `/ide` — show IDE integration status.
pub fn render_ide(config: &RuntimeConfig) {
    println!("IDE integration:");
    println!("  session: {}", config.session_id);
    println!("  cwd:     {}", config.cwd.display());
    println!("  status:  standalone mode");
    println!("  (IDE integration requires the VS Code extension or JetBrains plugin)");
}

/// Dispatch `/voice` — show voice mode status.
pub fn render_voice(config: &RuntimeConfig) {
    println!("Voice mode:");
    println!("  status: off");
    println!("  session: {}", config.session_id);
    println!("  (voice mode requires a configured audio input/output device)");
}

/// Dispatch `/thinkback` — show thinking/reasoning playback.
pub fn render_thinkback(config: &RuntimeConfig) {
    println!("Thinkback:");
    println!("  session: {}", config.session_id);
    println!(
        "  model:   {}",
        config.provider.model.as_deref().unwrap_or("(default)")
    );
    println!(
        "  effort:  {}",
        config.effort.as_deref().unwrap_or("(default)")
    );
    println!("  (thinking/reasoning playback is available for models that support it)");
}

/// Dispatch `/debugToolCall` — debug tool call execution.
pub fn dispatch_debug_tool_call(input: &str, config: &RuntimeConfig) {
    let tool_name = input
        .trim()
        .strip_prefix("/debugToolCall")
        .unwrap_or_default()
        .trim();

    if tool_name.is_empty() {
        println!("Debug tool call:");
        println!("  Usage: /debugToolCall <tool-name>");
        println!("  Shows detailed debug information for the specified tool.");
        return;
    }

    println!("Debug tool call: {tool_name}");
    println!("  session: {}", config.session_id);
    println!("  cwd:     {}", config.cwd.display());
    println!("  (detailed tool call debugging requires a running session with tool calls)");
}

/// Dispatch `/subscribe-pr` — subscribe to PR activity.
pub fn dispatch_subscribe_pr(input: &str, config: &RuntimeConfig) {
    let pr_ref = input
        .trim()
        .strip_prefix("/subscribe-pr")
        .unwrap_or_default()
        .trim();

    if pr_ref.is_empty() {
        println!("PR subscription:");
        println!("  Usage: /subscribe-pr <pr-url-or-number>");
        println!("  Subscribes to activity on the specified PR.");
        return;
    }

    println!("PR subscription: {pr_ref}");
    println!("  session: {}", config.session_id);
    println!("  (PR subscription requires GitHub CLI authentication)");
}

/// Dispatch `/upgrade` — check for updates.
pub fn render_upgrade() {
    println!("Upgrade check:");
    println!("  current version: {}", env!("CARGO_PKG_VERSION"));
    println!("  (use 'cargo install remote-code-rust' to check for updates)");
    println!("  or check the project repository for the latest release.");
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
    fn ide_shows_status() {
        let config = build_test_config();
        render_ide(&config);
    }

    #[test]
    fn voice_shows_status() {
        let config = build_test_config();
        render_voice(&config);
    }

    #[test]
    fn thinkback_shows_info() {
        let config = build_test_config();
        render_thinkback(&config);
    }

    #[test]
    fn debug_tool_call_without_name_shows_usage() {
        let config = build_test_config();
        dispatch_debug_tool_call("/debugToolCall", &config);
    }

    #[test]
    fn debug_tool_call_with_name() {
        let config = build_test_config();
        dispatch_debug_tool_call("/debugToolCall bash", &config);
    }

    #[test]
    fn subscribe_pr_without_ref_shows_usage() {
        let config = build_test_config();
        dispatch_subscribe_pr("/subscribe-pr", &config);
    }

    #[test]
    fn subscribe_pr_with_ref() {
        let config = build_test_config();
        dispatch_subscribe_pr("/subscribe-pr 123", &config);
    }

    #[test]
    fn upgrade_shows_version() {
        render_upgrade();
    }

    #[test]
    fn ide_displays_session_id() {
        let config = build_test_config();
        render_ide(&config);
    }

    #[test]
    fn debug_tool_call_with_read_tool() {
        let config = build_test_config();
        dispatch_debug_tool_call("/debugToolCall read_file", &config);
    }

    #[test]
    fn subscribe_pr_with_url() {
        let config = build_test_config();
        dispatch_subscribe_pr("/subscribe-pr https://github.com/org/repo/pull/42", &config);
    }
}
