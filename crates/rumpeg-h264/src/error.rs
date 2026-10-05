//! Error type for rumpeg-h264.

use thiserror::Error;

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// H.264 codec errors.
#[derive(Debug, Error)]
pub enum Error {
    /// Bitstream ended unexpectedly.
    #[error("truncated bitstream: {0}")]
    Truncated(String),
    /// Invalid / unsupported syntax.
    #[error("invalid bitstream: {0}")]
    Invalid(String),
    /// Feature not implemented in this pure-Rust subset.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl Error {
    pub(crate) fn truncated(msg: impl Into<String>) -> Self {
        Self::Truncated(msg.into())
    }
    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }
    pub(crate) fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }
}
