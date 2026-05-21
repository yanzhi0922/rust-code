use super::SLASH_COMMANDS;

pub fn render() {
    println!("Available commands:");
    for spec in SLASH_COMMANDS {
        println!("  {:<12} {} [{}]", spec.name, spec.summary, spec.usage);
    }
    println!();
    println!("Examples:");
    println!("  /provider");
    println!("  /model");
    println!("  /mcp");
    println!("  /plugins");
    println!("  /skills");
    println!("  /permissions");
    println!("  /review");
    println!("  /tasks");
    println!("  /worktree");
}
