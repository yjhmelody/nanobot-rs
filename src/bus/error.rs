use thiserror::Error;

/// Errors returned by the message bus.
#[derive(Debug, Error)]
pub enum BusError {
    #[error("failed to publish {kind}: no subscribers")]
    NoSubscribers { kind: &'static str },
}

pub type BusResult<T> = std::result::Result<T, BusError>;

impl BusError {
    pub fn no_subscribers(kind: &'static str) -> Self {
        Self::NoSubscribers { kind }
    }
}
