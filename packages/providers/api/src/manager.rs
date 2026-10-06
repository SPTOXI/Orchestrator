//! The user's API connections: persisted (without secrets) in
//! `connections.json`, registered as providers, keys in the vault (ADR-0010).

use crate::config::{Connection, CredentialSource, ModelEntry};
use crate::http::HttpClient;
use crate::local::{LocalEndpoint, SharedLocal};
use crate::presets::{presets, Preset};
use crate::provider::{ApiProvider, ConversationStore, TestReport};
use crate::secrets::SecretStore;
use orchestrator_core::{AuditEvent, CallOrigin, EventKind, EventSink, ProviderId};
use orchestrator_providers::{ProviderError, ProviderErrorKind, ProviderRegistry};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const FILE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct ConnectionsFile {
    version: u32,
    connections: Vec<Connection>,
}

/// Whether the credential is usable. The key itself never leaves the vault.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyStatus {
    pub source: CredentialSource,
    pub present: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionView {
    pub connection: Connection,
    pub key: KeyStatus,
}

/// Create or update a connection.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveRequest {
    pub connection: Connection,
    /// New key (stored in the vault); `None`/empty keeps the current one.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Remove the stored key.
    #[serde(default)]
    pub clear_key: bool,
    /// Id before an edit that renamed the connection.
    #[serde(default)]
    pub previous_id: Option<String>,
}

/// Test or list models for a (possibly unsaved) configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeRequest {
    pub connection: Connection,
    /// Key typed in the form; default: the stored credential.
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

pub struct ConnectionManager {
    path: PathBuf,
    registry: Arc<ProviderRegistry>,
    secrets: Arc<dyn SecretStore>,
    sink: Arc<dyn EventSink>,
    client: HttpClient,
    /// Shared with the providers, which look up their fallback here.
    connections: Arc<RwLock<Vec<Connection>>>,
    /// Conversation history per connection id, shared by its successive
    /// provider instances (sessions survive an edit of their connection).
    conversations: Mutex<HashMap<String, ConversationStore>>,
    writes: tokio::sync::Mutex<()>,
    /// The engine of the local connection (ADR-0025), once the app has one.
    local: SharedLocal,
}

impl ConnectionManager {
    /// Loads `path` and registers every enabled connection. Problems with
    /// individual entries come back as warnings; they never stop the app.
    pub fn open(
        path: &Path,
        registry: Arc<ProviderRegistry>,
        secrets: Arc<dyn SecretStore>,
        sink: Arc<dyn EventSink>,
    ) -> Result<(Self, Vec<String>), ProviderError> {
        let client = HttpClient::new()?;
        let mut warnings = Vec::new();
        let mut connections = Vec::new();
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<ConnectionsFile>(&text) {
                Ok(file) => {
                    for connection in file.connections {
                        match connection.validate() {
                            Ok(()) => connections.push(connection),
                            Err(err) => warnings.push(format!(
                                "connection {} ignored: {}",
                                connection.id, err.message
                            )),
                        }
                    }
                }
                Err(err) => warnings.push(format!(
                    "{} is not valid ({err}); no API connection loaded",
                    path.display()
                )),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => warnings.push(format!("cannot read {}: {err}", path.display())),
        }
        let manager = Self {
            path: path.to_path_buf(),
            registry,
            secrets,
            sink,
            client,
            connections: Arc::new(RwLock::new(Vec::new())),
            conversations: Mutex::new(HashMap::new()),
            writes: tokio::sync::Mutex::new(()),
            local: SharedLocal::default(),
        };
        for connection in &connections {
            if manager
                .registry
                .get(&ProviderId::from(connection.id.as_str()))
                .is_some()
            {
                warnings.push(format!(
                    "connection {} ignored: the id is used by another provider",
                    connection.id
                ));
                continue;
            }
            if connection.enabled {
                manager
                    .registry
                    .replace(Arc::new(manager.registered(connection.clone())));
            }
        }
        *manager.connections.write() = connections;
        Ok((manager, warnings))
    }

    /// A throwaway instance for tests and model discovery.
    fn provider(&self, connection: Connection) -> ApiProvider {
        ApiProvider::new(connection, self.secrets.clone(), self.client.clone())
            .with_local(self.local.clone())
    }

    /// Gives the local connection its engine (ADR-0025). Providers already
    /// registered see it from their next call.
    pub fn set_local_endpoint(&self, endpoint: Arc<dyn LocalEndpoint>) {
        *self.local.write() = Some(endpoint);
    }

