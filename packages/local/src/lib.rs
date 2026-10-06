//! Local models with the Orchestrator's own engine (ADR-0025).
//!
//! - [`engine`]: the llama.cpp release package for this computer,
//!   downloaded, checked (SHA-256) and installed under `<data>/local/engine`.
//! - [`sources`]: where models come from — the catalog, any Hugging Face
//!   repository, the models Ollama already downloaded, a file on disk.
//! - [`gguf`]: what a model file says about itself (context, tools…).
//! - [`server`]: `llama-server` running one model at a time, with the
//!   context the Orchestrator chose; started on demand, stopped when idle.
//! - [`LocalEngine`]: all of it behind one object, which also serves the
//!   API provider's local connection ([`LocalEndpoint`]).

pub mod archive;
pub mod download;
pub mod engine;
pub mod gguf;
pub mod platform;
pub mod server;
pub mod sources;
pub mod store;

use async_trait::async_trait;
use chrono::Utc;
use download::Expected;
use engine::{EngineDir, EngineInfo};
use orchestrator_provider_api::local::{LocalEndpoint, LocalLease, LOCAL_CONNECTION};
use orchestrator_provider_api::{
    ApiKind, Connection, Credential, CredentialSource, ModelEntry, ToolMode,
};
use orchestrator_providers::ProviderError;
use parking_lot::Mutex;
use platform::{Backend, System};
use serde::Serialize;
use serde_json::Value;
use server::{Launch, Server, ServerEvent, ServerOptions, ServerStatus};
use sources::{CatalogModel, HfFile, OllamaModel, Urls};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use store::{LocalModel, ModelSource, ModelsFile, Settings};
use tokio_util::sync::CancellationToken;

pub use server::ProcessWatch;

/// Key of the engine's download in progress events.
pub const ENGINE_DOWNLOAD: &str = "engine";

/// What the engine reports to the app (screen and history).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum LocalEvent {
    /// `key`: [`ENGINE_DOWNLOAD`] or the model download's key.
    Progress {
        key: String,
        done: u64,
        total: Option<u64>,
    },
    DownloadDone {
        key: String,
    },
    DownloadFailed {
        key: String,
        error: String,
    },
    EngineInstalled {
        tag: String,
        backend: Backend,
        previous: Option<String>,
    },
    EngineRemoved {
        tag: String,
        backend: Backend,
    },
    ModelAdded {
        id: String,
        source: String,
        size: u64,
        sha256: Option<String>,
    },
    ModelRemoved {
        id: String,
    },
    /// The models changed: the local connection must follow.
    ModelsChanged,
    Server(ServerEvent),
}

/// Everything the screen shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStatus {
    pub system: System,
    pub engine: Option<EngineInfo>,
    /// The package "automático" picks here.
    pub auto_backend: Option<Backend>,
    pub backends: Vec<Backend>,
    pub settings: Settings,
    pub server: ServerStatus,
    pub models: Vec<LocalModel>,
    pub catalog: Vec<CatalogEntry>,
    /// Downloads in progress, by key.
    pub downloading: Vec<String>,
    pub default_context: u32,
    /// Problems reading the files under `<data>/local`.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    #[serde(flatten)]
    pub model: CatalogModel,
    /// Id of the model already downloaded from this entry.
    pub installed: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaEntry {
    #[serde(flatten)]
    pub model: OllamaModel,
    /// Id of the model already imported from it.
    pub imported: Option<String>,
}

pub struct LocalEngine {
    root: PathBuf,
    system: System,
    urls: Urls,
    client: reqwest::Client,
    engine_dir: EngineDir,
    engine: Mutex<Option<EngineInfo>>,
    models: Mutex<ModelsFile>,
    settings: Mutex<Settings>,
    warnings: Mutex<Vec<String>>,
    downloads: Mutex<HashMap<String, CancellationToken>>,
    /// Serializes installs and model file changes.
    changes: tokio::sync::Mutex<()>,
    server: Server,
    events: Arc<dyn Fn(LocalEvent) + Send + Sync>,
    ollama_dirs: Vec<PathBuf>,
}

