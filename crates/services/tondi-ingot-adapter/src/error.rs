//! Error types for the Tondi Ingot Adapter.

use thiserror::Error;

/// Errors that can occur in the Tondi Ingot Adapter.
#[derive(Debug, Error)]
pub enum TondiAdapterError {
    /// Failed to serialize block data.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Failed to deserialize data.
    #[error("Deserialization error: {0}")]
    Deserialization(String),

    /// Failed to build Ingot payload.
    #[error("Payload build error: {0}")]
    PayloadBuild(String),

    /// Failed to submit transaction to Tondi.
    #[error("Submission error: {0}")]
    Submission(String),

    /// RPC communication error.
    #[error("RPC error: {0}")]
    Rpc(String),

    /// Payload exceeds size limit.
    #[error("Payload too large: {size} bytes (max: {max} bytes)")]
    PayloadTooLarge {
        /// Actual payload size.
        size: usize,
        /// Maximum allowed size.
        max: usize,
    },

    /// Invalid configuration.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Storage error.
    #[error("Storage error: {0}")]
    Storage(String),

    /// Signing error.
    #[error("Signing error: {0}")]
    Signing(String),

    /// Block data is missing or invalid.
    #[error("Invalid block data: {0}")]
    InvalidBlockData(String),

    /// Parent reference mismatch.
    #[error("Parent reference mismatch: expected {expected}, got {actual}")]
    ParentMismatch {
        /// Expected parent reference.
        expected: String,
        /// Actual parent reference.
        actual: String,
    },

    /// Service shutdown.
    #[error("Service shutdown")]
    Shutdown,
}

impl From<postcard::Error> for TondiAdapterError {
    fn from(err: postcard::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

impl From<borsh::io::Error> for TondiAdapterError {
    fn from(err: borsh::io::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

/// Result type for Tondi Ingot Adapter operations.
pub type Result<T> = std::result::Result<T, TondiAdapterError>;

