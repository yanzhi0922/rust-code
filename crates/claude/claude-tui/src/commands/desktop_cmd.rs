//! `/desktop` command — show desktop integration status.

/// Dispatch the `/desktop` command.
pub fn render() {
    println!("Desktop Integration Status");
    println!("─────────────────────────");

    // Deep link support
    let installed = claude_utils::deep_link::is_desktop_installed();
    println!(
        "Desktop app installed: {}",
        if installed { "Yes" } else { "No" }
    );

    if installed {
        if let Some(version) = claude_utils::deep_link::get_desktop_version() {
            let supported = claude_utils::deep_link::is_version_supported(&version);
            println!("Desktop version:      {version}");
            println!(
                "Version supported:    {}",
                if supported {
                    "Yes"
                } else {
                    "No (upgrade needed)"
                }
            );
        } else {
            println!("Desktop version:      (unknown)");
        }
    }

    // Protocol handler
    println!("Protocol handler:     remote-code://");
    println!(
        "Min required version: {}",
        claude_utils::deep_link::MIN_DESKTOP_VERSION
    );

    // Deep link example
    let example_link =
        claude_utils::deep_link::build_deep_link("example-session-id", "/home/user/project", false);
    println!("Example deep link:    {example_link}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_no_panic() {
        render();
    }

    #[test]
    fn min_version_is_valid() {
        assert!(!claude_utils::deep_link::MIN_DESKTOP_VERSION.is_empty());
    }
}
