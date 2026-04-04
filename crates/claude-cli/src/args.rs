use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "claude",
    version,
    about = "Claude Code CLI - AI-powered coding assistant"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// One-shot prompt (non-interactive mode)
    #[arg(short, long)]
    pub prompt: Option<String>,

    /// Model to use
    #[arg(short, long, env = "CLAUDE_MODEL")]
    pub model: Option<String>,

    /// Maximum turns for the agentic loop
    #[arg(long, default_value = "100")]
    pub max_turns: u32,

    /// Maximum tokens per response
    #[arg(long, default_value = "8192")]
    pub max_tokens: u32,

    /// Permission mode
    #[arg(long, value_enum, default_value = "default")]
    pub permission_mode: PermissionModeArg,

    /// Bypass all permission checks
    #[arg(long)]
    pub dangerously_skip_permissions: bool,

    /// Output format
    #[arg(long, value_enum)]
    pub output_format: Option<OutputFormatArg>,

    /// Working directory
    #[arg(short, long, default_value = ".")]
    pub workdir: String,

    /// Verbose logging
    #[arg(short, long)]
    pub verbose: bool,

    /// API key
    #[arg(long, env = "ANTHROPIC_API_KEY")]
    pub api_key: Option<String>,

    /// Base URL for API
    #[arg(long, env = "ANTHROPIC_BASE_URL")]
    pub base_url: Option<String>,

    /// System prompt
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Continue the most recent conversation
    #[arg(long)]
    pub resume: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new project
    Init {
        #[arg(default_value = ".")]
        path: String,
    },
    /// Log in with OAuth
    Login,
    /// Log out
    Logout,
    /// Check system health
    Doctor,
    /// Update the CLI
    Update,
    /// Manage MCP servers
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Export conversation
    Export {
        #[arg(short, long)]
        format: Option<String>,
        #[arg(short, long)]
        output: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum McpAction {
    /// List configured MCP servers
    List,
    /// Add a new MCP server
    Add {
        name: String,
        command: String,
        args: Vec<String>,
    },
    /// Remove an MCP server
    Remove { name: String },
}

#[derive(clap::ValueEnum, Clone, Debug, Default)]
pub enum PermissionModeArg {
    #[default]
    Default,
    Plan,
    AcceptEdits,
    ReadOnly,
    Bypass,
}

#[derive(clap::ValueEnum, Clone, Debug)]
pub enum OutputFormatArg {
    Text,
    Json,
    StreamJson,
}
