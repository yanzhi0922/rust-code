pub mod adapter;
pub mod app;
pub mod args;
pub mod input;
pub mod render;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let cli = args::Cli::parse();
    let mut app = app::App::new(cli)?;

    app.run().await?;

    Ok(())
}
