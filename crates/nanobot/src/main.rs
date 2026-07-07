//! Crate-level entry point for the nanobot application binary.
//!
//! This module is the binary entry point (`main`). It parses CLI arguments via
//! `clap`, initialises observability (tracing), then dispatches to `cli::run`.
//!
//! ## Sub-modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | `acp` | Agent Client Protocol — spawn and manage external ACP agent processes |
//! | `cli` | CLI argument parsing and subcommand dispatch |
//! | `error` | Top-level error enum that wraps all crate-internal errors |
//! | `heartbeat` | Periodic LLM-driven task review and execution |
//! | `observability` | Tracing/logging initialisation |
//! | `runtime` | Bootstrap logic that wires all core services together |
//! | `utils` | Path resolution and workspace template synchronisation |

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

/// Application entry point.
///
/// 1. Initialises the `tracing` subscriber (logging).
/// 2. Parses command-line arguments via `clap`.
/// 3. Dispatches to the appropriate CLI subcommand handler.
///
/// # Errors
///
/// Returns a `NanobotError` on any subcommand failure (config, I/O, provider, etc.).
#[tokio::main]
async fn main() -> NanobotResult<()> {
    observability::init();
    let cli = Cli::parse();
    cli::run(cli).await
}
