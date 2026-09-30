//! File Lock Manager (ADR-0015, section 13 of the master document).
//!
//! Two agents must not change the same file at the same time. A lock is a
//! row keyed by `(project, path)`: taking one either writes or names who
//! already holds it, so there is no check-then-write race.
//!
//! Rules that do not change: only agents lock and only agents are locked
//! (the project is the user's), reading is never locked, and a denied lock
//! refuses the tool with a reason instead of waiting — no queue of I/O, no
//! deadlock.

use chrono::Utc;
use orchestrator_core::{Agent, FileLock};
use orchestrator_memory::MemoryStore;
use std::path::Path;
use std::sync::Arc;

/// Paths are stored relative to the project, with `/` as the separator, so
/// the same file is the same lock however a tool named it.
pub fn normalize(project_path: &Path, path: &str) -> String {
    let cleaned = path.replace('\\', "/");
    let cleaned = cleaned.trim();
    let root = project_path.to_string_lossy().replace('\\', "/");
    let root = root.trim_end_matches('/');
    let relative = match cleaned.strip_prefix(root) {
        Some(rest) => rest.trim_start_matches('/'),
        // A path outside the project keeps its shape: it is a different
        // file, so it must not collide with one inside.
        None if cleaned.starts_with('/') => cleaned,
        None => cleaned.trim_start_matches("./"),
    };
    relative.trim_end_matches('/').to_owned()
}

/// Locks held in a project, with the manager that hands them out.
pub struct LockManager {
    store: Arc<MemoryStore>,
}

impl LockManager {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }

    /// Locks held in a project (all of them with `None`), oldest first.
    pub fn list(&self, project_id: Option<&str>) -> Vec<FileLock> {
        self.store.locks_list(project_id)
    }

    /// Tries to take `paths` for `agent`. Returns the lock that stopped it,
    /// leaving everything it had taken so far in place: the caller either
    /// refuses one tool call (and the agent keeps working) or gives up the
    /// agent (and every lock falls with it).
    ///
    /// Paths the agent already holds cost nothing. The agent's row keeps
    /// the list of what it holds, for the panel.
    pub fn take(&self, agent: &Agent, project_path: &Path, paths: &[String]) -> Option<FileLock> {
        let mut taken: Vec<String> = Vec::new();
        let mut denied = None;
        for path in paths {
            let path = normalize(project_path, path);
            if path.is_empty() || agent.files.contains(&path) || taken.contains(&path) {
                continue;
            }
            let lock = FileLock {
                project_id: agent.project_id.clone(),
                path: path.clone(),
                agent_id: agent.id.clone(),
                agent_title: agent.title.clone(),
                task: agent.task.clone(),
                at: Utc::now(),
            };
            match self.store.lock_take(&lock) {
                Ok(None) => taken.push(path),
                Ok(Some(holder)) => {
                    denied = Some(holder);
                    break;
                }
                // The database refused: treat it as a denial rather than
                // letting two agents into the same file.
                Err(message) => {
                    denied = Some(FileLock {
                        agent_title: format!("indisponível ({message})"),
                        ..lock
                    });
                    break;
                }
            }
        }
        if !taken.is_empty() {
            let mut updated = agent.clone();
            updated.files.extend(taken);
            updated.updated_at = Utc::now();
            let _ = self.store.agent_save(&updated);
        }
        denied
    }

    /// The lock that would stop this agent from working on `paths`, if any
    /// — used by the queue before starting an agent.
    pub fn blocked_by(
        &self,
        project_id: &str,
        project_path: &Path,
        paths: &[String],
        agent: &Agent,
    ) -> Option<FileLock> {
        if paths.is_empty() {
            return None;
        }
        let wanted: Vec<String> = paths
            .iter()
            .map(|p| normalize(project_path, p))
            .filter(|p| !p.is_empty())
            .collect();
        self.store
            .locks_list(Some(project_id))
            .into_iter()
            .find(|lock| lock.agent_id != agent.id && wanted.contains(&lock.path))
    }

    /// Frees everything an agent holds. Called when it ends, however it
    /// ends.
    pub fn release(&self, agent: &Agent) {
        let _ = self.store.locks_release(agent.id.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_same_file_is_the_same_lock_however_a_tool_named_it() {
        let root = PathBuf::from("/home/ana/projeto");
        assert_eq!(normalize(&root, "src/api.ts"), "src/api.ts");
        assert_eq!(normalize(&root, "./src/api.ts"), "src/api.ts");
        assert_eq!(
            normalize(&root, "/home/ana/projeto/src/api.ts"),
            "src/api.ts"
        );
        assert_eq!(normalize(&root, "src\\api.ts"), "src/api.ts");
        assert_eq!(normalize(&root, "  src/api.ts  "), "src/api.ts");
        assert_eq!(normalize(&root, "src/"), "src");
        // A file outside the project is a different file: it must not
        // collide with `etc/hosts` inside it.
        assert_eq!(normalize(&root, "/etc/hosts"), "/etc/hosts");
        assert_ne!(
            normalize(&root, "/etc/hosts"),
            normalize(&root, "etc/hosts")
        );
    }
}