    /// The instance registered for a saved connection.
    fn registered(&self, connection: Connection) -> ApiProvider {
        let store = self
            .conversations
            .lock()
            .entry(connection.id.clone())
            .or_default()
            .clone();
        let connections = self.connections.clone();
        self.provider(connection)
            .with_conversations(store)
            .with_fallbacks(Arc::new(move |id: &str| {
                connections
                    .read()
                    .iter()
                    .find(|c| c.id == id && c.enabled)
                    .cloned()
            }))
    }

    pub fn secrets_backend(&self) -> String {
        self.secrets.describe()
    }

    pub fn presets(&self) -> Vec<Preset> {
        presets()
    }

    pub fn get(&self, id: &str) -> Option<Connection> {
        self.connections.read().iter().find(|c| c.id == id).cloned()
    }

    /// Every connection with the state of its credential.
    pub async fn list(&self) -> Vec<ConnectionView> {
        let connections = self.connections.read().clone();
        let mut views = Vec::with_capacity(connections.len());
        for connection in connections {
            let key = self.key_status(&connection).await;
            views.push(ConnectionView { connection, key });
        }
        views
    }

    async fn key_status(&self, connection: &Connection) -> KeyStatus {
        let source = connection.credential.source;
        match source {
            CredentialSource::None => KeyStatus {
                source,
                present: true,
                detail: None,
            },
            CredentialSource::Env => {
                let var = connection.credential.env_var.clone().unwrap_or_default();
                let present = std::env::var(&var).is_ok_and(|v| !v.trim().is_empty());
                KeyStatus {
                    source,
                    present,
                    detail: (!present).then(|| format!("variável {var} não definida")),
                }
            }
            CredentialSource::Vault => {
                let secrets = self.secrets.clone();
                let id = connection.id.clone();
                match tokio::task::spawn_blocking(move || secrets.get(&id)).await {
                    Ok(Ok(value)) => KeyStatus {
                        source,
                        present: value.is_some(),
                        detail: None,
                    },
                    Ok(Err(err)) => KeyStatus {
                        source,
                        present: false,
                        detail: Some(format!("cofre indisponível: {err}")),
                    },
                    Err(err) => KeyStatus {
                        source,
                        present: false,
                        detail: Some(err.to_string()),
                    },
                }
            }
        }
    }

    async fn vault(&self, op: VaultOp) -> Result<Option<String>, ProviderError> {
        let secrets = self.secrets.clone();
        tokio::task::spawn_blocking(move || match op {
            VaultOp::Get(id) => secrets.get(&id),
            VaultOp::Set(id, key) => secrets.set(&id, &key).map(|()| None),
            VaultOp::Delete(id) => secrets.delete(&id).map(|()| None),
        })
        .await
        .map_err(|e| ProviderError::internal(format!("vault task failed: {e}")))?
        .map_err(|e| {
            ProviderError::unavailable(format!(
                "OS vault unavailable ({e}); use an environment variable as credential instead"
            ))
        })
    }

    fn persist(&self, connections: &[Connection]) -> Result<(), ProviderError> {
        let io = |e: std::io::Error| {
            ProviderError::internal(format!("cannot write {}: {e}", self.path.display()))
        };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let file = ConnectionsFile {
            version: FILE_VERSION,
            connections: connections.to_vec(),
        };
        let text = serde_json::to_string_pretty(&file)
            .map_err(|e| ProviderError::internal(e.to_string()))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::rename(&tmp, &self.path).map_err(io)
    }

    /// Creates or updates a connection (`CONNECTION_SAVED`).
    pub async fn save(
        &self,
        request: SaveRequest,
        origin: CallOrigin,
    ) -> Result<ConnectionView, ProviderError> {
        let _guard = self.writes.lock().await;
        let connection = request.connection;
        connection.validate()?;
        let previous = request
            .previous_id
            .clone()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| connection.id.clone());
        let mut list = self.connections.read().clone();
        let existing = list.iter().position(|c| c.id == previous);
        if list
            .iter()
            .enumerate()
            .any(|(i, c)| c.id == connection.id && Some(i) != existing)
        {
            return Err(ProviderError::new(
                ProviderErrorKind::AlreadyExists,
                format!("another connection already uses the id {}", connection.id),
            ));
        }
        let ours = |id: &str| self.connections.read().iter().any(|c| c.id == id);
        if !ours(&connection.id)
            && self
                .registry
                .get(&ProviderId::from(connection.id.as_str()))
                .is_some()
        {
            return Err(ProviderError::new(
                ProviderErrorKind::AlreadyExists,
                format!("the id {} is used by another provider", connection.id),
            ));
        }

