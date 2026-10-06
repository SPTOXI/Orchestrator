//! Tauri commands of the offline models (ADR-0021): Ollama's state, the
//! models on this machine, downloading and removing, and the API connection
//! that makes them providers. Downloads report progress on
//! `runtime://offline`.

use crate::AppState;
use orchestrator_core::CallOrigin;
use orchestrator_provider_api::ollama::{
    self, catalog, CatalogModel, LocalModel, Ollama, OllamaStatus, PullProgress, OLLAMA_CONNECTION,
};
use orchestrator_provider_api::SaveRequest;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;

pub const OFFLINE_EVENT: &str = "runtime://offline";

/// Downloads in progress, by model.
#[derive(Default)]
pub struct Downloads(Mutex<HashMap<String, CancellationToken>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineView {
    pub status: OllamaStatus,
    /// Empty when Ollama is not running.
    pub models: Vec<LocalModel>,
    pub catalog: Vec<CatalogModel>,
    /// The connection with the offline models, when it exists.
    pub connection: Option<String>,
    pub downloading: Vec<String>,
    /// How to install Ollama on this system, run in a terminal.
    pub install_command: Option<String>,
    pub download_page: &'static str,
    pub models_error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum OfflineEvent {
    Progress(PullProgress),
    Done { model: String },
    Failed { model: String, error: String },
}

fn client() -> Result<Ollama, String> {
    Ollama::new(ollama::ollama_url()).map_err(|e| e.message)
}

fn install_command() -> Option<String> {
    if cfg!(windows) {
        Some("winget install --id Ollama.Ollama -e --accept-source-agreements --accept-package-agreements".into())
    } else if cfg!(target_os = "macos") {
        Some("brew install ollama".into())
    } else if cfg!(target_os = "linux") {
        Some("curl -fsSL https://ollama.com/install.sh | sh".into())
    } else {
        None
    }
}

#[tauri::command]
pub async fn offline_status(
    state: State<'_, AppState>,
    downloads: State<'_, Downloads>,
) -> Result<OfflineView, String> {
    let ollama = client()?;
    let status = ollama.status().await;
    let (models, models_error) = if status.running {
        match ollama.models().await {
            Ok(models) => (models, None),
            Err(err) => (Vec::new(), Some(err.message)),
        }
    } else {
        (Vec::new(), None)
    };
    let connection = state
        .connections
        .as_ref()
        .and_then(|m| m.get(OLLAMA_CONNECTION))
        .map(|c| c.id);
    let downloading = downloads.0.lock().keys().cloned().collect();
    Ok(OfflineView {
        status,
        models,
        catalog: catalog(),
        connection,
        downloading,
        install_command: install_command(),
        download_page: "https://ollama.com/download",
        models_error,
    })
}

/// Starts `ollama serve` in the background (it keeps running after the
/// Orchestrator closes, like Ollama's own app).
#[tauri::command]
pub fn offline_start() -> Result<(), String> {
    let program =
        ollama::find_program().ok_or("o Ollama não está instalado (ou não está no PATH)")?;
    let mut command = std::process::Command::new(program);
    command
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | DETACHED_PROCESS
        command.creation_flags(0x0800_0000 | 0x0000_0008);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("não foi possível iniciar o Ollama: {e}"))
}

/// Downloads a model; returns when it is done. Progress goes out as
/// events. The connection, when it exists, gets the new model.
#[tauri::command]
pub async fn offline_pull(app: AppHandle, model: String) -> Result<(), String> {
    let model = model.trim().to_owned();
    let cancel = CancellationToken::new();
    {
        let downloads = app.state::<Downloads>();
        let mut map = downloads.0.lock();
        if map.contains_key(&model) {
            return Err(format!("{model} já está sendo baixado"));
        }
        map.insert(model.clone(), cancel.clone());
    }
    let ollama = client()?;
    let emitter = app.clone();
    let result = ollama
        .pull(
            &model,
            &move |progress| {
                let _ = emitter.emit(OFFLINE_EVENT, OfflineEvent::Progress(progress));
            },
            &cancel,
        )
        .await;
    app.state::<Downloads>().0.lock().remove(&model);
    match result {
        Ok(()) => {
            let _ = app.emit(
                OFFLINE_EVENT,
                OfflineEvent::Done {
                    model: model.clone(),
                },
            );
            // Keep the connection in step, if the user already made it.
            let state = app.state::<AppState>();
            if state
                .connections
                .as_ref()
                .is_some_and(|m| m.get(OLLAMA_CONNECTION).is_some())
            {
                sync_connection(&state).await?;
            }
            Ok(())
        }
        Err(err) => {
            let error = if cancel.is_cancelled() {
                "download cancelado".to_owned()
            } else {
                err.message
            };
            let _ = app.emit(
                OFFLINE_EVENT,
                OfflineEvent::Failed {
                    model,
                    error: error.clone(),
                },
            );
            Err(error)
        }
    }
}

#[tauri::command]
pub fn offline_cancel(downloads: State<'_, Downloads>, model: String) {
    if let Some(token) = downloads.0.lock().get(&model) {
        token.cancel();
    }
}

#[tauri::command]
pub async fn offline_delete(state: State<'_, AppState>, model: String) -> Result<(), String> {
    client()?.delete(&model).await.map_err(|e| e.message)?;
    if state
        .connections
        .as_ref()
        .is_some_and(|m| m.get(OLLAMA_CONNECTION).is_some())
    {
        sync_connection(&state).await?;
    }
    Ok(())
}

/// Creates (or updates) the connection with every model on this machine.
#[tauri::command]
pub async fn offline_use(state: State<'_, AppState>) -> Result<String, String> {
    sync_connection(&state).await
}

async fn sync_connection(state: &AppState) -> Result<String, String> {
    let manager = state
        .connections
        .as_ref()
        .ok_or("as conexões de API não estão disponíveis")?;
    let ollama = client()?;
    let models = ollama.models().await.map_err(|e| e.message)?;
    if models.is_empty() {
        return Err("nenhum modelo baixado ainda".into());
    }
    let existing = manager.get(OLLAMA_CONNECTION);
    let connection = ollama::connection(ollama.url(), &models, existing.as_ref());
    let id = connection.id.clone();
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
        .map_err(|e| e.message)?;
    Ok(id)
}
