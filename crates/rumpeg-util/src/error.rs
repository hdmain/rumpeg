//! Error types shared across Rumpeg crates.

use thiserror::Error;

/// Convenient result alias for Rumpeg operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Library-wide error type.
#[derive(Debug, Error)]
pub enum Error {
    /// I/O failure while reading or writing media.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// End of stream / no more data available.
    #[error("end of file")]
    Eof,

    /// Decoder/encoder needs more input before producing output.
    #[error("need more data")]
    NeedMoreData,

    /// Operation would block waiting for input or output capacity.
    #[error("try again")]
    TryAgain,

    /// Requested codec, format, or feature is not available.
    #[error("not found: {0}")]
    NotFound(String),

    /// Invalid or unsupported media parameters.
    #[error("invalid data: {0}")]
    InvalidData(String),

    /// Unsupported codec, pixel format, sample format, or container.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// Protocol or format probe failed.
    #[error("probe failed: {0}")]
    ProbeFailed(String),

    /// Buffer or frame size exceeded limits.
    #[error("buffer too small: need {need}, have {have}")]
    BufferTooSmall {
        /// Required size in bytes.
        need: usize,
        /// Available size in bytes.
        have: usize,
    },

    /// Catch-all for other failures.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Create an [`Error::InvalidData`] variant.
    pub fn invalid_data(msg: impl Into<String>) -> Self {
        Self::InvalidData(msg.into())
    }

    /// Create an [`Error::Unsupported`] variant.
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }

    /// Create an [`Error::NotFound`] variant.
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }
}
