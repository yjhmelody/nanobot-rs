use clap::Parser;

use crate::cli::Cli;
use crate::error::NanobotResult;

mod acp;
mod cli;
mod error;
mod heartbeat;
mod observability;
mod runtime;
mod utils;

#[tokio::main]
async fn main() -> NanobotResult<()> {
    observability::init();
    let cli = Cli::parse();
    cli::run(cli).await
}
