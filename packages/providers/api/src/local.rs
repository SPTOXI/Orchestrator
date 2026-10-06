//! Connections served by the Orchestrator's own engine (ADR-0025): the
//! connection says *what* (`local: true`, its models), the engine says
//! *where*. Before each call the provider asks the engine for a server
//! with the model loaded; the engine starts or switches it, and the lease
//! keeps it loaded until the call ends.

use async_trait::async_trait;
use orchestrator_providers::ProviderError;
use parking_lot::RwLock;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Id of the connection the app keeps for the local models.
pub const LOCAL_CONNECTION: &str = "local";

/// A server with the model loaded, for one call.
pub struct LocalLease {
    /// `http://127.0.0.1:<port>/v1`
    pub base_url: String,
    /// Keeps the model loaded while it exists.
    pub hold: Box<dyn Send + Sync>,
}

#[async_trait]
pub trait LocalEndpoint: Send + Sync {
    /// A server with `model` loaded, started (or switched to) if needed.
    async fn acquire(
        &self,
        model: &str,
        cancel: &CancellationToken,
    ) -> Result<LocalLease, ProviderError>;

    /// Whether the engine can serve at all; `Err` says what is missing
    /// (no engine installed, …).
    fn ready(&self) -> Result<(), String>;
}

/// The engine, once the app has one (shared by every provider).
pub type SharedLocal = Arc<RwLock<Option<Arc<dyn LocalEndpoint>>>>;