impl LocalEngine {
    /// Opens `<data>/local` (`root`). Must run inside a Tokio runtime (the
    /// idle watch is a task).
    pub fn open(
        root: PathBuf,
        watch: Option<Arc<dyn ProcessWatch>>,
        events: Arc<dyn Fn(LocalEvent) + Send + Sync>,
    ) -> Arc<Self> {
        Self::open_with(
            root,
            System::detect(),
            Urls::default(),
            sources::ollama_dirs(),
            watch,
            events,
            ServerOptions::default(),
        )
    }

    /// [`Self::open`] with every outside dependency chosen (tests).
    pub fn open_with(
        root: PathBuf,
        system: System,
        urls: Urls,
        ollama_dirs: Vec<PathBuf>,
        watch: Option<Arc<dyn ProcessWatch>>,
        events: Arc<dyn Fn(LocalEvent) + Send + Sync>,
        options: ServerOptions,
    ) -> Arc<Self> {
        let mut warnings = Vec::new();
        let (models, w) = store::read_json::<ModelsFile>(&root.join("models.json"));
        warnings.extend(w);
        let (settings, w) = store::read_json::<Settings>(&root.join("settings.json"));
        warnings.extend(w);
        let engine_dir = EngineDir::new(root.join("engine"));
        let engine = engine_dir.load();
        let server_events = events.clone();
        let server = Server::new(
            watch,
            Arc::new(move |e| server_events(LocalEvent::Server(e))),
            options,
        );
        server.set_idle(Duration::from_secs(u64::from(settings.idle_minutes) * 60));
        server.watch_idle();
        let client = reqwest::Client::builder()
            .user_agent(concat!("Orchestrator/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Arc::new(Self {
            root,
            system,
            urls,
            client,
            engine_dir,
            engine: Mutex::new(engine),
            models: Mutex::new(models),
            settings: Mutex::new(settings),
            warnings: Mutex::new(warnings),
            downloads: Mutex::new(HashMap::new()),
            changes: tokio::sync::Mutex::new(()),
            server,
            events,
            ollama_dirs,
        })
    }

    pub fn status(&self) -> LocalStatus {
        let models = self.models.lock().models.clone();
        let catalog = sources::catalog()
            .into_iter()
            .map(|model| CatalogEntry {
                installed: models
                    .iter()
                    .find(|m| matches!(&m.source, ModelSource::Catalog { entry, .. } if entry == model.id))
                    .map(|m| m.id.clone()),
                model,
            })
            .collect();
        LocalStatus {
            system: self.system.clone(),
            engine: self.engine.lock().clone(),
            auto_backend: self.system.auto_backend(),
            backends: self.system.backends(),
            settings: self.settings.lock().clone(),
            server: self.server.status(),
            models,
            catalog,
            downloading: self.downloads.lock().keys().cloned().collect(),
            default_context: self.system.default_context(),
            warnings: self.warnings.lock().clone(),
        }
    }

    pub fn models(&self) -> Vec<LocalModel> {
        self.models.lock().models.clone()
    }

    pub fn log(&self) -> Vec<String> {
        self.server.log()
    }

    pub fn save_settings(&self, settings: Settings) -> Result<Settings, String> {
        settings.validate()?;
        store::write_json(&self.root.join("settings.json"), &settings)?;
        self.server
            .set_idle(Duration::from_secs(u64::from(settings.idle_minutes) * 60));
        *self.settings.lock() = settings.clone();
        Ok(settings)
    }

    /// The newest llama.cpp release, to offer an update.
    pub async fn latest_engine(&self) -> Result<String, String> {
        Ok(sources::latest_release(&self.client, &self.urls).await?.tag)
    }

