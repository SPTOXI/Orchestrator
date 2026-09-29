//! Strongly typed identifiers.
//!
//! Identifiers are UUID v7 strings: globally unique and ordered by creation
//! time, so they can be used directly as SQLite primary keys (Phase 6).

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates a new time-ordered identifier.
            pub fn new() -> Self {
                Self(Uuid::now_v7().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    };
}

define_id!(
    /// Identifies one [`crate::ToolCall`] and its [`crate::ToolResult`].
    ToolCallId
);
define_id!(
    /// Identifies one [`crate::AuditEvent`].
    EventId
);
define_id!(
    /// Identifies a terminal (PTY session) managed by the Tool Runtime.
    TerminalId
);
define_id!(
    /// Identifies a long-running process managed by the Tool Runtime.
    ProcessId
);
define_id!(
    /// Identifies a provider session owned by the Orchestrator (ADR-0009).
    SessionId
);
define_id!(
    /// Identifies one turn (input → provider output) of a session.
    TurnId
);

/// Stable, human-chosen identifier of a registered AI provider, e.g.
/// `openai-codex` or `claude-code`. Not generated: it names an adapter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(String);

impl ProviderId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ProviderId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for ProviderId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_time_ordered() {
        let a = ToolCallId::new();
        let b = ToolCallId::new();
        assert_ne!(a, b);
        assert!(a < b, "UUID v7 ids must sort by creation time");
    }

    #[test]
    fn ids_serialize_as_plain_strings() {
        let id = TerminalId::from("term-1");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"term-1\"");
        let back: TerminalId = serde_json::from_str("\"term-1\"").unwrap();
        assert_eq!(back, id);
    }
}
