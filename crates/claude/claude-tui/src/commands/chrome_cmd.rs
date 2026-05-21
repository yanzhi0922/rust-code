//! `/chrome` command — show Chrome extension status.

/// Dispatch the `/chrome` command.
pub fn render() {
    println!("Chrome Extension Status");
    println!("───────────────────────");
    println!("Extension installed:  (not detected)");
    println!("Connected:            No");
    println!("Version:              N/A");
    println!();
    println!("To install the Chrome extension:");
    println!("  1. Open chrome://extensions/");
    println!("  2. Enable Developer mode");
    println!("  3. Load the extension from apps/remote-code-chrome/");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_no_panic() {
        render();
    }
}