        // Secrets first: a failure here changes nothing else.
        let new_key = request.api_key.filter(|k| !k.trim().is_empty());
        let renamed = previous != connection.id;
        if new_key.is_some() && connection.credential.source != CredentialSource::Vault {
            return Err(ProviderError::invalid(
                "a key is only stored when the credential is the OS vault",
            ));
        }
        if request.clear_key {
            self.vault(VaultOp::Delete(connection.id.clone())).await?;
        }
        if let Some(key) = new_key {
            self.vault(VaultOp::Set(connection.id.clone(), key.trim().to_owned()))
                .await?;
        } else if renamed && connection.credential.source == CredentialSource::Vault {
            if let Some(old) = self.vault(VaultOp::Get(previous.clone())).await? {
                self.vault(VaultOp::Set(connection.id.clone(), old)).await?;
            }
        }
        if renamed {
            let _ = self.vault(VaultOp::Delete(previous.clone())).await;
        }

        match existing {
            Some(index) => list[index] = connection.clone(),
            None => list.push(connection.clone()),
        }
        self.persist(&list)?;
        *self.connections.write() = list;

        if renamed {
            self.registry
                .unregister(&ProviderId::from(previous.as_str()), origin.clone());
            self.conversations.lock().remove(&previous);
        }
        if connection.enabled {
            self.registry
                .replace(Arc::new(self.registered(connection.clone())));
        } else {
            self.registry
                .unregister(&ProviderId::from(connection.id.as_str()), origin.clone());
        }
        self.sink.audit(AuditEvent::new(
            EventKind::ConnectionSaved,
            origin,
            format!(
                "API connection {} · {} ({})",
                if existing.is_some() {
                    "updated"
                } else {
                    "added"
                },
                connection.name,
                connection.kind.label()
            ),
            json!({
                "id": connection.id,
                "name": connection.name,
                "kind": connection.kind,
                "baseUrl": connection.base_url,
                "credential": connection.credential.source,
                "models": connection.models.len(),
                "enabled": connection.enabled,
                "created": existing.is_none(),
                "renamedFrom": renamed.then_some(previous),
            }),
        ));
        let key = self.key_status(&connection).await;
        Ok(ConnectionView { connection, key })
    }

    /// Removes a connection and its stored key (`CONNECTION_REMOVED`).
    pub async fn remove(&self, id: &str, origin: CallOrigin) -> Result<(), ProviderError> {
        let _guard = self.writes.lock().await;
        let mut list = self.connections.read().clone();
        let index = list
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| ProviderError::not_found(format!("connection {id} not found")))?;
        let removed = list.remove(index);
        self.persist(&list)?;
        *self.connections.write() = list;
        self.registry
            .unregister(&ProviderId::from(id), origin.clone());
        self.conversations.lock().remove(id);
        let vault_note = if removed.credential.source == CredentialSource::Vault {
            self.vault(VaultOp::Delete(id.to_owned()))
                .await
                .err()
                .map(|e| e.message)
        } else {
            None
        };
        self.sink.audit(AuditEvent::new(
            EventKind::ConnectionRemoved,
            origin,
            format!("API connection removed · {}", removed.name),
            json!({
                "id": removed.id,
                "name": removed.name,
                "kind": removed.kind,
                "vaultError": vault_note,
            }),
        ));
        Ok(())
    }

    /// Tests a (possibly unsaved) configuration.
    pub async fn test(&self, request: ProbeRequest) -> Result<TestReport, ProviderError> {
        request.connection.validate()?;
        let provider = self.provider(request.connection).with_key(request.api_key);
        Ok(provider.test(request.model).await)
    }

    /// Models of a (possibly unsaved) configuration.
    pub async fn models(&self, request: ProbeRequest) -> Result<Vec<ModelEntry>, ProviderError> {
        request.connection.validate()?;
        self.provider(request.connection)
            .with_key(request.api_key)
            .list_models()
            .await
    }
}

enum VaultOp {
    Get(String),
    Set(String, String),
    Delete(String),
}
