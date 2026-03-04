use anyhow::Result;
use clap::Parser;

use nanobot_rs::cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    nanobot_rs::utils::helpers::init_tracing();
    let cli = Cli::parse();
    nanobot_rs::cli::run(cli).await
}
