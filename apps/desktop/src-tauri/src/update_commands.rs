//! Installers and updates (ADR-0019). The updater plugin lives on the Rust
//! side only: the webview has none of its permissions and goes through
//! these commands, like everything else. The app looks for updates and
//! says so; installing one is always the user's call.

use crate::{backup_commands, AppState};
use chrono::{DateTime, Utc};
use orchestrator_core::{AuditEvent, CallOrigin, EventKind, EventSink};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Tauri event with the progress of a check or a download.
pub const UPDATE_EVENT: &str = "runtime://update";
/// The first automatic check waits for the app to settle.
const FIRST_CHECK: Duration = Duration::from_secs(15);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// A value the release build embeds; empty counts as absent.
fn baked(value: Option<&'static str>) -> Option<&'static str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// Public key that verifies updates. Builds without it never look for one.
pub fn pubkey() -> Option<&'static str> {
    baked(option_env!("ORCHESTRATOR_UPDATER_PUBKEY"))
}

/// Where the update manifest (`latest.json`) is.
pub fn endpoint() -> Option<&'static str> {
    baked(option_env!("ORCHESTRATOR_UPDATER_ENDPOINT"))
}

fn commit() -> Option<&'static str> {
    baked(option_env!("ORCHESTRATOR_COMMIT"))
}

/// Why this build does not look for updates, if it does not.
fn not_configured() -> Option<&'static str> {
    match (pubkey(), endpoint()) {
        (Some(_), Some(_)) => None,
        (None, _) => Some(
            "Este build não procura atualizações: foi compilado sem a chave pública \
             (builds de desenvolvimento e locais). Os instaladores do release a trazem.",
        ),
        (Some(_), None) => Some("Este build não sabe onde procurar atualizações."),
    }
}

/// `<app-data>/updates.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateSettings {
    /// Look for updates on its own (never install).
    pub auto_check: bool,
    /// The version of the last run, to notice an update.
    pub last_version: Option<String>,
    /// The version the updater installed, until the app opens with it.
    pub pending: Option<String>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            last_version: None,
            pending: None,
        }
    }
}

pub fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("updates.json")
}

/// `updates.json`, or the defaults with a warning when it is unreadable.
pub fn load_settings(path: &Path) -> (UpdateSettings, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (UpdateSettings::default(), None);
    };
    match serde_json::from_str(&text) {
        Ok(settings) => (settings, None),
        Err(err) => (
            UpdateSettings::default(),
            Some(format!("updates.json ignorado: {err}")),
        ),
    }
}

fn save_settings(path: &Path, settings: &UpdateSettings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    crate::files::write_atomic(path, (text + "\n").as_bytes())
}

/// The version changed since the last run: from which one, and whether the
/// updater did it (`updater`) or the user ran an installer (`installer`).
pub fn version_change(settings: &UpdateSettings, current: &str) -> Option<(String, &'static str)> {
    let previous = settings.last_version.as_deref()?;
    (previous != current).then(|| {
        let via = if settings.pending.as_deref() == Some(current) {
            "updater"
        } else {
            "installer"
        };
        (previous.to_owned(), via)
    })
}

/// A newer version the endpoint offers.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    /// Release date as the manifest says it (RFC 3339).
    pub date: Option<String>,
    /// Release notes.
    pub notes: Option<String>,
}

