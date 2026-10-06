//! Catalog: every model of every registered provider, with the provider's
//! last known availability (ADR-0011).

use chrono::Utc;
use orchestrator_core::ProviderId;
use orchestrator_providers::{ModelInfo, ProviderRegistry, ProviderStatus};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;

/// How long an `inspect` result is trusted.
pub const AVAILABILITY_TTL: Duration = Duration::from_secs(5 * 60);
/// Upper bound for one `inspect` while routing.
const INSPECT_TIMEOUT: Duration = Duration::from_secs(20);

/// One model as the router sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub provider: ProviderId,
    pub provider_name: String,
    pub info: ModelInfo,
    /// The connection's default model.
    pub is_default: bool,
    /// The provider new sessions use by default.
    pub provider_active: bool,
    /// The provider lets models call tools.
    pub provider_tools: bool,
    /// The provider answers one-off requests (can sit on the Council).
    pub completion: bool,
    /// Last `inspect`; `None` = not checked.
    pub available: Option<bool>,
    pub availability_detail: Option<String>,
}

/// `inspect` results, reused for [`AVAILABILITY_TTL`].
pub struct Availability {
    entries: Mutex<HashMap<ProviderId, (Instant, ProviderStatus)>>,
    ttl: Duration,
}

impl Availability {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    pub fn get(&self, id: &ProviderId) -> Option<ProviderStatus> {
        let entries = self.entries.lock();
        entries
            .get(id)
            .filter(|(at, _)| at.elapsed() < self.ttl)
            .map(|(_, status)| status.clone())
    }

    pub fn put(&self, id: ProviderId, status: ProviderStatus) {
        self.entries.lock().insert(id, (Instant::now(), status));
    }

    pub fn forget(&self, id: &ProviderId) {
        self.entries.lock().remove(id);
    }

    /// Inspects, in parallel, every registered provider without a fresh
    /// result.
    pub async fn refresh(&self, registry: &ProviderRegistry) {
        let mut jobs = JoinSet::new();
        for info in registry.list() {
            let id = info.descriptor.id;
            if self.get(&id).is_some() {
                continue;
            }
            let Some(provider) = registry.get(&id) else {
                continue;
            };
            jobs.spawn(async move {
                let status = match tokio::time::timeout(INSPECT_TIMEOUT, provider.inspect()).await {
                    Ok(status) => status,
                    Err(_) => ProviderStatus {
                        available: false,
                        version: None,
                        authenticated: None,
                        detail: Some(format!("sem resposta em {} s", INSPECT_TIMEOUT.as_secs())),
                        checked_at: Utc::now(),
                    },
                };
                (id, status)
            });
        }
        while let Some(joined) = jobs.join_next().await {
            if let Ok((id, status)) = joined {
                self.put(id, status);
            }
        }
    }
}

impl Default for Availability {
    fn default() -> Self {
        Self::new(AVAILABILITY_TTL)
    }
}

/// Every model of every registered provider, with the cached availability.
pub fn catalog(registry: &ProviderRegistry, availability: &Availability) -> Vec<CatalogModel> {
    let mut models = Vec::new();
    for info in registry.list() {
        let status = availability.get(&info.descriptor.id);
        let caps = &info.capabilities;
        for model in &caps.models {
            models.push(CatalogModel {
                provider: info.descriptor.id.clone(),
                provider_name: info.descriptor.name.clone(),
                info: model.clone(),
                is_default: caps.default_model.as_deref() == Some(model.id.as_str()),
                provider_active: info.active,
                provider_tools: caps.tool_calls,
                completion: caps.completion,
                available: status.as_ref().map(|s| s.available),
                availability_detail: status
                    .as_ref()
                    .filter(|s| !s.available)
                    .and_then(|s| s.detail.clone()),
            });
        }
    }
    models
}
