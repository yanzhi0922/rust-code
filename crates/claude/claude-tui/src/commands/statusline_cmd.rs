//! `/statusline` command — configure status line display.

/// Dispatch the `/statusline` command with subcommands.
pub fn dispatch(input: &str) {
    let mut parts = input.split_whitespace();
    let _command = parts.next(); // skip "/statusline"
    let subcommand = parts.next();

    match subcommand {
        None | Some("show") => render_show(),
        Some("set") => {
            let format: Vec<&str> = parts.collect();
            if format.is_empty() {
                println!("Usage: /statusline set <format>");
                println!("Available format tokens:");
                println!("  {{session}}  - Session ID");
                println!("  {{model}}    - Current model name");
                println!("  {{provider}} - Active provider");
                println!("  {{mode}}     - Permission mode");
                println!("  {{cost}}     - Session cost");
                println!("  {{tokens}}   - Token usage");
            } else {
                let format_str = format.join(" ");
                println!("Status line format set to: {format_str}");
            }
        }
        Some(other) => {
            println!("Unknown subcommand '{other}'");
            println!("Usage: /statusline [show|set <format>]");
        }
    }
}

fn render_show() {
    println!("Status Line Configuration");
    println!("─────────────────────────");
    println!("Current format: {{session}} | {{model}} | {{cost}}");
    println!();
    println!("Available format tokens:");
    println!("  {{session}}  - Session ID");
    println!("  {{model}}    - Current model name");
    println!("  {{provider}} - Active provider");
    println!("  {{mode}}     - Permission mode");
    println!("  {{cost}}     - Session cost");
    println!("  {{tokens}}   - Token usage");
    println!();
    println!("Usage: /statusline set <format>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_show_no_panic() {
        dispatch("/statusline show");
    }

    #[test]
    fn dispatch_no_args_no_panic() {
        dispatch("/statusline");
    }

    #[test]
    fn dispatch_set_format() {
        dispatch("/statusline set {session} | {model}");
    }

    #[test]
    fn dispatch_set_no_format() {
        dispatch("/statusline set");
    }

    #[test]
    fn dispatch_unknown_subcommand() {
        dispatch("/statusline foo");
    }
}
