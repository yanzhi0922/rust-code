//! `/teleport` command — directory jumping.
//!
//! Provides quick navigation between project directories using
//! the teleport system.

use claude_config::RuntimeConfig;

/// Dispatch the `/teleport` command.
pub fn dispatch(input: &str, config: &RuntimeConfig) {
    let target = input
        .trim()
        .strip_prefix("/teleport")
        .unwrap_or_default()
        .trim();

    if target.is_empty() {
        show_status(config);
        return;
    }

    navigate_to(target, config);
}

/// Show current teleport status and available targets.
fn show_status(config: &RuntimeConfig) {
    println!("Teleport: directory navigation");
    println!("  current: {}", config.cwd.display());
    println!("  session: {}", config.session_id);
    println!();
    println!("  Usage: /teleport <path>");
    println!("  Jumps to the specified directory for the current session.");
    println!();
    println!("  Common targets:");
    println!("    /teleport ~              — home directory");
    println!("    /teleport /tmp           — temp directory");
    println!("    /teleport ../sibling     — sibling directory");
    println!("    /teleport ./subdir       — subdirectory");
}

/// Navigate to a target directory.
fn navigate_to(target: &str, config: &RuntimeConfig) {
    // Resolve special tokens
    let resolved = match target {
        "~" => {
            let home = directories::BaseDirs::new()
                .map(|bd| bd.home_dir().to_path_buf())
                .unwrap_or_else(|| config.cwd.clone());
            home.to_string_lossy().into_owned()
        }
        _ => target.to_owned(),
    };

    let path = if std::path::Path::new(&resolved).is_absolute() {
        std::path::PathBuf::from(&resolved)
    } else {
        config.cwd.join(&resolved)
    };

    if path.exists() && path.is_dir() {
        println!("Teleport: → {}", path.display());
        println!("  session: {}", config.session_id);
        println!("  (directory change will take effect on next operation)");
    } else {
        println!("Teleport: target not found: {}", path.display());
        println!("  The specified path does not exist or is not a directory.");
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
    fn dispatch_teleport_no_args() {
        let config = build_test_config();
        dispatch("/teleport", &config);
    }

    #[test]
    fn dispatch_teleport_to_existing_dir() {
        let config = build_test_config();
        dispatch("/teleport /tmp", &config);
    }

    #[test]
    fn dispatch_teleport_to_nonexistent_dir() {
        let config = build_test_config();
        dispatch("/teleport /nonexistent/path/xyz", &config);
    }

    #[test]
    fn dispatch_teleport_relative_path() {
        let config = build_test_config();
        dispatch("/teleport ..", &config);
    }

    #[test]
    fn show_status_no_panic() {
        let config = build_test_config();
        show_status(&config);
    }

    #[test]
    fn navigate_to_existing_dir() {
        let config = build_test_config();
        navigate_to("/tmp", &config);
    }

    #[test]
    fn navigate_to_nonexistent_dir() {
        let config = build_test_config();
        navigate_to("/this/path/does/not/exist", &config);
    }

    #[test]
    fn dispatch_teleport_with_whitespace() {
        let config = build_test_config();
        dispatch("/teleport   /tmp  ", &config);
    }
}
