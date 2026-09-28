//! Durable audit trail for Phase 1 (ADR-0005): JSON Lines file plus an
//! in-memory window of recent events. Replaced by SQLite in Phase 6.

use orchestrator_core::AuditEvent;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Recent events kept in memory for `history_recent`.
pub const RECENT_CAPACITY: usize = 1000;

pub struct AuditLog {
    path: PathBuf,
    file: Mutex<Option<File>>,
    recent: Mutex<VecDeque<AuditEvent>>,
}

impl AuditLog {
    /// Opens (or creates) the log file. If the file cannot be opened the log
    /// keeps working in memory and reports the problem on stderr: losing the
    /// file must never stop the app.
    pub fn open(path: &Path) -> Self {
        let file = path
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| OpenOptions::new().create(true).append(true).open(path));
        let file = match file {
            Ok(file) => Some(file),
            Err(err) => {
                eprintln!(
                    "[orchestrator] cannot open audit log {}: {err}",
                    path.display()
                );
                None
            }
        };
        Self {
            path: path.to_path_buf(),
            file: Mutex::new(file),
            recent: Mutex::new(VecDeque::with_capacity(RECENT_CAPACITY)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event: &AuditEvent) {
        if let Some(file) = self.file.lock().as_mut() {
            let written = serde_json::to_string(event)
                .map_err(std::io::Error::other)
                .and_then(|line| writeln!(file, "{line}"))
                .and_then(|_| file.flush());
            if let Err(err) = written {
                eprintln!("[orchestrator] cannot write audit log: {err}");
            }
        }
        let mut recent = self.recent.lock();
        if recent.len() == RECENT_CAPACITY {
            recent.pop_front();
        }
        recent.push_back(event.clone());
    }

    /// The most recent `limit` events, oldest first.
    pub fn recent(&self, limit: usize) -> Vec<AuditEvent> {
        let recent = self.recent.lock();
        let skip = recent.len().saturating_sub(limit);
        recent.iter().skip(skip).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator_core::{CallOrigin, EventKind};
    use serde_json::json;

    fn event(n: usize) -> AuditEvent {
        AuditEvent::new(
            EventKind::ToolCalled,
            CallOrigin::User,
            format!("event {n}"),
            json!({"n": n}),
        )
    }

    #[test]
    fn appends_json_lines_and_keeps_recent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/audit.jsonl");
        let log = AuditLog::open(&path);
        for n in 0..3 {
            log.append(&event(n));
        }

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<AuditEvent> = content
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2].summary, "event 2");

        let recent = log.recent(2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].summary, "event 1");
        assert_eq!(recent[1].summary, "event 2");
    }

    #[test]
    fn recent_window_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::open(&dir.path().join("audit.jsonl"));
        for n in 0..RECENT_CAPACITY + 5 {
            log.append(&event(n));
        }
        let all = log.recent(usize::MAX);
        assert_eq!(all.len(), RECENT_CAPACITY);
        assert_eq!(all[0].summary, "event 5");
    }

    #[test]
    fn keeps_working_when_file_cannot_be_opened() {
        let dir = tempfile::tempdir().unwrap();
        // A directory in place of the file makes `open` fail.
        let path = dir.path().join("audit.jsonl");
        std::fs::create_dir(&path).unwrap();
        let log = AuditLog::open(&path);
        log.append(&event(1));
        assert_eq!(log.recent(10).len(), 1);
    }
}
