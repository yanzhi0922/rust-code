use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};
use std::io::{self, Write};

pub struct Renderer;

impl Renderer {
    pub fn new() -> Self {
        Self
    }

    pub fn render_welcome(&self) {
        self.render_color_bold("Claude Code (Rust)", Color::Cyan);
        println!();
        self.render_color(
            "Type /help for commands, or start typing to chat.",
            Color::Blue,
        );
        self.render_color("Press Ctrl+D or type /exit to quit.", Color::Blue);
        println!();
    }

    pub fn render_markdown(&self, text: &str) {
        let parser = pulldown_cmark::Parser::new(text);
        let mut html_output = String::new();
        pulldown_cmark::html::push_html(&mut html_output, parser);
        println!("{html_output}");
    }

    pub fn render_user_message(&self, text: &str) {
        self.render_color_bold(text, Color::Green);
    }

    pub fn render_tool_use(&self, name: &str, input: &serde_json::Value) {
        self.render_color_bold(&format!("[tool:{name}]"), Color::Yellow);
        if let Some(cmd) = input.get("command").and_then(|c| c.as_str()) {
            println!("  {cmd}");
        }
        if let Some(path) = input.get("file_path").and_then(|p| p.as_str()) {
            println!("  {path}");
        }
    }

    pub fn render_tool_result(&self, content: &str, is_error: bool) {
        if is_error {
            self.render_color(&format!("[error] {content}"), Color::Red);
        } else {
            let lines: Vec<&str> = content.lines().take(50).collect();
            for line in lines {
                println!("  {line}");
            }
            if content.lines().count() > 50 {
                println!("  ... ({})", content.lines().count() - 50);
            }
        }
    }

    pub fn render_error(&self, message: &str) {
        self.render_color_bold(&format!("Error: {message}"), Color::Red);
    }

    pub fn render_usage(&self, input_tokens: u32, output_tokens: u32) {
        self.render_color(
            &format!("[{input_tokens} in, {output_tokens} out]"),
            Color::DarkGrey,
        );
    }

    pub fn render_info(&self, message: &str) {
        self.render_color(message, Color::Blue);
    }

    fn render_color(&self, text: &str, color: Color) {
        let mut stdout = io::stdout().lock();
        let _ = crossterm::execute!(stdout, SetForegroundColor(color));
        let _ = write!(stdout, "{text}");
        let _ = crossterm::execute!(stdout, ResetColor);
        println!();
    }

    fn render_color_bold(&self, text: &str, color: Color) {
        let mut stdout = io::stdout().lock();
        let _ = crossterm::execute!(
            stdout,
            SetForegroundColor(color),
            SetAttribute(Attribute::Bold)
        );
        let _ = write!(stdout, "{text}");
        let _ = crossterm::execute!(stdout, ResetColor, SetAttribute(Attribute::Reset));
        println!();
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}
