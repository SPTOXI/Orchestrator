//! Backups of the user's data (ADR-0022). An update must never cost what
//! was built with the version it replaces, so a copy of everything is made
//! before installing an update, when a new version opens, before the
//! database is migrated, and whenever the user asks. Restoring one happens
//! on the next start, before anything opens the files, and the state it
//! replaces is backed up first.

use crate::files::write_atomic;
use crate::update_commands;
use crate::AppState;
use chrono::{DateTime, Utc};
use orchestrator_core::CallOrigin;
use orchestrator_memory::{schema_of, snapshot_file, MemoryStore, SCHEMA_VERSION};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};

/// `<app-data>/backups`.
pub const DIR: &str = "backups";
const DATABASE: &str = "orchestrator.db";
const MANIFEST: &str = "manifest.json";
/// Asks the next start to restore a backup.
const MARKER: &str = "restore-pending.json";
const SKILLS: &str = "skills";
/// Automatic backups kept; the ones the user makes are never removed.
pub const KEEP_AUTOMATIC: usize = 5;
/// Files of a run, not the user's data: never copied, never restored.
const RUN_STATE: &[&str] = &["updates.json", "processes.json", MARKER];

/// One backup at a time, and no restore request in the middle of one.
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// The user asked.
    Manual,
    /// Right before the updater installs a new version.
    Update,
    /// A new version opened (an installer the user ran).
    NewVersion,
    /// Right before the database is migrated to a newer schema.
    Migration,
    /// The state a restore replaced.
    BeforeRestore,
}

impl Reason {
    fn slug(self) -> &'static str {
        match self {
            Reason::Manual => "manual",
            Reason::Update => "atualizacao",
            Reason::NewVersion => "versao-nova",
            Reason::Migration => "migracao",
            Reason::BeforeRestore => "antes-de-restaurar",
        }
    }

    fn automatic(self) -> bool {
        self != Reason::Manual
    }
}

/// `manifest.json` of a backup, and what the screen lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    /// The folder name in `backups/`.
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub reason: Reason,
    /// Why, in the user's words.
    pub label: String,
    /// The version that made it.
    pub app_version: String,
    /// The database schema in it (`None`: no database yet).
    pub schema: Option<i64>,
    /// Settings files, relative to the data folder (`skills/…` included).
    pub files: Vec<String>,
    pub size_bytes: u64,
}

fn is_user_file(name: &str) -> bool {
    !name.starts_with('.')
        && (name.ends_with(".json") || name.ends_with(".md"))
        && !RUN_STATE.contains(&name)
}

fn copy_tree(from: &Path, to: &Path, base: &Path, files: &mut Vec<String>) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    let entries =
        std::fs::read_dir(from).map_err(|e| format!("cannot read {}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            copy_tree(&source, &target, base, files)?;
        } else if kind.is_file() {
            std::fs::copy(&source, &target)
                .map_err(|e| format!("cannot copy {}: {e}", source.display()))?;
            if let Ok(relative) = source.strip_prefix(base) {
                files.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(())
}

fn size_of(path: &Path) -> u64 {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::read_dir(path)
            .map(|entries| entries.flatten().map(|e| size_of(&e.path())).sum())
            .unwrap_or(0),
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}

fn new_id(backups: &Path, reason: Reason, at: DateTime<Utc>) -> String {
    let base = format!("{}-{}", at.format("%Y%m%d-%H%M%S"), reason.slug());
    let mut id = base.clone();
    let mut n = 2;
    while backups.join(&id).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Copies the user's data into `backups/<id>`: the database (from `store`
/// while the app uses it, or from the file when nothing has it open), the
/// settings (`*.json`, `*.md`) and the skills. The folder only appears
/// once the copy is complete.
pub fn create(
    data_dir: &Path,
    store: Option<&MemoryStore>,
    reason: Reason,
    label: &str,
    app_version: &str,
) -> Result<BackupInfo, String> {
    let _guard = LOCK.lock();
    create_locked(data_dir, store, reason, label, app_version)
}

fn create_locked(
    data_dir: &Path,
    store: Option<&MemoryStore>,
    reason: Reason,
    label: &str,
    app_version: &str,
) -> Result<BackupInfo, String> {
    let backups = data_dir.join(DIR);
    let created_at = Utc::now();
    let id = new_id(&backups, reason, created_at);
    let partial = backups.join(format!(".partial-{id}"));
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial)
        .map_err(|e| format!("cannot create {}: {e}", partial.display()))?;
    let result = fill(data_dir, store, &partial).and_then(|(files, schema)| {
        let info = BackupInfo {
            id: id.clone(),
            created_at,
            reason,
            label: label.to_owned(),
            app_version: app_version.to_owned(),
            schema,
            files,
            size_bytes: size_of(&partial),
        };
        let text = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
        write_atomic(&partial.join(MANIFEST), text.as_bytes())?;
        std::fs::rename(&partial, backups.join(&id))
            .map_err(|e| format!("cannot finish the backup: {e}"))?;
        Ok(info)
    });
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&partial);
    }
    result
}

