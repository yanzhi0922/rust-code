//! `/passes` command — show available passes (auth passes, rate limit passes).

/// Dispatch the `/passes` command.
pub fn render() {
    println!("Passes & Rate Limit Status");
    println!("──────────────────────────");
    println!();
    println!("Auth Passes:");
    println!("  (no active passes)");
    println!();
    println!("Rate Limit Passes:");
    println!("  (no active passes)");
    println!();
    println!("Passes grant temporary elevated access or extended rate limits.");
    println!("Contact your administrator to obtain a pass.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_no_panic() {
        render();
    }
}
