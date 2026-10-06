//! Provider Registry: which providers exist and which one is active.

use crate::error::{ProviderError, ProviderErrorKind};
use crate::provider::{AIProvider, ProviderCapabilities, ProviderDescriptor, ProviderStatus};
use orchestrator_core::{AuditEvent, CallOrigin, EventKind, EventSink, ProviderId};
use parking_lot::RwLock;
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;

/// A registered provider as listed to the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    #[serde(flatten)]
    pub descriptor: ProviderDescriptor,
    pub capabilities: ProviderCapabilities,
    /// True for the provider new sessions use by default.
    pub active: bool,
}

pub struct ProviderRegistry {
    providers: RwLock<Vec<Arc<dyn AIProvider>>>,
    active: RwLock<Option<ProviderId>>,
    sink: Arc<dyn EventSink>,
}

impl ProviderRegistry {
    pub fn new(sink: Arc<dyn EventSink>) -> Self {
        Self {
            providers: RwLock::new(Vec::new()),
            active: RwLock::new(None),
            sink,
        }
    }

    /// Adds a provider. The first one registered becomes active.
    pub fn register(&self, provider: Arc<dyn AIProvider>) -> Result<(), ProviderError> {
        let id = provider.descriptor().id;
        let mut providers = self.providers.write();
        if providers.iter().any(|p| p.descriptor().id == id) {
            return Err(ProviderError::new(
                ProviderErrorKind::AlreadyExists,
                format!("provider {id} is already registered"),
            ));
        }
        providers.push(provider);
        let mut active = self.active.write();
        if active.is_none() {
            *active = Some(id);
        }
        Ok(())
    }

    /// Adds `provider`, or replaces the one registered with the same id
    /// (e.g. an edited API connection). Open sessions keep the instance they
    /// started with.
    pub fn replace(&self, provider: Arc<dyn AIProvider>) {
        let id = provider.descriptor().id;
        let mut providers = self.providers.write();
        match providers.iter().position(|p| p.descriptor().id == id) {
            Some(index) => providers[index] = provider,
            None => providers.push(provider),
        }
        let mut active = self.active.write();
        if active.is_none() {
            *active = Some(id);
        }
    }

    /// Removes a provider. When it was the active one, the first remaining
    /// provider becomes active (recorded as `PROVIDER_SWITCHED`).
    pub fn unregister(&self, id: &ProviderId, origin: CallOrigin) -> bool {
        let next = {
            let mut providers = self.providers.write();
            let before = providers.len();
            providers.retain(|p| &p.descriptor().id != id);
            if providers.len() == before {
                return false;
            }
            providers.first().map(|p| p.descriptor())
        };
        let mut active = self.active.write();
        if active.as_ref() == Some(id) {
            *active = next.as_ref().map(|d| d.id.clone());
            drop(active);
            self.sink.audit(AuditEvent::new(
                EventKind::ProviderSwitched,
                origin,
                format!(
                    "provider {id} removed → {}",
                    next.as_ref().map_or("—", |d| d.name.as_str())
                ),
                json!({ "from": id, "to": next.map(|d| d.id), "reason": "removed" }),
            ));
        }
        true
    }

    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn AIProvider>> {
        self.providers
            .read()
            .iter()
            .find(|p| &p.descriptor().id == id)
            .cloned()
    }

    /// Like [`Self::get`], with a `NOT_FOUND` error.
    pub fn require(&self, id: &ProviderId) -> Result<Arc<dyn AIProvider>, ProviderError> {
        self.get(id)
            .ok_or_else(|| ProviderError::not_found(format!("provider {id} is not registered")))
    }

    pub fn list(&self) -> Vec<ProviderInfo> {
        let active = self.active_id();
        self.providers
            .read()
            .iter()
            .map(|p| {
                let descriptor = p.descriptor();
                ProviderInfo {
                    active: active.as_ref() == Some(&descriptor.id),
                    descriptor,
                    capabilities: p.capabilities(),
                }
            })
            .collect()
    }

    pub fn active_id(&self) -> Option<ProviderId> {
        self.active.read().clone()
    }

    pub fn active(&self) -> Option<Arc<dyn AIProvider>> {
        self.active_id().and_then(|id| self.get(&id))
    }

    /// Makes `id` the active provider. Records `PROVIDER_SWITCHED` when it
    /// changes.
    pub fn select(&self, id: &ProviderId, origin: CallOrigin) -> Result<(), ProviderError> {
        let provider = self.require(id)?;
        let previous = {
            let mut active = self.active.write();
            if active.as_ref() == Some(id) {
                return Ok(());
            }
            active.replace(id.clone())
        };
        let name = provider.descriptor().name;
        self.sink.audit(AuditEvent::new(
            EventKind::ProviderSwitched,
            origin,
            format!(
                "provider {} → {name}",
                previous.as_ref().map_or("—", ProviderId::as_str)
            ),
            json!({ "from": previous, "to": id }),
        ));
        Ok(())
    }

    pub async fn inspect(&self, id: &ProviderId) -> Result<ProviderStatus, ProviderError> {
        Ok(self.require(id)?.inspect().await)
    }
}
