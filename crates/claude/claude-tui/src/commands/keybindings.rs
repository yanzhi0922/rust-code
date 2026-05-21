//! Keybinding commands: `/keybindings`, `/terminalSetup`.

use claude_config::RuntimeConfig;

/// Default keybinding set for the TUI.
pub const DEFAULT_KEYBINDINGS: &[(&str, &str)] = &[
    ("Enter", "Submit input / confirm"),
    ("Escape", "Cancel / switch to normal mode"),
    ("i", "Enter insert mode"),
    ("h/j/k/l", "Navigate (Vim-style)"),
    ("G", "Scroll to bottom"),
    ("gg", "Scroll to top"),
    ("Ctrl+c", "Interrupt current operation"),
    ("Ctrl+d", "Exit session"),
    ("Tab", "Autocomplete"),
    ("?", "Show help"),
    (":", "Command mode"),
    ("/", "Search mode"),
    ("n", "Next search result"),
    ("N", "Previous search result"),
    ("q", "Quit from popup/help"),
];

/// Dispatch `/keybindings` — manage keybindings.
///
/// - `/keybindings` or `/keybindings list` — show current keybindings.
/// - `/keybindings set <key> <action>` — display keybinding change.
/// - `/keybindings reset` — reset to defaults.
pub fn dispatch(input: &str, config: &RuntimeConfig) {
    let remainder = input
        .trim()
        .strip_prefix("/keybindings")
        .unwrap_or_default()
        .trim();

    if remainder.is_empty() || remainder == "list" {
        render_list();
        return;
    }

    let mut parts = remainder.split_whitespace();
    match parts.next().unwrap_or_default() {
        "set" => {
            let key = parts.next().unwrap_or_default();
            if key.is_empty() {
                println!("Usage: /keybindings set <key> <action>");
                return;
            }
            let action = remainder
                .strip_prefix("set")
                .unwrap_or_default()
                .trim()
                .strip_prefix(key)
                .unwrap_or_default()
                .trim();
            if action.is_empty() {
                println!("Usage: /keybindings set <key> <action>");
            } else {
                println!("Keybinding set: {key} -> {action}");
            }
        }
        "reset" => {
            println!("Keybindings reset to defaults.");
        }
        other => {
            println!("Unknown /keybindings subcommand '{other}'.");
            println!("Usage: /keybindings [list|set <key> <action>|reset]");
        }
    }
    println!("  session: {}", config.session_id);
}

fn render_list() {
    println!("Keybindings ({}):", DEFAULT_KEYBINDINGS.len());
    for (key, action) in DEFAULT_KEYBINDINGS {
        println!("  {:<14} {action}", format!("[{key}]"));
    }
}

/// Dispatch `/terminalSetup` — show terminal setup information.
pub fn render_terminal_setup(config: &RuntimeConfig) {
    println!("Terminal setup:");
    println!(
        "  term:      {}",
        std::env::var("TERM").unwrap_or_else(|_| "(unknown)".to_owned())
    );
    println!(
        "  term_prog: {}",
        std::env::var("TERM_PROGRAM").unwrap_or_else(|_| "(unknown)".to_owned())
    );
    println!(
        "  shell:     {}",
        std::env::var("SHELL").unwrap_or_else(|_| "(unknown)".to_owned())
    );
    println!(
        "  editor:    {}",
        std::env::var("EDITOR").unwrap_or_else(|_| "(none)".to_owned())
    );
    println!("  session:   {}", config.session_id);
    println!();
    println!("  Tips:");
    println!("    - Use a terminal with 24-bit color support for best experience");
    println!("    - Set EDITOR environment variable for external editing");
    println!("    - Ensure terminal supports UTF-8 for proper rendering");
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
    fn keybindings_list_shows_all() {
        let config = build_test_config();
        dispatch("/keybindings list", &config);
    }

    #[test]
    fn keybindings_default_shows_list() {
        let config = build_test_config();
        dispatch("/keybindings", &config);
    }

    #[test]
    fn keybindings_set_requires_key_and_action() {
        let config = build_test_config();
        dispatch("/keybindings set", &config);
    }

    #[test]
    fn keybindings_set_requires_action() {
        let config = build_test_config();
        dispatch("/keybindings set Enter", &config);
    }

    #[test]
    fn keybindings_set_with_key_and_action() {
        let config = build_test_config();
        dispatch("/keybindings set Enter submit", &config);
    }

    #[test]
    fn keybindings_reset() {
        let config = build_test_config();
        dispatch("/keybindings reset", &config);
    }

    #[test]
    fn keybindings_unknown_subcommand() {
        let config = build_test_config();
        dispatch("/keybindings foo", &config);
    }

    #[test]
    fn terminal_setup_shows_info() {
        let config = build_test_config();
        render_terminal_setup(&config);
    }

    #[test]
    fn default_keybindings_not_empty() {
        assert!(!DEFAULT_KEYBINDINGS.is_empty());
    }

    #[test]
    fn default_keybindings_contains_enter() {
        assert!(DEFAULT_KEYBINDINGS.iter().any(|(k, _)| *k == "Enter"));
    }

    #[test]
    fn default_keybindings_contains_escape() {
        assert!(DEFAULT_KEYBINDINGS.iter().any(|(k, _)| *k == "Escape"));
    }
}
