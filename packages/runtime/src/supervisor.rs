//! Supervision of the processes the runtime starts (ADR-0018): none of
//! them should outlive the app, even when the app dies abruptly (crash,
//! `kill -9`, `taskkill /F`).
//!
//! - **Windows:** every process joins a Job Object created with "kill on
//!   job close". When the app dies, Windows closes the job and ends the
//!   process and everything it started.
//! - **Linux and macOS:** there is no portable equivalent. The process
//!   groups `process.start` creates are written to a registry file; when
//!   the app opens again, it ends the groups a crash left behind — after
//!   checking that the pid still belongs to the same process (its start
//!   time), so a reused pid is never killed.

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A process group a previous run left behind and that was ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Orphan {
    pub pid: u32,
    pub command: String,
    pub started_at: DateTime<Utc>,
}

/// One supervised process group, as written to the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    pid: u32,
    /// What makes the pid this process and no other (start time).
    identity: String,
    command: String,
    started_at: DateTime<Utc>,
}

pub struct Supervisor {
    #[cfg(windows)]
    job: Option<job::Job>,
    /// Registry file (Unix); `None` = nothing survives a restart to reap.
    registry: Mutex<Option<PathBuf>>,
    entries: Mutex<Vec<Entry>>,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Supervisor {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            job: job::Job::new(),
            registry: Mutex::new(None),
            entries: Mutex::new(Vec::new()),
        }
    }

    /// Takes a process just started. On Windows it joins the job; on Unix,
    /// with `register`, its group (pid = pgid, `configure_process_tree`)
    /// is written to the registry.
    pub fn adopt(&self, child: &tokio::process::Child, command: &str, register: bool) {
        #[cfg(windows)]
        {
            let _ = (command, register);
            if let (Some(job), Some(handle)) = (&self.job, child.raw_handle()) {
                if !job.assign(handle) {
                    eprintln!(
                        "[orchestrator] process {:?} could not join the supervision job",
                        child.id()
                    );
                }
            }
        }
        #[cfg(unix)]
        {
            let Some(pid) = child.id().filter(|_| register) else {
                return;
            };
            if self.registry.lock().is_none() {
                return;
            }
            let Some(identity) = identity(pid) else {
                return;
            };
            self.entries.lock().push(Entry {
                pid,
                identity,
                command: command.chars().take(500).collect(),
                started_at: Utc::now(),
            });
            self.write();
        }
        #[cfg(not(any(unix, windows)))]
        let _ = (child, command, register);
    }

    /// The process ended (or was stopped): nothing to reap anymore.
    pub fn release(&self, pid: Option<u32>) {
        let Some(pid) = pid else {
            return;
        };
        let removed = {
            let mut entries = self.entries.lock();
            let before = entries.len();
            entries.retain(|e| e.pid != pid);
            entries.len() != before
        };
        if removed {
            self.write();
        }
    }

    /// Uses `path` as the registry and ends what a previous run left in
    /// it. Returns the groups that were ended. Unix only; on Windows the
    /// job already took care of them.
    pub fn open_registry(&self, path: &Path) -> Vec<Orphan> {
        let left: Vec<Entry> = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        *self.registry.lock() = Some(path.to_path_buf());
        let reaped = left.into_iter().filter_map(reap).collect();
        // What is running now (nothing, at startup) replaces the old list.
        self.write();
        reaped
    }

    fn write(&self) {
        let Some(path) = self.registry.lock().clone() else {
            return;
        };
        let entries = self.entries.lock().clone();
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&entries)?)?;
            std::fs::rename(&tmp, &path)
        })();
        if let Err(err) = result {
            eprintln!("[orchestrator] cannot write {}: {err}", path.display());
        }
    }
}

/// Ends a group a previous run left behind, if it is still that process.
#[cfg(unix)]
fn reap(entry: Entry) -> Option<Orphan> {
    (identity(entry.pid).as_deref() == Some(entry.identity.as_str()) && end_group(entry.pid))
        .then_some(Orphan {
            pid: entry.pid,
            command: entry.command,
            started_at: entry.started_at,
        })
}

#[cfg(not(unix))]
fn reap(_entry: Entry) -> Option<Orphan> {
    None
}

/// What tells this process apart from a later one with the same pid.
#[cfg(target_os = "linux")]
fn identity(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields after the command (which may contain spaces and parentheses):
    // state is field 3, starttime field 22.
    let rest = &stat[stat.rfind(')')? + 1..];
    let start = rest.split_whitespace().nth(19)?;
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default();
    Some(format!("{}:{start}", boot.trim()))
}

#[cfg(target_os = "macos")]
fn identity(pid: u32) -> Option<String> {
    // SAFETY: `proc_pidinfo` writes at most `size` bytes into `info`, a
    // plain C struct for which all-zero is a valid value.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let read = libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        );
        (read == size).then(|| format!("{}.{}", info.pbi_start_tvsec, info.pbi_start_tvusec))
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn identity(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// SIGTERM to the group, a moment to finish, then SIGKILL. True when the
/// group was there.
#[cfg(unix)]
fn end_group(pgid: u32) -> bool {
    let Ok(pgid) = libc::pid_t::try_from(pgid) else {
        return false;
    };
    // SAFETY: `kill` has no memory-safety preconditions; a negative pid
    // targets the group.
    let alive = || unsafe { libc::kill(-pgid, 0) == 0 };
    if !alive() {
        return false;
    }
    unsafe { libc::kill(-pgid, libc::SIGTERM) };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while alive() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    if alive() {
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
    }
    true
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::RawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// A Job Object that ends its processes when its last handle closes —
    /// when the runtime is dropped, or when the app dies.
    pub struct Job(HANDLE);

    // SAFETY: a job handle can be used from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> Option<Self> {
            // SAFETY: plain Win32 calls; the handle is closed on failure
            // and on drop.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    CloseHandle(handle);
                    return None;
                }
                Some(Self(handle))
            }
        }

        pub fn assign(&self, process: RawHandle) -> bool {
            // SAFETY: `process` is a live process handle owned by tokio's
            // `Child`; the job handle is open.
            unsafe { AssignProcessToJobObject(self.0, process as HANDLE) != 0 }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle came from `CreateJobObjectW`.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The app dies, its job closes, its processes end. Dropping the
    /// supervisor closes the job the same way.
    #[tokio::test(flavor = "multi_thread")]
    async fn closing_the_job_ends_its_processes() {
        let supervisor = Supervisor::new();
        let mut child = tokio::process::Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        supervisor.adopt(&child, "ping", false);
        assert!(child.try_wait().unwrap().is_none(), "running");
        drop(supervisor);
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("ended with the job")
            .unwrap();
        assert!(!status.success());
    }
}
