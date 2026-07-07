//! Observability initialisation (tracing/logging).
//!
//! Sets up the `tracing_subscriber` with:
//! - An `EnvFilter` that reads `RUST_LOG` (defaults to `info`).
//! - A fmt layer that includes target module names.
//!
//! Uses `OnceLock` to ensure `init()` is called at most once, even if
//! invoked multiple times (e.g., from tests).

use std::sync::OnceLock;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialise the global tracing subscriber (logging).
///
/// Idempotent: subsequent calls are no-ops. The level is configured via
/// the `RUST_LOG` environment variable (defaults to `"info"`).
pub fn init() {
    static INIT: OnceLock<()> = OnceLock::new();

    INIT.get_or_init(|| {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_target(true))
            .init();
    });
}
