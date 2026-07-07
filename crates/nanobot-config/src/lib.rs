//! Configuration types for the nanobot AI agent framework.
//!
//! This crate provides the central configuration subsystem used by all
//! nanobot components. It defines the [`Config`] struct — the top-level
//! configuration document — along with all nested sub-configuration types
//! for agents, channels, providers, tools, retrieval, ACP, and the gateway.
//!
//! # Architecture
//!
//! The crate is organized into four modules:
//!
//! | Module     | Contents                                              |
//! |------------|-------------------------------------------------------|
//! | `schema`   | Core configuration types and validation logic         |
//! | `loader`   | File I/O: reading, parsing, and writing config files  |
//! | `error`    | Error and result types                                |
//! | `acp`      | Agent Client Protocol (ACP) integration configuration |
//!
//! # Design
//!
//! - **Serde-driven deserialization**: All types derive `Serialize` and
//!   `Deserialize`, supporting both `camelCase` (primary) and `snake_case`
//!   field names for migration compatibility from a Python codebase.
//! - **Defaults-first**: Every configuration struct implements `Default`,
//!   and the config file is optionally loaded on top of defaults. This means
//!   nanobot can run without any config file at all.
//! - **Validation**: [`Config::validate`] performs semantic checks (range
//!   checks, cross-field consistency) after deserialization.
//! - **Provider name normalization**: Provider names are normalized via
//!   [`normalize_provider_name`] to accept hyphenated, camelCase, or
//!   snake_case variants interchangeably.
//!
//! # Relationships
//!
//! - Depends on `nanobot-types` for shared types like [`ReasoningConfig`].
//! - Used by `nanobot-agent`, `nanobot-gateway`, and other binary crates.
//! - The `loader` module is built on top of `schema`, reading files into
//!   the `Config` type.

pub mod acp;
pub mod error;
pub mod loader;
pub mod schema;

pub use error::{ConfigError, ConfigResult};
pub use loader::{get_config_path, load_config, save_config};
pub use schema::*;
