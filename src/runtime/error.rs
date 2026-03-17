use thiserror::Error;

/// Errors returned by runtime bootstrapping.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Runtime error: {0}")]
    Message(String),
}

pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;

impl RuntimeError {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}