fn fill(
    data_dir: &Path,
    store: Option<&MemoryStore>,
    into: &Path,
) -> Result<(Vec<String>, Option<i64>), String> {
    let database = data_dir.join(DATABASE);
    let copy = into.join(DATABASE);
    let live = store.filter(|s| s.path() == Some(database.as_path()));
    match live {
        Some(store) => store.backup_to(&copy)?,
        // In memory, or not open yet: the file as it is on disk.
        None => {
            if schema_of(&database)?.is_some() {
                snapshot_file(&database, &copy)?;
            }
        }
    }
    let schema = schema_of(&copy)?;
    let mut files = Vec::new();
    let entries = std::fs::read_dir(data_dir)
        .map_err(|e| format!("cannot read {}: {e}", data_dir.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_file() && is_user_file(&name) {
            std::fs::copy(entry.path(), into.join(&name))
                .map_err(|e| format!("cannot copy {name}: {e}"))?;
            files.push(name);
        } else if kind.is_dir() && name == SKILLS {
            copy_tree(&entry.path(), &into.join(SKILLS), data_dir, &mut files)?;
        }
    }
    files.sort();
    Ok((files, schema))
}

/// Every complete backup, newest first.
pub fn list(data_dir: &Path) -> Vec<BackupInfo> {
    let mut found: Vec<BackupInfo> = std::fs::read_dir(data_dir.join(DIR))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .filter_map(|e| std::fs::read_to_string(e.path().join(MANIFEST)).ok())
                .filter_map(|text| serde_json::from_str(&text).ok())
                .collect()
        })
        .unwrap_or_default();
    found.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
    found
}

/// Removes automatic backups beyond the newest `keep`, and copies a crash
/// left half made. The user's own backups stay until the user removes them.
pub fn prune(data_dir: &Path, keep: usize) {
    let _guard = LOCK.lock();
    let backups = data_dir.join(DIR);
    if let Ok(entries) = std::fs::read_dir(&backups) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(".partial-") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    for old in list(data_dir)
        .into_iter()
        .filter(|b| b.reason.automatic())
        .skip(keep)
    {
        let _ = std::fs::remove_dir_all(backups.join(&old.id));
    }
}

fn backup_dir(data_dir: &Path, id: &str) -> Result<PathBuf, String> {
    let valid = !id.is_empty()
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let dir = data_dir.join(DIR).join(id);
    if !valid || !dir.join(MANIFEST).is_file() {
        return Err(format!("o backup {id} não existe"));
    }
    Ok(dir)
}

