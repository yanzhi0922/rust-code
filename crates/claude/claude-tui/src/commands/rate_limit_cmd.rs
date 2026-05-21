//! `/rate-limits` command — show rate limit status.

/// Dispatch the `/rate-limits` command.
pub fn render() {
    println!("Rate Limit Status");
    println!("─────────────────");
    println!();
    println!("Current limits:");
    println!("  Requests per minute:  (not tracked)");
    println!("  Tokens per minute:    (not tracked)");
    println!("  Concurrent requests:  (not tracked)");
    println!();
    println!("Usage:");
    println!("  (no rate limit data available for this session)");
    println!();
    println!("Rate limits are determined by your API provider and plan.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_no_panic() {
        render();
    }
}
