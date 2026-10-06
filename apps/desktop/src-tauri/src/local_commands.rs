//! Tauri commands of the local models (ADR-0025): the engine (install,
//! update, remove), the models (catalog, Hugging Face, Ollama, a file),
//! their context, and the API connection the sessions use. Progress and
//! the engine's state go out on `runtime://local`.

use crate::{AppState, DesktopSink};
use orchestrator_core::{AuditEvent, CallOrigin, EventKind, EventSink};
use orchestrator_local::engine::{backend_id, EngineInfo};
use orchestrator_local::server::ServerEvent;
use orchestrator_local::sources::HfFile;
use orchestrator_local::store::{LocalModel, Settings};
use orchestrator_local::{LocalEngine, LocalEvent, LocalStatus, OllamaEntry, ProcessWatch};
use orchestrator_provider_api::local::LOCAL_CONNECTION;
use orchestrator_provider_api::{ConnectionManager, SaveRequest};
use orchestrator_runtime::ToolRuntime;
use serde::Serialize;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

pub const LOCAL_EVENT: &str = "runtime://local";
/// The connection the app made for Ollama before ADR-0025.
const LEGACY_OLLAMA: &str = "ollama";

/// The engine process joins the runtime's supervision (ADR-0018).
pub struct Supervision(pub ToolRuntime);

impl ProcessWatch for Supervision {
    fn adopt(&self, child: &tokio::process::Child, command: &str) {
        self.0.supervise(child, command);
    }
    fn release(&self, pid: Option<u32>) {
        self.0.unsupervise(pid);
    }
}

/// Opens the engine under `<data>/local`; its events go to the screen and,
/// the ones that matter, to the history.
pub fn open(
    data_dir: &std::path::Path,
    runtime: ToolRuntime,
    sink: Arc<DesktopSink>,
    app: AppHandle,
) -> Arc<LocalEngine> {
    let root = data_dir.join("local");
    tauri::async_runtime::block_on(async move {
        LocalEngine::open(
            root,
            Some(Arc::new(Supervision(runtime))),
            Arc::new(move |event| {
                audit(sink.as_ref(), &event);
                let _ = app.emit(LOCAL_EVENT, &event);
            }),
        )
    })
}

fn audit(sink: &DesktopSink, event: &LocalEvent) {
    let (kind, origin, summary, data) = match event {
        LocalEvent::EngineInstalled {
            tag,
            backend,
            previous,
        } => (
            EventKind::LocalEngineInstalled,
            CallOrigin::User,
            match previous {
                Some(from) if from != tag => format!(
                    "motor local atualizado: llama.cpp {from} → {tag} ({})",
                    backend.label()
                ),
                _ => format!(
                    "motor local instalado: llama.cpp {tag} ({})",
                    backend.label()
                ),
            },
            json!({"tag": tag, "variant": backend_id(*backend), "from": previous}),
        ),
        LocalEvent::EngineRemoved { tag, backend } => (
            EventKind::LocalEngineRemoved,
            CallOrigin::User,
            format!("motor local removido: llama.cpp {tag}"),
            json!({"tag": tag, "variant": backend_id(*backend)}),
        ),
        LocalEvent::ModelAdded {
            id,
            source,
            size,
            sha256,
        } => (
            EventKind::LocalModelAdded,
            CallOrigin::User,
            format!("modelo local adicionado: {id} ({})", source_label(source)),
            json!({"id": id, "source": source, "size": size, "sha256": sha256}),
        ),
        LocalEvent::ModelRemoved { id } => (
            EventKind::LocalModelRemoved,
            CallOrigin::User,
            format!("modelo local removido: {id}"),
            json!({"id": id}),
        ),
        LocalEvent::Server(ServerEvent::Ready {
            model,
            context,
            gpu,
            ms,
        }) => (
            EventKind::LocalModelLoaded,
            CallOrigin::System,
            format!(
                "motor local ligou {model} (contexto {context}) em {:.1} s",
                *ms as f64 / 1000.0
            ),
            json!({"id": model, "context": context, "gpu": gpu, "ms": ms}),
        ),
        _ => return,
    };
    sink.audit(AuditEvent::new(kind, origin, summary, data));
}

fn source_label(source: &str) -> &str {
    match source {
        "catalog" => "catálogo",
        "huggingface" => "Hugging Face",
        "ollama" => "importado do Ollama",
        _ => "arquivo",
    }
}

fn manager(state: &AppState) -> Result<&Arc<ConnectionManager>, String> {
    state
        .connections
        .as_ref()
        .ok_or_else(|| "as conexões de API não estão disponíveis".to_owned())
}