/// Removes a backup the user made.
pub fn delete(data_dir: &Path, id: &str) -> Result<(), String> {
    let _guard = LOCK.lock();
    let dir = backup_dir(data_dir, id)?;
    std::fs::remove_dir_all(&dir).map_err(|e| format!("não foi possível apagar o backup: {e}"))
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreRequest {
    id: String,
    /// The backup of the state being replaced, once made.
    #[serde(default)]
    safety: Option<String>,
}

/// Asks the next start to restore `id`.
pub fn request_restore(data_dir: &Path, id: &str) -> Result<(), String> {
    let _guard = LOCK.lock();
    backup_dir(data_dir, id)?;
    let request = RestoreRequest {
        id: id.to_owned(),
        safety: None,
    };
    let text = serde_json::to_string_pretty(&request).map_err(|e| e.to_string())?;
    write_atomic(&data_dir.join(MARKER), text.as_bytes())
}

/// What a restore puts back in place: the database (with its WAL files)
/// and the user's files.
fn current_entries(data_dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| data_dir.join(format!("{DATABASE}{suffix}")))
        .filter(|p| p.exists())
        .collect();
    if let Ok(read) = std::fs::read_dir(data_dir) {
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_file = entry.file_type().is_ok_and(|t| t.is_file());
            if (is_file && is_user_file(&name)) || name == SKILLS {
                entries.push(entry.path());
            }
        }
    }
    entries
}

/// Restores the backup the last run asked for, if any. Runs at start,
/// before anything opens the files; the state it replaces is backed up
/// first, and without that backup nothing is restored.
pub fn apply_pending_restore(data_dir: &Path, app_version: &str) -> Option<Result<String, String>> {
    let marker = data_dir.join(MARKER);
    let text = std::fs::read_to_string(&marker).ok()?;
    let _guard = LOCK.lock();
    let finish = |result: Result<String, String>| {
        let _ = std::fs::remove_file(&marker);
        Some(result)
    };
    let mut request: RestoreRequest = match serde_json::from_str(&text) {
        Ok(request) => request,
        Err(err) => return finish(Err(format!("pedido de restauração ilegível: {err}"))),
    };
    let source = match backup_dir(data_dir, &request.id) {
        Ok(dir) => dir,
        Err(err) => return finish(Err(format!("nada foi restaurado: {err}"))),
    };
    let info: BackupInfo = match std::fs::read_to_string(source.join(MANIFEST))
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(info) => info,
        Err(err) => return finish(Err(format!("nada foi restaurado: backup ilegível ({err})"))),
    };
    if request.safety.is_none() {
        match create_locked(
            data_dir,
            None,
            Reason::BeforeRestore,
            &format!("Antes de restaurar o backup de {}", info.label),
            app_version,
        ) {
            Ok(safety) => {
                request.safety = Some(safety.id);
                if let Ok(text) = serde_json::to_string_pretty(&request) {
                    let _ = write_atomic(&marker, text.as_bytes());
                }
            }
            Err(err) => {
                return finish(Err(format!(
                    "nada foi restaurado: não foi possível guardar antes o estado atual ({err})"
                )))
            }
        }
    }
    let result = restore_files(data_dir, &source, &info).map(|()| {
        format!(
            "Dados restaurados do backup \"{}\" ({}). O estado anterior está no backup {}.",
            info.label,
            info.created_at.format("%d/%m/%Y %H:%M UTC"),
            request.safety.as_deref().unwrap_or("-")
        )
    });
    finish(result.map_err(|err| {
        format!(
            "a restauração parou no meio ({err}); o estado anterior está no backup {}",
            request.safety.as_deref().unwrap_or("-")
        )
    }))
}

fn restore_files(data_dir: &Path, source: &Path, info: &BackupInfo) -> Result<(), String> {
    // The backup is copied next to the data first: if that fails, nothing
    // was touched.
    let staged = data_dir.join(".restoring");
    let _ = std::fs::remove_dir_all(&staged);
    let mut names: Vec<String> = info.files.clone();
    if info.schema.is_some() {
        names.push(DATABASE.to_owned());
    }
    for name in &names {
        let target = staged.join(name);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        if let Err(err) = std::fs::copy(source.join(name), &target) {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(format!("cannot read {name} from the backup: {err}"));
        }
    }
    for entry in current_entries(data_dir) {
        remove_patiently(&entry)?;
    }
    for name in &names {
        let target = data_dir.join(name);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::rename(staged.join(name), &target)
            .map_err(|e| format!("cannot restore {name}: {e}"))?;
    }
    let _ = std::fs::remove_dir_all(&staged);
    Ok(())
}

