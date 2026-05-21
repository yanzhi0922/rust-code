//! `/mobile` command — show mobile app connection status.

/// Dispatch the `/mobile` command.
pub fn render() {
    println!("Mobile App Status");
    println!("─────────────────");
    println!("Connected:        No");
    println!("Platform:         N/A");
    println!("Session:          N/A");
    println!();
    println!("To connect a mobile app:");
    println!("  1. Install Remote Code from the App Store or Google Play");
    println!("  2. Scan the QR code from the desktop app");
    println!("  3. Or enter the pairing code manually");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_no_panic() {
        render();
    }
}
