//! Core error types for cortex-core.
//!
//! Uses `thiserror` for ergonomic error definitions. Consumers that need
//! to pattern-match errors import the `CortexError` enum directly.

use thiserror::Error;

/// Errors that can occur in cortex-core.
#[derive(Error, Debug)]
pub enum CortexError {
    #[error("Project not found: {0}")]
    ProjectNotFound(String),

    #[error("Theme not found: {0}")]
    ThemeNotFound(String),

    #[error("Job not found: {0}")]
    JobNotFound(String),

    #[error("Invalid guardrails signature")]
    InvalidGuardrailsSignature,

    #[error("State corruption detected: {0}")]
    StateCorruption(String),

    #[error("Concurrency limit reached: {0}")]
    ConcurrencyLimitReached(String),

    #[error("Worker timeout after {0}s")]
    WorkerTimeout(u64),

    #[error("LLM call failed: {0}")]
    LlmError(String),

    #[error("Hermes communication failed: {0}")]
    HermesError(String),

    #[error("WAL operation failed: {0}")]
    WalError(String),

    #[error("Validation failed: {0}")]
    ValidationError(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

pub type Result<T> = std::result::Result<T, CortexError>;
