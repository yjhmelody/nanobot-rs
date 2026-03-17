use clap::Parser;

use nanobot_rs::cli::Cli;
use nanobot_rs::error::NanobotResult;

#[tokio::main]
async fn main() -> NanobotResult<()> {
    nanobot_rs::observability::init();
    let cli = Cli::parse();
    nanobot_rs::cli::run(cli).await
}