    /// Installs (or updates) the engine: the package in the settings, or
    /// the one this computer calls for.
    pub async fn install_engine(&self) -> Result<EngineInfo, String> {
        let backend = self
            .settings
            .lock()
            .backend
            .filter(|b| self.system.backends().contains(b))
            .or_else(|| self.system.auto_backend())
            .ok_or("não há motor do llama.cpp para este sistema")?;
        let _guard = self.changes.lock().await;
        let cancel = self.begin(ENGINE_DOWNLOAD)?;
        let result = async {
            let release = sources::latest_release(&self.client, &self.urls).await?;
            // The server must not run the files being replaced.
            self.server.stop("atualização do motor").await;
            let key = ENGINE_DOWNLOAD.to_owned();
            let events = self.events.clone();
            self.engine_dir
                .install(
                    &self.client,
                    &self.system,
                    &release,
                    backend,
                    &move |done, total| {
                        events(LocalEvent::Progress {
                            key: key.clone(),
                            done,
                            total,
                        })
                    },
                    &cancel,
                )
                .await
        }
        .await;
        self.end(ENGINE_DOWNLOAD, &result);
        let info = result?;
        let previous = self.engine.lock().replace(info.clone()).map(|p| p.tag);
        (self.events)(LocalEvent::EngineInstalled {
            tag: info.tag.clone(),
            backend: info.backend,
            previous,
        });
        Ok(info)
    }

    pub async fn remove_engine(&self) -> Result<(), String> {
        let _guard = self.changes.lock().await;
        self.server.stop("motor removido").await;
        let removed = self.engine_dir.remove()?;
        *self.engine.lock() = None;
        if let Some(info) = removed {
            (self.events)(LocalEvent::EngineRemoved {
                tag: info.tag,
                backend: info.backend,
            });
        }
        Ok(())
    }

    pub async fn stop(&self) {
        self.server.stop("pedido do usuário").await;
    }

    /// At exit (synchronous).
    pub fn shutdown(&self) {
        self.server.kill_now();
    }

    pub fn cancel_download(&self, key: &str) {
        if let Some(token) = self.downloads.lock().get(key) {
            token.cancel();
        }
    }

    fn begin(&self, key: &str) -> Result<CancellationToken, String> {
        let mut downloads = self.downloads.lock();
        if downloads.contains_key(key) {
            return Err("esse download já está em andamento".into());
        }
        let token = CancellationToken::new();
        downloads.insert(key.to_owned(), token.clone());
        Ok(token)
    }

    fn end<T>(&self, key: &str, result: &Result<T, String>) {
        self.downloads.lock().remove(key);
        (self.events)(match result {
            Ok(_) => LocalEvent::DownloadDone {
                key: key.to_owned(),
            },
            Err(error) => LocalEvent::DownloadFailed {
                key: key.to_owned(),
                error: error.clone(),
            },
        });
    }

    // ------------------------------------------------------------ models ---

    pub async fn hf_files(&self, repo: &str) -> Result<Vec<HfFile>, String> {
        let repo = sources::repo_id(repo)?;
        sources::hf_files(&self.client, &self.urls, &repo).await
    }

    /// Downloads a catalog entry.
    pub async fn download_catalog(&self, entry: &str) -> Result<LocalModel, String> {
        let item = sources::catalog()
            .into_iter()
            .find(|c| c.id == entry)
            .ok_or_else(|| format!("{entry} não está no catálogo"))?;
        let cancel = self.begin(entry)?;
        let result = async {
            let files = sources::hf_files(&self.client, &self.urls, item.repo).await?;
            let file = sources::pick_quant(&files, item.quant).ok_or_else(|| {
                format!(
                    "{} não tem mais a versão {} no Hugging Face",
                    item.repo, item.quant
                )
            })?;
            let source = ModelSource::Catalog {
                entry: item.id.into(),
                repo: item.repo.into(),
                file: file.clone(),
            };
            self.download_hf_files(entry, item.label, item.repo, &files, &file, source, &cancel)
                .await
        }
        .await;
        self.end(entry, &result);
        result
    }