impl From<&Update> for UpdateInfo {
    fn from(update: &Update) -> Self {
        Self {
            version: update.version.clone(),
            current_version: update.current_version.clone(),
            date: update
                .raw_json
                .get("pub_date")
                .and_then(|d| d.as_str())
                .map(str::to_owned),
            notes: update.body.clone().filter(|b| !b.trim().is_empty()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Idle,
    Checking,
    Downloading,
    /// Installed; the new version runs after a restart.
    Installed,
}

/// What `update_status` reports.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub version: String,
    pub commit: Option<String>,
    pub os: &'static str,
    pub arch: &'static str,
    /// How the app was installed (`deb`, `rpm`, `appimage`, `msi`, `nsis`,
    /// `app`); `None` when it runs from a build.
    pub bundle: Option<String>,
    /// The build looks for updates.
    pub configured: bool,
    pub not_configured: Option<String>,
    pub endpoint: Option<String>,
    pub auto_check: bool,
    pub last_check: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub phase: Phase,
    pub available: Option<UpdateInfo>,
    pub warning: Option<String>,
}

/// Progress pushed to the webview.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum UpdateEvent {
    /// A check ended (found something or not, or failed).
    Checked,
    Available {
        info: UpdateInfo,
    },
    /// The user's data was backed up before installing (ADR-0022).
    BackedUp {
        id: String,
        label: String,
    },
    Progress {
        downloaded: u64,
        total: Option<u64>,
    },
    Installed {
        version: String,
    },
    Failed {
        message: String,
    },
}

/// Updater state, managed by Tauri next to `AppState`.
pub struct Updates {
    path: PathBuf,
    settings: Mutex<UpdateSettings>,
    warning: Option<String>,
    phase: Mutex<Phase>,
    last_check: Mutex<Option<DateTime<Utc>>>,
    last_error: Mutex<Option<String>>,
    available: Mutex<Option<Update>>,
}

impl Updates {
    /// Loads `updates.json` and notes the version this run is. Returns the
    /// change since the last run (for `APP_UPDATED`).
    pub fn open(data_dir: &Path, current: &str) -> (Self, Option<(String, &'static str)>) {
        let path = settings_path(data_dir);
        let (mut settings, warning) = load_settings(&path);
        let warning = crate::files::guard(&path, warning);
        let change = version_change(&settings, current);
        let before = settings.clone();
        settings.last_version = Some(current.to_owned());
        settings.pending = None;
        if settings != before {
            if let Err(err) = save_settings(&path, &settings) {
                eprintln!("[orchestrator] cannot write {}: {err}", path.display());
            }
        }
        let updates = Self {
            path,
            settings: Mutex::new(settings),
            warning,
            phase: Mutex::new(Phase::Idle),
            last_check: Mutex::new(None),
            last_error: Mutex::new(None),
            available: Mutex::new(None),
        };
        (updates, change)
    }

    fn status(&self, app: &AppHandle) -> UpdateStatus {
        UpdateStatus {
            version: app.package_info().version.to_string(),
            commit: commit().map(str::to_owned),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            bundle: tauri::utils::platform::bundle_type().map(|b| b.to_string()),
            configured: not_configured().is_none(),
            not_configured: not_configured().map(str::to_owned),
            endpoint: endpoint().map(str::to_owned),
            auto_check: self.settings.lock().auto_check,
            last_check: *self.last_check.lock(),
            last_error: self.last_error.lock().clone(),
            phase: *self.phase.lock(),
            available: self.available.lock().as_ref().map(UpdateInfo::from),
            warning: self.warning.clone(),
        }
    }

    fn update_settings(&self, change: impl FnOnce(&mut UpdateSettings)) -> Result<(), String> {
        let mut settings = self.settings.lock();
        let mut next = settings.clone();
        change(&mut next);
        save_settings(&self.path, &next)?;
        *settings = next;
        Ok(())
    }
}

/// The updater's errors in the user's words; the rest as they come.
fn explain(err: &tauri_plugin_updater::Error) -> String {
    use tauri_plugin_updater::Error as E;
    match err {
        E::Minisign(_) | E::SignatureUtf8(_) | E::Base64(_) => {
            "a assinatura do pacote não confere com a chave deste Orchestrator (nada foi instalado)"
                .to_owned()
        }
        E::Reqwest(_) | E::Network(_) => {
            format!("sem conexão com o servidor de atualizações ({err})")
        }
        E::ReleaseNotFound => {
            "o servidor não tem o manifesto da versão (o release já foi publicado?)".to_owned()
        }
        E::TargetNotFound(_) | E::TargetsNotFound(_) => format!(
            "o release não tem pacote para este sistema ({}/{})",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
        E::AuthenticationFailed => "a senha de administrador não foi informada".to_owned(),
        E::DebInstallFailed | E::PackageInstallFailed => {
            "o gerenciador de pacotes do sistema não instalou a atualização".to_owned()
        }
        E::InsecureTransportProtocol => "o endereço de atualização precisa ser https".to_owned(),
        _ => err.to_string(),
    }
}

/// Records `APP_UPDATED` when this run is a new version.
pub fn record_change(sink: &dyn EventSink, change: Option<(String, &'static str)>, current: &str) {
    let Some((from, via)) = change else {
        return;
    };
    sink.audit(AuditEvent::new(
        EventKind::AppUpdated,
        CallOrigin::System,
        format!("Orchestrator mudou de versão: {from} → {current}"),
        json!({ "from": from, "to": current, "via": via }),
    ));
}

/// Asks the endpoint for a newer version.
async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    if let Some(reason) = not_configured() {
        return Err(reason.to_owned());
    }
    let updates = app.state::<Updates>();
    {
        let mut phase = updates.phase.lock();
        match *phase {
            Phase::Downloading => return Err("A atualização está sendo baixada.".into()),
            Phase::Installed => {
                return Err("A atualização já foi instalada: reinicie o Orchestrator.".into())
            }
            _ => *phase = Phase::Checking,
        }
    }
    let result = async {
        let url = endpoint()
            .unwrap_or_default()
            .parse()
            .map_err(|e| format!("endereço de atualização inválido: {e}"))?;
        app.updater_builder()
            .endpoints(vec![url])
            .and_then(|builder| builder.build())
            .map_err(|e| explain(&e))?
            .check()
            .await
            .map_err(|e| explain(&e))
    }
    .await;
    *updates.phase.lock() = Phase::Idle;
    *updates.last_check.lock() = Some(Utc::now());
    let outcome = match result {
        Ok(found) => {
            let info = found.as_ref().map(UpdateInfo::from);
            *updates.available.lock() = found;
            *updates.last_error.lock() = None;
            if let Some(info) = &info {
                let _ = app.emit(UPDATE_EVENT, UpdateEvent::Available { info: info.clone() });
            }
            Ok(info)
        }
        Err(err) => {
            let message = format!("Não foi possível procurar atualizações: {err}");
            *updates.last_error.lock() = Some(message.clone());
            Err(message)
        }
    };
    // The automatic checks have no caller waiting: the UI learns here.
    let _ = app.emit(UPDATE_EVENT, UpdateEvent::Checked);
    outcome
}

/// Looks for updates 15 s after the app opens and every 6 hours, when the
/// build can and the user did not turn it off. It only tells.
pub fn start_auto_check(app: AppHandle) {
    if not_configured().is_some() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            let enabled = app.state::<Updates>().settings.lock().auto_check;
            if enabled {
                if let Err(err) = check(&app).await {
                    eprintln!("[orchestrator] {err}");
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[tauri::command]
pub fn update_status(app: AppHandle, updates: State<'_, Updates>) -> UpdateStatus {
    updates.status(&app)
}

#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    check(&app).await
}

/// Downloads, verifies and installs the update found by the last check.
/// Running agents stop first, with a handoff, as "Parar todos" does; then
/// the user's data is backed up, and without that backup nothing is
/// installed (ADR-0022). On Windows the installer closes the app and opens
/// the new version.
#[tauri::command]
pub async fn update_install(
    app: AppHandle,
    state: State<'_, AppState>,
    updates: State<'_, Updates>,
) -> Result<(), String> {
    let update = updates
        .available
        .lock()
        .clone()
        .ok_or("Procure atualizações primeiro.")?;
    {
        let mut phase = updates.phase.lock();
        if *phase != Phase::Idle {
            return Err("Já há uma atualização em andamento.".into());
        }
        *phase = Phase::Downloading;
    }
    let stopped = state
        .agents
        .stop_all(None, CallOrigin::User)
        .await
        .unwrap_or(0);
    if stopped > 0 {
        eprintln!("[orchestrator] {stopped} agent(s) stopped before the update");
    }
    let version = update.version.clone();
    let current = app.package_info().version.to_string();
    let data_dir = state.data_dir.clone();
    let store = state.store.clone();
    let label = format!("Antes de atualizar da {current} para a {version}");
    let backup = tauri::async_runtime::spawn_blocking(move || {
        let made = backup_commands::create(
            &data_dir,
            Some(&store),
            backup_commands::Reason::Update,
            &label,
            &current,
        );
        if made.is_ok() {
            backup_commands::prune(&data_dir, backup_commands::KEEP_AUTOMATIC);
        }
        made
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|made| made);
    match backup {
        Ok(info) => {
            let _ = app.emit(
                UPDATE_EVENT,
                UpdateEvent::BackedUp {
                    id: info.id,
                    label: info.label,
                },
            );
        }
        Err(err) => {
            *updates.phase.lock() = Phase::Idle;
            let message = format!(
                "A atualização não foi instalada: não foi possível fazer antes o backup dos seus dados ({err}). \
                 Libere espaço em disco e tente de novo."
            );
            *updates.last_error.lock() = Some(message.clone());
            let _ = app.emit(
                UPDATE_EVENT,
                UpdateEvent::Failed {
                    message: message.clone(),
                },
            );
            return Err(message);
        }
    }
    updates.update_settings(|s| s.pending = Some(version.clone()))?;

    let emitter = app.clone();
    let mut downloaded: u64 = 0;
    let mut reported: u64 = 0;
    let result = update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk as u64;
                // About every 1% (or 256 KiB without a size).
                let step = total.map_or(256 * 1024, |t| (t / 100).max(1));
                if downloaded - reported >= step || Some(downloaded) == total {
                    reported = downloaded;
                    let _ = emitter.emit(UPDATE_EVENT, UpdateEvent::Progress { downloaded, total });
                }
            },
            || {},
        )
        .await;
    match result {
        Ok(()) => {
            *updates.phase.lock() = Phase::Installed;
            let _ = app.emit(UPDATE_EVENT, UpdateEvent::Installed { version });
            Ok(())
        }
        Err(err) => {
            *updates.phase.lock() = Phase::Idle;
            let _ = updates.update_settings(|s| s.pending = None);
            let message = format!("A atualização não foi instalada: {}", explain(&err));
            *updates.last_error.lock() = Some(message.clone());
            let _ = app.emit(
                UPDATE_EVENT,
                UpdateEvent::Failed {
                    message: message.clone(),
                },
            );
            Err(message)
        }
    }
}

/// Opens the installed version.
#[tauri::command]
pub fn update_restart(app: AppHandle) {
    app.restart();
}

#[tauri::command]
pub fn update_settings_save(
    app: AppHandle,
    updates: State<'_, Updates>,
    auto_check: bool,
) -> Result<UpdateStatus, String> {
    updates.update_settings(|s| s.auto_check = auto_check)?;
    Ok(updates.status(&app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_tolerate_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        assert_eq!(load_settings(&path), (UpdateSettings::default(), None));
        assert!(UpdateSettings::default().auto_check, "on by default");

        let settings = UpdateSettings {
            auto_check: false,
            last_version: Some("0.1.0".into()),
            pending: Some("0.2.0".into()),
        };
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path), (settings, None));

        std::fs::write(&path, "{ quebrado").unwrap();
        let (fallback, warning) = load_settings(&path);
        assert_eq!(fallback, UpdateSettings::default());
        assert!(warning.unwrap().starts_with("updates.json ignorado"));
    }

    #[test]
    fn a_new_version_is_noticed_and_says_how_it_came() {
        let mut settings = UpdateSettings::default();
        assert_eq!(version_change(&settings, "0.1.0"), None, "first run");

        settings.last_version = Some("0.1.0".into());
        assert_eq!(version_change(&settings, "0.1.0"), None, "same version");
        assert_eq!(
            version_change(&settings, "0.2.0"),
            Some(("0.1.0".into(), "installer"))
        );

        settings.pending = Some("0.2.0".into());
        assert_eq!(
            version_change(&settings, "0.2.0"),
            Some(("0.1.0".into(), "updater"))
        );
    }

    #[test]
    fn opening_records_this_version_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        save_settings(
            &path,
            &UpdateSettings {
                auto_check: false,
                last_version: Some("0.1.0".into()),
                pending: Some("0.2.0".into()),
            },
        )
        .unwrap();

        let (updates, change) = Updates::open(dir.path(), "0.2.0");
        assert_eq!(change, Some(("0.1.0".into(), "updater")));
        let saved = load_settings(&path).0;
        assert_eq!(saved.last_version.as_deref(), Some("0.2.0"));
        assert_eq!(saved.pending, None);
        assert!(!saved.auto_check, "the user's choice stays");
        assert!(!updates.settings.lock().auto_check);

        let (_, again) = Updates::open(dir.path(), "0.2.0");
        assert_eq!(again, None);
    }

    #[test]
    fn updater_errors_are_told_in_words() {
        use tauri_plugin_updater::Error as E;
        assert!(explain(&E::ReleaseNotFound).contains("manifesto"));
        assert!(explain(&E::TargetNotFound("linux-x86_64-deb".into())).contains("este sistema"));
        assert!(explain(&E::SignatureUtf8("x".into())).contains("assinatura"));
        assert!(explain(&E::AuthenticationFailed).contains("senha"));
        assert_eq!(explain(&E::EmptyEndpoints), E::EmptyEndpoints.to_string());
    }

    #[test]
    fn builds_without_the_key_do_not_look_for_updates() {
        // Test builds are never compiled with the release key.
        if pubkey().is_none() {
            assert!(not_configured().unwrap().contains("sem a chave pública"));
        }
    }
}