/// Keeps the local connection in step with the models: created with the
/// first one, updated on every change.
async fn sync_connection(state: &AppState) -> Result<(), String> {
    let manager = manager(state)?;
    let existing = manager.get(LOCAL_CONNECTION);
    if existing.is_none() && state.local.models().is_empty() {
        return Ok(());
    }
    let connection = state.local.connection(existing.as_ref());
    if existing.as_ref() == Some(&connection) {
        return Ok(());
    }
    manager
        .save(
            SaveRequest {
                connection,
                api_key: None,
                clear_key: false,
                previous_id: None,
            },
            CallOrigin::User,
        )
        .await
        .map(|_| ())
        .map_err(|e| e.message)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalView {
    #[serde(flatten)]
    pub status: LocalStatus,
    /// The connection the sessions use, when it exists.
    pub connection: Option<String>,
    /// The connection the app made for Ollama before ADR-0025 is still
    /// there.
    pub legacy_ollama: bool,
    pub ollama: Vec<OllamaEntry>,
}

#[tauri::command]
pub fn local_status(state: State<'_, AppState>) -> LocalView {
    let connections = state.connections.as_ref();
    LocalView {
        status: state.local.status(),
        connection: connections
            .and_then(|m| m.get(LOCAL_CONNECTION))
            .map(|c| c.id),
        legacy_ollama: connections.is_some_and(|m| m.get(LEGACY_OLLAMA).is_some()),
        ollama: state.local.ollama_models(),
    }
}

#[tauri::command]
pub async fn local_engine_install(state: State<'_, AppState>) -> Result<EngineInfo, String> {
    state.local.install_engine().await
}

/// The newest llama.cpp release (to offer "Atualizar motor").
#[tauri::command]
pub async fn local_engine_latest(state: State<'_, AppState>) -> Result<String, String> {
    state.local.latest_engine().await
}

#[tauri::command]
pub async fn local_engine_remove(state: State<'_, AppState>) -> Result<(), String> {
    state.local.remove_engine().await
}

#[tauri::command]
pub fn local_cancel(state: State<'_, AppState>, key: String) {
    state.local.cancel_download(&key);
}

#[tauri::command]
pub async fn local_download_catalog(
    state: State<'_, AppState>,
    entry: String,
) -> Result<LocalModel, String> {
    let model = state.local.download_catalog(&entry).await?;
    sync_connection(&state).await?;
    Ok(model)
}

#[tauri::command]
pub async fn local_hf_files(
    state: State<'_, AppState>,
    repo: String,
) -> Result<Vec<HfFile>, String> {
    state.local.hf_files(&repo).await
}

#[tauri::command]
pub async fn local_download_hf(
    state: State<'_, AppState>,
    repo: String,
    file: String,
) -> Result<LocalModel, String> {
    let model = state.local.download_hf(&repo, &file).await?;
    sync_connection(&state).await?;
    Ok(model)
}

#[tauri::command]
pub async fn local_import_ollama(
    state: State<'_, AppState>,
    name: String,
) -> Result<LocalModel, String> {
    let model = state.local.import_ollama(&name).await?;
    sync_connection(&state).await?;
    Ok(model)
}

#[tauri::command]
pub async fn local_add_file(
    state: State<'_, AppState>,
    path: String,
) -> Result<LocalModel, String> {
    let model = state.local.add_file(&PathBuf::from(path.trim())).await?;
    sync_connection(&state).await?;
    Ok(model)
}

#[tauri::command]
pub async fn local_remove_model(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.local.remove_model(&id).await?;
    sync_connection(&state).await
}

#[tauri::command]
pub async fn local_set_context(
    state: State<'_, AppState>,
    id: String,
    context: u32,
) -> Result<LocalModel, String> {
    let model = state.local.set_context(&id, context)?;
    sync_connection(&state).await?;
    Ok(model)
}

#[tauri::command]
pub fn local_settings_save(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Settings, String> {
    state.local.save_settings(settings)
}

#[tauri::command]
pub async fn local_stop(state: State<'_, AppState>) -> Result<(), String> {
    state.local.stop().await;
    Ok(())
}

#[tauri::command]
pub fn local_log(state: State<'_, AppState>) -> Vec<String> {
    state.local.log()
}

/// Removes the connection the app made for Ollama before ADR-0025.
#[tauri::command]
pub async fn local_remove_legacy(state: State<'_, AppState>) -> Result<(), String> {
    manager(&state)?
        .remove(LEGACY_OLLAMA, CallOrigin::User)
        .await
        .map_err(|e| e.message)
}

/// At startup: the local connection follows the models (they may have
/// changed while the app was closed, or the engine may be new).
pub async fn sync_at_start(state: &AppState) {
    if let Err(e) = sync_connection(state).await {
        eprintln!("[orchestrator] modelos locais: {e}");
    }
}