/// Removes a file or folder, waiting a little for the run that asked for
/// the restore to let go of it (Windows does not remove open files).
fn remove_patiently(path: &Path) -> Result<(), String> {
    let mut tries = 0;
    loop {
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        match removed {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) if tries < 40 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(err) => return Err(format!("cannot remove {}: {err}", path.display())),
        }
    }
}

/// What the start did to keep the data safe.
#[derive(Debug, Default)]
pub struct StartBackup {
    pub made: Option<BackupInfo>,
    pub error: Option<String>,
    /// The database needs migrating and could not be copied first: it
    /// must not be opened (the app runs on an in-memory one).
    pub hold_database: bool,
}

/// At start, before the database opens: backs up the data when this is a
/// new version the updater did not install (it backs up before
/// installing), and always before the database is migrated.
pub fn on_start(data_dir: &Path, app_version: &str) -> StartBackup {
    let (settings, _) = update_commands::load_settings(&update_commands::settings_path(data_dir));
    let installer = settings.last_version.as_deref().filter(|previous| {
        *previous != app_version && settings.pending.as_deref() != Some(app_version)
    });
    let migration = match schema_of(&data_dir.join(DATABASE)) {
        Ok(Some(schema)) if (1..SCHEMA_VERSION).contains(&schema) => Some(schema),
        _ => None,
    };
    let (reason, label) = match (installer, migration) {
        (None, None) => return StartBackup::default(),
        (Some(previous), None) => (
            Reason::NewVersion,
            format!("Versão nova aberta: {previous} → {app_version}"),
        ),
        (previous, Some(schema)) => (
            Reason::Migration,
            match previous {
                Some(previous) => format!(
                    "Versão nova aberta: {previous} → {app_version}; antes de atualizar o banco (esquema {schema} → {SCHEMA_VERSION})"
                ),
                None => format!(
                    "Antes de atualizar o banco (esquema {schema} → {SCHEMA_VERSION})"
                ),
            },
        ),
    };
    match create(data_dir, None, reason, &label, app_version) {
        Ok(info) => {
            prune(data_dir, KEEP_AUTOMATIC);
            StartBackup {
                made: Some(info),
                ..Default::default()
            }
        }
        Err(err) => StartBackup {
            error: Some(err),
            hold_database: migration.is_some(),
            ..Default::default()
        },
    }
}

// ------------------------------------------------------------- commands

/// What "Dados e backups" shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStatus {
    pub data_dir: String,
    pub backups_dir: String,
    pub keep_automatic: usize,
    pub backups: Vec<BackupInfo>,
    /// What happened to the data at this start (backups, restores, files
    /// kept aside), newest last.
    pub notices: Vec<String>,
}

fn status(state: &AppState) -> BackupStatus {
    BackupStatus {
        data_dir: state.data_dir.display().to_string(),
        backups_dir: state.data_dir.join(DIR).display().to_string(),
        keep_automatic: KEEP_AUTOMATIC,
        backups: list(&state.data_dir),
        notices: state.data_notices.clone(),
    }
}

#[tauri::command]
pub fn backup_status(state: State<'_, AppState>) -> BackupStatus {
    status(&state)
}

#[tauri::command]
pub async fn backup_create(
    app: AppHandle,
    state: State<'_, AppState>,
    label: Option<String>,
) -> Result<BackupStatus, String> {
    let label = label
        .map(|l| l.trim().chars().take(120).collect::<String>())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "Backup manual".to_owned());
    let version = app.package_info().version.to_string();
    let data_dir = state.data_dir.clone();
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        create(&data_dir, Some(&store), Reason::Manual, &label, &version)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|err| format!("O backup não foi feito: {err}"))?;
    Ok(status(&state))
}

#[tauri::command]
pub fn backup_delete(state: State<'_, AppState>, id: String) -> Result<BackupStatus, String> {
    delete(&state.data_dir, &id)?;
    Ok(status(&state))
}

