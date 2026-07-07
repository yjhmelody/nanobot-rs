//! # Error Types
//!
//! Defines the error and result types used throughout the message bus.
//!
//! The bus currently has a single error variant:
//!
//! - [`NoSubscribers`](BusError::NoSubscribers) — returned when a message is published
//!   to a channel that has zero active receivers.
//!
//! This minimal error surface reflects the bus's role as a pure in-memory
//! communication channel; errors relating to message processing or persistence are
//! handled upstream by consumers.
//!
//! `BusResult<T>` is a type alias for `Result<T, BusError>`, modelled after
//! `std::io::Result<T>` for ergonomic use across the crate.

use thiserror::Error;

/// Errors returned by the message bus.
///
/// Each variant corresponds to a distinct failure mode when publishing or
/// subscribing to messages.
///
/// # Variants
///
/// * `NoSubscribers` — The publish target (inbound or outbound) has no active
///   subscribers, so the message was dropped without being delivered. The `kind`
///   field is a static string label (`"inbound"` or `"outbound"`) identifying
///   which channel failed.
#[derive(Debug, Error)]
pub enum BusError {
    #[error("failed to publish {kind}: no subscribers")]
    NoSubscribers { kind: &'static str },
}

/// Convenience alias for `Result<T, BusError>`.
///
/// This is the standard return type for fallible bus operations such as
/// [`MessageBus::publish_inbound`](crate::MessageBus::publish_inbound) and
/// [`MessageBus::publish_outbound`](crate::MessageBus::publish_outbound).
pub type BusResult<T> = std::result::Result<T, BusError>;

impl BusError {
    /// Creates a `NoSubscribers` error for the given message kind label.
    ///
    /// # Parameters
    ///
    /// * `kind` — A static string identifying the channel type, typically
    ///   `"inbound"` or `"outbound"`. This is used in the error display message.
    ///
    /// # Returns
    ///
    /// A `BusError::NoSubscribers` with the provided kind.
    pub fn no_subscribers(kind: &'static str) -> Self {
        Self::NoSubscribers { kind }
    }
}
