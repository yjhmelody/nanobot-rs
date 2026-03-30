use thiserror::Error;

/// Errors returned by the message bus.
#[derive(Debug, Error)]
pub enum BusError {
    #[error("failed to publish {kind}: no subscribers")]
    NoSubscribers { kind: &'static str },
}

/// Convenience alias for `Result<T, BusError>`.
pub type BusResult<T> = std::result::Result<T, BusError>;

impl BusError {
    /// Creates a `NoSubscribers` error for the given message kind label.
    pub fn no_subscribers(kind: &'static str) -> Self {
        Self::NoSubscribers { kind }
    }
}
