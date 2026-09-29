//! Errors of the provider layer.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Machine-readable category of a [`ProviderError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProviderErrorKind {
    /// Unknown provider or session.
    NotFound,
    /// A provider with the same id is already registered.
    AlreadyExists,
    /// The provider is not installed, not authenticated or unreachable.
    Unavailable,
    /// The provider does not support the operation.
    Unsupported,
    /// The request is malformed (empty input, unknown command, …).
    InvalidRequest,
    /// The session is running a turn.
    Busy,
    /// The session is closed; resume it first.
    Closed,
    /// The turn was cancelled.
    Cancelled,
    /// The provider or its model reported a failure.
    Failed,
    /// Unexpected internal failure.
    Internal,
}

/// Error returned by providers, the registry and the session manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
}

impl ProviderError {
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::NotFound, message)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Unavailable, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Unsupported, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::InvalidRequest, message)
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Cancelled, message)
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Failed, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Internal, message)
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ProviderError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_uses_screaming_snake_case() {
        let err = ProviderError::invalid("empty input");
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            serde_json::json!({"kind": "INVALID_REQUEST", "message": "empty input"})
        );
    }
}
