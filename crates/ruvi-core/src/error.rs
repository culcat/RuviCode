use thiserror::Error;

/// Canonical error type for the `ruvi-core` domain and extension traits.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Requested provider, tool, runtime, or theme does not exist in the registry.
    #[error("{kind} '{id}' was not found")]
    NotFound {
        /// Category of extension or entity.
        kind: &'static str,
        /// Identifier that was looked up.
        id: String,
    },

    /// Configuration or user input failed validation.
    #[error("configuration error: {0}")]
    Config(String),

    /// Upstream LLM provider returned an error or failed transport.
    #[error("provider '{provider}' failed: {message}")]
    Provider {
        /// Provider identifier.
        provider: String,
        /// Failure details.
        message: String,
    },

    /// Tool or plugin execution error.
    #[error("execution error in '{component}': {message}")]
    Execution {
        /// Tool, plugin, or runtime identifier.
        component: String,
        /// Failure details.
        message: String,
    },

    /// External I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience result alias for `ruvi-core` operations.
pub type CoreResult<T> = Result<T, CoreError>;
