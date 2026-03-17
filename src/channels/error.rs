use thiserror::Error;

/// Errors returned by channel adapters and channel management.
#[derive(Debug, Error)]
pub enum ChannelError {
    /// Configuration error for a channel.
    #[error("Channel configuration error: {0}")]
    Config(String),

    /// Adapter runtime error.
    #[error("Channel '{channel}' error: {message}")]
    Adapter { channel: String, message: String },
}

pub type ChannelResult<T> = std::result::Result<T, ChannelError>;

impl ChannelError {
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }

    pub fn adapter(channel: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Adapter {
            channel: channel.into(),
            message: message.into(),
        }
    }
}