/// Restores a backup: running agents stop with a handoff, and the app
/// restarts to put the files back before anything opens them.
#[tauri::command]
pub async fn backup_restore(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    request_restore(&state.data_dir, &id)?;
    let _ = state.agents.stop_all(None, CallOrigin::User).await;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator_core::{AuditEvent, EventKind};
    use serde_json::json;

    fn write(dir: &Path, name: &str, text: &str) {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(dir: &Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap()
    }

    fn opened(path: &str) -> AuditEvent {
        AuditEvent::new(
            EventKind::ProjectOpened,
            CallOrigin::User,
            "opened",
            json!({"name": "p", "path": path}),
        )
    }

    /// A data folder as a user leaves it: database, settings, skills, and
    /// the files of a run.
    fn user_data(dir: &Path) -> MemoryStore {
        let (store, warning) = MemoryStore::open(&dir.join(DATABASE));
        assert!(warning.is_none());
        store.record(&opened("/p/one"));
        write(dir, "connections.json", r#"{"connections":["deepseek"]}"#);
        write(dir, "rules.md", "Sempre rodar os testes.");
        write(
            dir,
            "skills/revisar-pr/SKILL.md",
            "---\nname: revisar-pr\n---\nLeia o diff.",
        );
        write(dir, "updates.json", r#"{"lastVersion":"0.1.0"}"#);
        write(dir, "processes.json", "[]");
        std::fs::create_dir_all(dir.join("cli/session")).unwrap();
        write(dir, "cli/session/system.md", "scratch");
        store
    }

    /// The data folder comes from the app identifier and the vault entries
    /// from the service name: changing either would make an update look
    /// like a first install, with nothing of the user's.
    #[test]
    fn the_data_folder_and_the_vault_keep_their_names() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], "dev.orchestrator.desktop");
        for service in [
            crate::vault::SERVICE,
            crate::github_commands::SERVICE,
            crate::secret_commands::SERVICE,
        ] {
            assert_eq!(service, "dev.orchestrator.desktop");
        }
    }

    #[test]
    fn a_backup_has_the_database_settings_and_skills_but_not_run_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = user_data(dir.path());
        let info = create(
            dir.path(),
            Some(&store),
            Reason::Manual,
            "Antes de mexer",
            "0.1.0",
        )
        .unwrap();
        assert_eq!(info.schema, Some(SCHEMA_VERSION));
        assert_eq!(
            info.files,
            ["connections.json", "rules.md", "skills/revisar-pr/SKILL.md"]
        );
        assert!(info.size_bytes > 0);
        let folder = dir.path().join(DIR).join(&info.id);
        assert_eq!(read(&folder, "rules.md"), "Sempre rodar os testes.");
        assert!(!folder.join("updates.json").exists());
        assert!(!folder.join("cli").exists());
        assert_eq!(list(dir.path()), [info]);
    }

    #[test]
    fn only_the_newest_automatic_backups_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = user_data(dir.path());
        let mine = create(dir.path(), Some(&store), Reason::Manual, "meu", "0.1.0").unwrap();
        for _ in 0..KEEP_AUTOMATIC + 2 {
            create(dir.path(), Some(&store), Reason::Update, "auto", "0.1.0").unwrap();
        }
        std::fs::create_dir_all(dir.path().join(DIR).join(".partial-crashed")).unwrap();
        prune(dir.path(), KEEP_AUTOMATIC);
        let left = list(dir.path());
        assert_eq!(left.len(), KEEP_AUTOMATIC + 1);
        assert!(left.contains(&mine), "the user's backup stays");
        assert!(!dir.path().join(DIR).join(".partial-crashed").exists());
        delete(dir.path(), &mine.id).unwrap();
        assert!(delete(dir.path(), "../..").is_err());
        assert_eq!(list(dir.path()).len(), KEEP_AUTOMATIC);
    }

    #[test]
    fn a_restore_puts_everything_back_and_keeps_what_it_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let backup = {
            let store = user_data(dir.path());
            create(
                dir.path(),
                Some(&store),
                Reason::Manual,
                "bom estado",
                "0.1.0",
            )
            .unwrap()
        };
        // Later: more history, a changed rule, a new file, a skill gone.
        {
            let (store, _) = MemoryStore::open(&dir.path().join(DATABASE));
            store.record(&opened("/p/two"));
        }
        write(dir.path(), "rules.md", "regra nova");
        write(dir.path(), "mcp.json", "{}");
        std::fs::remove_dir_all(dir.path().join("skills")).unwrap();

        request_restore(dir.path(), &backup.id).unwrap();
        assert!(request_restore(dir.path(), "nope").is_err());
        let message = apply_pending_restore(dir.path(), "0.2.0").unwrap().unwrap();
        assert!(message.contains("bom estado"), "{message}");
        assert!(
            apply_pending_restore(dir.path(), "0.2.0").is_none(),
            "only once"
        );

        assert_eq!(read(dir.path(), "rules.md"), "Sempre rodar os testes.");
        assert!(!dir.path().join("mcp.json").exists());
        assert!(dir.path().join("skills/revisar-pr/SKILL.md").exists());
        assert_eq!(
            read(dir.path(), "updates.json"),
            r#"{"lastVersion":"0.1.0"}"#
        );
        let (store, _) = MemoryStore::open(&dir.path().join(DATABASE));
        assert!(store.project_by_path("/p/one").is_some());
        assert!(store.project_by_path("/p/two").is_none());

        // What was replaced is a backup of its own, restorable in turn.
        let safety = list(dir.path())
            .into_iter()
            .find(|b| b.reason == Reason::BeforeRestore)
            .unwrap();
        assert!(safety.files.contains(&"mcp.json".to_owned()));
        let folder = dir.path().join(DIR).join(&safety.id);
        assert_eq!(read(&folder, "rules.md"), "regra nova");
        let (before, _) = MemoryStore::open(&folder.join(DATABASE));
        assert!(before.project_by_path("/p/two").is_some());
    }

    #[test]
    fn a_new_version_from_an_installer_is_backed_up_at_start() {
        let dir = tempfile::tempdir().unwrap();
        drop(user_data(dir.path()));
        // Same version: nothing to do.
        let start = on_start(dir.path(), "0.1.0");
        assert!(start.made.is_none() && start.error.is_none());
        // The updater installed 0.2.0: it backed up before installing.
        write(
            dir.path(),
            "updates.json",
            r#"{"lastVersion":"0.1.0","pending":"0.2.0"}"#,
        );
        assert!(on_start(dir.path(), "0.2.0").made.is_none());
        // The user ran the 0.2.0 installer.
        write(dir.path(), "updates.json", r#"{"lastVersion":"0.1.0"}"#);
        let start = on_start(dir.path(), "0.2.0");
        let made = start.made.unwrap();
        assert_eq!(made.reason, Reason::NewVersion);
        assert_eq!(made.label, "Versão nova aberta: 0.1.0 → 0.2.0");
        assert_eq!(made.schema, Some(SCHEMA_VERSION));
        // A first install has nothing to keep.
        let fresh = tempfile::tempdir().unwrap();
        assert!(on_start(fresh.path(), "0.2.0").made.is_none());
    }

    #[test]
    fn the_database_is_copied_before_any_migration() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join(DATABASE);
        {
            let conn = rusqlite::Connection::open(&database).unwrap();
            conn.execute_batch(
                "CREATE TABLE projects (id TEXT); INSERT INTO projects VALUES ('old');",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
        }
        let start = on_start(dir.path(), "0.2.0");
        let made = start.made.unwrap();
        assert_eq!(made.reason, Reason::Migration);
        assert_eq!(made.schema, Some(1));
        assert!(!start.hold_database);
        // The file itself was not touched.
        assert_eq!(schema_of(&database).unwrap(), Some(1));
    }

    #[test]
    fn without_a_copy_the_database_is_not_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join(DATABASE);
        {
            let conn = rusqlite::Connection::open(&database).unwrap();
            conn.pragma_update(None, "user_version", 2).unwrap();
            conn.execute_batch("CREATE TABLE t (x);").unwrap();
        }
        // The backups folder cannot be created: a file is in its place.
        write(dir.path(), DIR, "not a folder");
        let start = on_start(dir.path(), "0.2.0");
        assert!(start.made.is_none());
        assert!(start.error.is_some());
        assert!(start.hold_database);
    }
}
