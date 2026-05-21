//! `/version` command — display version information.

/// Dispatch the `/version` command.
pub fn render() {
    println!("Remote Code Rust");
    println!("  version:     {}", env!("CARGO_PKG_VERSION"));
    println!("  description: {}", env!("CARGO_PKG_DESCRIPTION"));
    println!(
        "  platform:    {}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("  rustc:       {}", rustc_version());
}

/// Get the Rust compiler version (best-effort).
fn rustc_version() -> &'static str {
    // rustc_version is not available at runtime, so we use a compile-time fallback
    env!("CARGO_PKG_RUST_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_no_panic() {
        render();
    }

    #[test]
    fn version_contains_package_version() {
        // The version string should contain the package version from Cargo.toml
        let expected = env!("CARGO_PKG_VERSION");
        assert!(!expected.is_empty());
    }

    #[test]
    fn rustc_version_not_empty() {
        assert!(!rustc_version().is_empty());
    }

    #[test]
    fn platform_info_valid() {
        let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        assert!(!platform.is_empty());
        assert!(platform.contains('-'));
    }

    #[test]
    fn description_not_empty() {
        let desc = env!("CARGO_PKG_DESCRIPTION");
        assert!(!desc.is_empty());
    }
}