    /// Downloads `file` of a Hugging Face repository.
    pub async fn download_hf(&self, repo: &str, file: &str) -> Result<LocalModel, String> {
        let repo = sources::repo_id(repo)?;
        let key = format!("{repo}/{file}");
        let cancel = self.begin(&key)?;
        let result = async {
            let files = sources::hf_files(&self.client, &self.urls, &repo).await?;
            let source = ModelSource::HuggingFace {
                repo: repo.clone(),
                file: file.to_owned(),
            };
            let name = file
                .rsplit('/')
                .next()
                .unwrap_or(file)
                .trim_end_matches(".gguf")
                .to_owned();
            self.download_hf_files(&key, &name, &repo, &files, file, source, &cancel)
                .await
        }
        .await;
        self.end(&key, &result);
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_hf_files(
        &self,
        key: &str,
        name: &str,
        repo: &str,
        files: &[HfFile],
        file: &str,
        source: ModelSource,
        cancel: &CancellationToken,
    ) -> Result<LocalModel, String> {
        let parts = sources::parts_of(files, file);
        if parts.is_empty() {
            return Err(format!("{file} não está (inteiro) em {repo}"));
        }
        let id = self.new_id(name);
        let dir = self.root.join("models").join(&id);
        let total: u64 = parts.iter().map(|p| p.size).sum();
        let mut offset = 0u64;
        let mut paths = Vec::new();
        let mut sha256 = None;
        for part in &parts {
            let dest = dir.join(part.name());
            let base = offset;
            let key_owned = key.to_owned();
            let events = self.events.clone();
            let digest = download::fetch(
                &self.client,
                &sources::hf_url(&self.urls, repo, &part.path),
                &dest,
                &Expected {
                    size: (part.size > 0).then_some(part.size),
                    sha256: part.sha256.clone(),
                },
                &move |done, _| {
                    events(LocalEvent::Progress {
                        key: key_owned.clone(),
                        done: base + done,
                        total: Some(total),
                    })
                },
                cancel,
            )
            .await;
            let digest = match digest {
                Ok(d) => d,
                Err(e) => {
                    if cancel.is_cancelled() {
                        let _ = std::fs::remove_dir_all(&dir);
                    }
                    return Err(e);
                }
            };
            sha256.get_or_insert(digest);
            offset += part.size;
            paths.push(dest);
        }
        self.register(name, paths, sha256, source).await
    }

    /// The models Ollama downloaded on this computer.
    pub fn ollama_models(&self) -> Vec<OllamaEntry> {
        let models = self.models.lock().models.clone();
        sources::ollama_models(&self.ollama_dirs)
            .into_iter()
            .map(|model| OllamaEntry {
                imported: models
                    .iter()
                    .find(|m| matches!(&m.source, ModelSource::Ollama { name } if *name == model.name))
                    .map(|m| m.id.clone()),
                model,
            })
            .collect()
    }

    /// Takes a model from Ollama: a hard link to its file (no new space,
    /// and it survives Ollama), or a copy when that cannot be done.
    pub async fn import_ollama(&self, name: &str) -> Result<LocalModel, String> {
        let model = sources::ollama_models(&self.ollama_dirs)
            .into_iter()
            .find(|m| m.name == name)
            .ok_or_else(|| format!("o Ollama não tem o modelo {name} neste computador"))?;
        let id = self.new_id(name);
        let dir = self.root.join("models").join(&id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let dest = dir.join("model.gguf");
        let blob = model.blob.clone();
        let target = dest.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::hard_link(&blob, &target)
                .or_else(|_| std::fs::copy(&blob, &target).map(|_| ()))
                .map_err(|e| format!("não foi possível trazer o arquivo do Ollama: {e}"))
        })
        .await
        .map_err(|e| e.to_string())??;
        let result = self
            .register(
                name,
                vec![dest],
                Some(model.sha256.clone()),
                ModelSource::Ollama {
                    name: name.to_owned(),
                },
            )
            .await;
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        result
    }

    /// A `.gguf` on disk, used where it is.
    pub async fn add_file(&self, path: &Path) -> Result<LocalModel, String> {
        if !path.is_file() {
            return Err(format!("{} não é um arquivo", path.display()));
        }
        let name = path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("modelo")
            .to_owned();
        self.register(&name, vec![path.to_path_buf()], None, ModelSource::File)
            .await
    }

