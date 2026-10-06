//! The llama.cpp engine on disk (ADR-0025): installing a release package
//! (download, SHA-256, unpack, a test run), what is installed
//! (`engine.json`), and removing it.

use crate::archive;
use crate::download::{self, Expected, OnProgress};
use crate::platform::{pick_assets, Backend, System};
use crate::sources::Release;
use crate::store::{read_json, write_json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const SERVER_PROGRAM: &str = "llama-server";
const ENGINE_FILE: &str = "engine.json";

/// The engine installed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    /// llama.cpp release (`b9000`).
    pub tag: String,
    pub backend: Backend,
    /// Release files it came from.
    pub assets: Vec<String>,
    pub dir: PathBuf,
    pub server: PathBuf,
    /// Folders with its libraries (and CUDA's).
    pub library_dirs: Vec<PathBuf>,
    /// What `llama-server --version` printed.
    #[serde(default)]
    pub version: Option<String>,
    pub installed_at: DateTime<Utc>,
}

/// `<data>/local/engine/`.
pub struct EngineDir {
    root: PathBuf,
}

impl EngineDir {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The engine installed, if its program is still there.
    pub fn load(&self) -> Option<EngineInfo> {
        let path = self.root.join(ENGINE_FILE);
        if !path.exists() {
            return None;
        }
        let (info, _) = read_json::<Option<EngineInfo>>(&path);
        info.filter(|i| i.server.is_file())
    }

    /// Downloads and installs `backend` from `release`. The previous
    /// engine is removed only after the new one runs.
    pub async fn install(
        &self,
        client: &reqwest::Client,
        system: &System,
        release: &Release,
        backend: Backend,
        on_progress: OnProgress<'_>,
        cancel: &CancellationToken,
    ) -> Result<EngineInfo, String> {
        let patterns = system
            .asset_patterns(backend, &release.tag)
            .ok_or_else(|| format!("não há motor {} para este sistema", backend.label()))?;
        let names: Vec<String> = release.assets.iter().map(|a| a.name.clone()).collect();
        let picked = pick_assets(&patterns, &names)?;
        let assets: Vec<_> = picked
            .iter()
            .filter_map(|name| release.assets.iter().find(|a| &a.name == name))
            .collect();
        for asset in &assets {
            if asset.sha256.is_none() {
                return Err(format!(
                    "o GitHub não publicou a conferência (SHA-256) de {}: nada foi instalado",
                    asset.name
                ));
            }
        }
        let staging = self.root.join(format!(
            ".installing-{}-{}",
            release.tag,
            backend_id(backend)
        ));
        let _ = std::fs::remove_dir_all(&staging);
        let downloads = staging.join("downloads");
        let files = staging.join("files");
        let total: u64 = assets.iter().map(|a| a.size).sum();
        let mut offset = 0u64;
        for asset in &assets {
            let dest = downloads.join(&asset.name);
            let base = offset;
            download::fetch(
                client,
                &asset.url,
                &dest,
                &Expected {
                    size: Some(asset.size),
                    sha256: asset.sha256.clone(),
                },
                &|done, _| on_progress(base + done, Some(total)),
                cancel,
            )
            .await
            .inspect_err(|_| {
                if cancel.is_cancelled() {
                    let _ = std::fs::remove_dir_all(&staging);
                }
            })?;
            offset += asset.size;
            let (archive_path, out) = (dest.clone(), files.clone());
            tokio::task::spawn_blocking(move || archive::extract(&archive_path, &out))
                .await
                .map_err(|e| e.to_string())??;
            let _ = std::fs::remove_file(&dest);
        }
        let server = archive::find_program(&files, SERVER_PROGRAM)
            .ok_or("o pacote baixado não tem o llama-server")?;
        let staged = EngineInfo {
            tag: release.tag.clone(),
            backend,
            assets: picked.clone(),
            dir: files.clone(),
            library_dirs: archive::library_dirs(&files),
            server: server.clone(),
            version: None,
            installed_at: Utc::now(),
        };
        let version = match self_test(&staged).await {
            Ok(v) => v,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
        };
        // Into place: `<tag>-<backend>`, then the old ones go.
        let final_dir = self
            .root
            .join(format!("{}-{}", release.tag, backend_id(backend)));
        let _ = std::fs::remove_dir_all(&final_dir);
        std::fs::rename(&files, &final_dir).map_err(|e| {
            format!(
                "não foi possível instalar o motor em {}: {e}",
                final_dir.display()
            )
        })?;
        let _ = std::fs::remove_dir_all(&staging);
        let relocate = |p: &Path| final_dir.join(p.strip_prefix(&files).unwrap_or(p));
        let info = EngineInfo {
            dir: final_dir.clone(),
            server: relocate(&server),
            library_dirs: staged.library_dirs.iter().map(|d| relocate(d)).collect(),
            version,
            ..staged
        };
        write_json(&self.root.join(ENGINE_FILE), &Some(&info))?;
        if let Ok(entries) = std::fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path != final_dir {
                    let _ = std::fs::remove_dir_all(&path);
                }
            }
        }
        Ok(info)
    }

    /// Removes the engine; returns what was installed.
    pub fn remove(&self) -> Result<Option<EngineInfo>, String> {
        let info = self.load();
        if self.root.exists() {
            std::fs::remove_dir_all(&self.root)
                .map_err(|e| format!("não foi possível remover o motor: {e}"))?;
        }
        Ok(info)
    }
}

pub fn backend_id(backend: Backend) -> &'static str {
    match backend {
        Backend::Cpu => "cpu",
        Backend::Vulkan => "vulkan",
        Backend::Cuda => "cuda",
        Backend::Metal => "metal",
    }
}

/// A command for the engine's program, with its libraries reachable.
pub fn command(info: &EngineInfo) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(&info.server);
    if let Some(dir) = info.server.parent() {
        cmd.current_dir(dir);
    }
    let var = if cfg!(windows) {
        "PATH"
    } else if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let mut paths: Vec<PathBuf> = info.library_dirs.clone();
    if let Some(dir) = info.server.parent() {
        if !paths.iter().any(|p| p == dir) {
            paths.insert(0, dir.to_path_buf());
        }
    }
    if let Some(existing) = std::env::var_os(var) {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env(var, joined);
    }
    cmd.stdin(std::process::Stdio::null());
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// `llama-server --version`: the package runs on this computer.
async fn self_test(info: &EngineInfo) -> Result<Option<String>, String> {
    let mut cmd = command(info);
    cmd.arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(60), cmd.output())
        .await
        .map_err(|_| "o motor baixado não respondeu ao teste (--version)".to_owned())?
        .map_err(|e| format!("o motor baixado não abre neste computador: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        let tail: Vec<&str> = text.lines().rev().take(6).collect();
        return Err(format!(
            "o motor baixado não roda neste computador ({}): {}",
            output.status,
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        ));
    }
    Ok(text
        .lines()
        .find(|l| l.contains("version"))
        .map(|l| l.trim().to_owned()))
}
