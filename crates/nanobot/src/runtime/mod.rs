//! Runtime bootstrapping for the nanobot application.
//!
//! The runtime module wires together all core services (bus, LLM provider,
//! agent loop, cron, heartbeat, tool registry) into a `RuntimeBundle`.
//!
//! ## Sub-modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | `app` | `RuntimeBundle` struct and `build_runtime` constructor |
//! | `error` | `RuntimeError` type for bootstrap failures |

pub mod app;
pub mod error;