    /// Reads the file's header and adds the model.
    async fn register(
        &self,
        name: &str,
        files: Vec<PathBuf>,
        sha256: Option<String>,
        source: ModelSource,
    ) -> Result<LocalModel, String> {
        let main = files.first().cloned().ok_or("nenhum arquivo")?;
        let info = tokio::task::spawn_blocking(move || gguf::read(&main))
            .await
            .map_err(|e| e.to_string())??;
        let size = files
            .iter()
            .filter_map(|f| std::fs::metadata(f).ok())
            .map(|m| m.len())
            .sum();
        let trained = info
            .context_length
            .map(|c| c.min(u64::from(u32::MAX)) as u32);
        let context = trained
            .map(|t| t.min(self.system.default_context()))
            .unwrap_or_else(|| self.system.default_context())
            .max(2048);
        let _guard = self.changes.lock().await;
        let mut models = self.models.lock().clone();
        let taken: Vec<String> = models.models.iter().map(|m| m.id.clone()).collect();
        let base_id = files
            .first()
            .and_then(|f| f.parent())
            .filter(|p| p.starts_with(self.root.join("models")))
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(str::to_owned);
        let id = base_id
            .filter(|id| !taken.contains(id))
            .unwrap_or_else(|| store::unique_id(name, &taken));
        let display = info
            .name
            .clone()
            .filter(|n| !n.trim().is_empty() && matches!(source, ModelSource::Catalog { .. }))
            .unwrap_or_else(|| name.to_owned());
        let model = LocalModel {
            id: id.clone(),
            name: display,
            files,
            size,
            sha256: sha256.clone(),
            source: source.clone(),
            context,
            trained_context: trained,
            architecture: info.architecture.clone(),
            size_label: info.size_label.clone(),
            quantization: info.quantization.clone(),
            tools: info.takes_tools(),
            chat_template: info.chat_template.is_some(),
            kv_bytes_per_token: info.kv_bytes_per_token(),
            added_at: Utc::now(),
        };
        models.models.push(model.clone());
        store::write_json(&self.root.join("models.json"), &models)?;
        *self.models.lock() = models;
        (self.events)(LocalEvent::ModelAdded {
            id,
            source: source.kind().into(),
            size,
            sha256,
        });
        (self.events)(LocalEvent::ModelsChanged);
        Ok(model)
    }

    /// An id for a model being downloaded (its folder's name).
    fn new_id(&self, name: &str) -> String {
        let mut taken: Vec<String> = self
            .models
            .lock()
            .models
            .iter()
            .map(|m| m.id.clone())
            .collect();
        // Folders left by unfinished downloads keep their names.
        if let Ok(entries) = std::fs::read_dir(self.root.join("models")) {
            taken.extend(
                entries
                    .flatten()
                    .filter(|e| {
                        !e.path().read_dir().is_ok_and(|mut d| {
                            d.any(|f| {
                                f.is_ok_and(|f| f.path().extension().is_some_and(|x| x == "part"))
                            })
                        })
                    })
                    .filter_map(|e| e.file_name().to_str().map(str::to_owned)),
            );
        }
        store::unique_id(name, &taken)
    }

    /// Removes a model; its files too, unless it is a file of the user's.
    pub async fn remove_model(&self, id: &str) -> Result<(), String> {
        let _guard = self.changes.lock().await;
        let mut models = self.models.lock().clone();
        let index = models
            .models
            .iter()
            .position(|m| m.id == id)
            .ok_or_else(|| format!("o modelo {id} não existe"))?;
        if matches!(self.server.status(), ServerStatus::Ready { model, .. } | ServerStatus::Starting { model } if model == id)
        {
            self.server.stop("modelo removido").await;
        }
        let model = models.models.remove(index);
        store::write_json(&self.root.join("models.json"), &models)?;
        *self.models.lock() = models;
        if model.source != ModelSource::File {
            let _ = std::fs::remove_dir_all(self.root.join("models").join(&model.id));
        }
        (self.events)(LocalEvent::ModelRemoved { id: id.to_owned() });
        (self.events)(LocalEvent::ModelsChanged);
        Ok(())
    }

