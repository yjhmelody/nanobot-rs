use thiserror::Error;

/// Errors returned by runtime bootstrapping (`build_runtime`).
///
/// Currently a simple message wrapper; may be extended with structured
/// variants as the bootstrap logic grows more complex.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// Generic runtime error with a human-readable message.
    #[error("Runtime error: {0}")]
    Message(String),
}

impl RuntimeError {
    /// Create a new `RuntimeError::Message` from any `Into<String>` value.
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}
