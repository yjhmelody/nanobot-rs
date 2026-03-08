//! ACP (Agent Client Protocol) integration

pub mod client;
pub mod config;
pub mod simple_client;

pub use client::ACPClient;
pub use config::{ACPConfig, AgentConfig};
pub use simple_client::SimpleClient;