    /// Changes the context a model runs with (applies on its next start).
    pub fn set_context(&self, id: &str, context: u32) -> Result<LocalModel, String> {
        if !(2048..=1_048_576).contains(&context) {
            return Err("o contexto vai de 2.048 a 1.048.576 tokens".into());
        }
        let mut models = self.models.lock().clone();
        let model = models
            .models
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| format!("o modelo {id} não existe"))?;
        if let Some(trained) = model.trained_context {
            if context > trained {
                return Err(format!(
                    "{} foi treinado para até {trained} tokens",
                    model.name
                ));
            }
        }
        model.context = context;
        let changed = model.clone();
        store::write_json(&self.root.join("models.json"), &models)?;
        *self.models.lock() = models;
        (self.events)(LocalEvent::ModelsChanged);
        Ok(changed)
    }

    /// The API connection for these models (ADR-0025), keeping what the
    /// user set on it before (name, tags, default…).
    pub fn connection(&self, existing: Option<&Connection>) -> Connection {
        let models = self.models.lock().models.clone();
        local_connection(&models, existing)
    }

    /// Why the engine cannot serve, if it cannot.
    pub fn readiness(&self) -> Result<(), String> {
        if self.engine.lock().is_none() {
            return Err("o motor local não está instalado (Configurações → Modelos locais)".into());
        }
        if self.models.lock().models.is_empty() {
            return Err("nenhum modelo local ainda (Configurações → Modelos locais)".into());
        }
        Ok(())
    }

    pub async fn acquire(
        &self,
        model: &str,
        cancel: &CancellationToken,
    ) -> Result<server::Lease, String> {
        let engine = self.engine.lock().clone().ok_or(
            "o motor local não está instalado: instale-o em Configurações → Modelos locais",
        )?;
        let entry = self
            .models
            .lock()
            .models
            .iter()
            .find(|m| m.id == model)
            .cloned()
            .ok_or_else(|| format!("o modelo local {model} não existe mais"))?;
        let file = entry.main_file().ok_or("modelo sem arquivo")?.to_path_buf();
        let launch = Launch {
            model: entry.id.clone(),
            file,
            context: entry.context,
            gpu: self.settings.lock().gpu,
        };
        self.server.acquire(&engine, &launch, cancel).await
    }
}

/// The local connection for `models`.
pub fn local_connection(models: &[LocalModel], existing: Option<&Connection>) -> Connection {
    let mut conn = existing.cloned().unwrap_or_else(|| Connection {
        id: LOCAL_CONNECTION.into(),
        name: "Modelos locais".into(),
        kind: ApiKind::Openai,
        base_url: String::new(),
        credential: Credential {
            source: CredentialSource::None,
            env_var: None,
        },
        headers: Default::default(),
        extra_body: Value::Null,
        models: Vec::new(),
        default_model: None,
        tool_mode: Some(ToolMode::Native),
        max_tool_rounds: orchestrator_provider_api::DEFAULT_MAX_TOOL_ROUNDS,
        max_output_tokens: None,
        options: Default::default(),
        generic: None,
        enabled: true,
        notes: Some("Mantida pelo Orchestrator: Configurações → Modelos locais.".into()),
        // Processing a long prompt on the processor takes a while.
        first_response_secs: Some(600),
        fallback: None,
        local: true,
    });
    // Never used: the engine gives the address on each call.
    conn.base_url = "http://127.0.0.1/v1".into();
    conn.local = true;
    let previous = std::mem::take(&mut conn.models);
    conn.models = models
        .iter()
        .map(|local| {
            let mut entry = previous
                .iter()
                .find(|m| m.id == local.id)
                .cloned()
                .unwrap_or_else(|| ModelEntry::new(&local.id));
            if entry.name.is_none() {
                entry.name = Some(local.name.clone());
            }
            entry.context_window = Some(local.context);
            entry.supports_tools = Some(local.tools);
            entry.input_price = Some(0.0);
            entry.output_price = Some(0.0);
            for tag in ["local", "offline"] {
                if !entry.tags.iter().any(|t| t == tag) {
                    entry.tags.push(tag.into());
                }
            }
            entry
        })
        .collect();
    if conn
        .default_model
        .as_ref()
        .is_some_and(|d| conn.model(d).is_none())
    {
        conn.default_model = None;
    }
    conn
}

#[async_trait]
impl LocalEndpoint for LocalEngine {
    async fn acquire(
        &self,
        model: &str,
        cancel: &CancellationToken,
    ) -> Result<LocalLease, ProviderError> {
        let lease = LocalEngine::acquire(self, model, cancel)
            .await
            .map_err(|e| {
                if cancel.is_cancelled() {
                    ProviderError::cancelled(e)
                } else {
                    ProviderError::unavailable(e)
                }
            })?;
        Ok(LocalLease {
            base_url: lease.base_url.clone(),
            hold: Box::new(lease),
        })
    }

    fn ready(&self) -> Result<(), String> {
        self.readiness()
    }
}
