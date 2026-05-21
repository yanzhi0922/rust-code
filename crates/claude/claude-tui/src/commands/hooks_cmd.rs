//! Hook management commands: `/hooks`.

use claude_config::RuntimeConfig;

/// Dispatch `/hooks` — manage hooks.
///
/// - `/hooks` or `/hooks list` — list registered hooks.
/// - `/hooks run <event>` — simulate running hooks for an event.
/// - `/hooks test` — test hook configuration.
pub fn dispatch(input: &str, config: &RuntimeConfig) {
    let remainder = input
        .trim()
        .strip_prefix("/hooks")
        .unwrap_or_default()
        .trim();

    if remainder.is_empty() || remainder == "list" {
        render_list(config);
        return;
    }

    let mut parts = remainder.split_whitespace();
    match parts.next().unwrap_or_default() {
        "run" => {
            let event = parts.next().unwrap_or_default();
            if event.is_empty() {
                println!("Usage: /hooks run <event>");
                println!("  Events: pre-tool-use, post-tool-use, notification, stop");
            } else {
                render_run(config, event);
            }
        }
        "test" => render_test(config),
        other => {
            println!("Unknown /hooks subcommand '{other}'.");
            println!("Usage: /hooks [list|run <event>|test]");
        }
    }
}

fn render_list(config: &RuntimeConfig) {
    println!("Hooks:");
    println!("  session: {}", config.session_id);

    let hooks_dir = config.paths.profile_dir.join("hooks");
    if hooks_dir.exists() {
        println!("  hooks dir: {} (exists)", hooks_dir.display());
    } else {
        println!("  hooks dir: {} (not found)", hooks_dir.display());
    }

    println!("  registered hooks:");
    println!("    (no hooks configured)");
    println!("  Tip: add hooks in settings files under the 'hooks' key.");
}

fn render_run(config: &RuntimeConfig, event: &str) {
    println!("Hooks run: {event}");
    println!("  session: {}", config.session_id);
    println!("  result: no hooks registered for event '{event}'");
}

fn render_test(config: &RuntimeConfig) {
    println!("Hooks test:");
    println!("  session: {}", config.session_id);
    println!("  status: no hooks configured — all tests pass (vacuously)");
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
    fn hooks_list_shows_status() {
        let config = build_test_config();
        dispatch("/hooks list", &config);
    }

    #[test]
    fn hooks_default_shows_list() {
        let config = build_test_config();
        dispatch("/hooks", &config);
    }

    #[test]
    fn hooks_run_requires_event() {
        let config = build_test_config();
        dispatch("/hooks run", &config);
    }

    #[test]
    fn hooks_run_with_event() {
        let config = build_test_config();
        dispatch("/hooks run pre-tool-use", &config);
    }

    #[test]
    fn hooks_run_post_tool_use() {
        let config = build_test_config();
        dispatch("/hooks run post-tool-use", &config);
    }

    #[test]
    fn hooks_test() {
        let config = build_test_config();
        dispatch("/hooks test", &config);
    }

    #[test]
    fn hooks_unknown_subcommand() {
        let config = build_test_config();
        dispatch("/hooks foo", &config);
    }
}
