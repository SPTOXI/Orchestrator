//! Agents of a project and the files they hold (ADR-0015).
//!
//! The rows keep what the panel and the board need to list and filter; the
//! rest of the agent travels as JSON. A lock is a row whose key is
//! `(project_id, path)`: taking one either writes or names who already has
//! it, so two agents cannot hold the same file.
//!
//! The rules (queue, turns, delegation) are the agent manager's.

use crate::store::{ts, MemoryStore, Sql};
use chrono::Utc;
use orchestrator_core::{Agent, AgentStatus, FileLock};
use rusqlite::{params, Connection, OptionalExtension};

fn from_row(data: &str) -> Option<Agent> {
    serde_json::from_str(data).ok()
}

fn save(conn: &Connection, agent: &Agent) -> Sql<()> {
    let data = serde_json::to_string(agent).expect("an agent serializes");
    conn.execute(
        "INSERT INTO agents (id, project_id, task, session, parent_agent, status, created_at, updated_at, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
             session = excluded.session, status = excluded.status,
             updated_at = excluded.updated_at, data = excluded.data",
        params![
            agent.id.as_str(),
            agent.project_id,
            agent.task.as_str(),
            agent.session.as_ref().map(|s| s.as_str().to_owned()),
            agent.parent_agent.as_ref().map(|p| p.as_str().to_owned()),
            agent.status.as_str(),
            ts(&agent.created_at),
            ts(&agent.updated_at),
            data,
        ],
    )?;
    Ok(())
}

/// Board and panel order: what is working first, then the queue, then
/// what ended, newest first inside each group.
fn order_key(agent: &Agent) -> (u8, std::cmp::Reverse<String>) {
    (
        agent.status.rank(),
        std::cmp::Reverse(ts(&agent.created_at)),
    )
}

impl MemoryStore {
    /// Records an agent (new or changed).
    pub fn agent_save(&self, agent: &Agent) -> Result<(), String> {
        let conn = self.db.conn.lock();
        save(&conn, agent).map_err(|e| e.to_string())
    }

    pub fn agent(&self, id: &str) -> Option<Agent> {
        let conn = self.db.conn.lock();
        conn.query_row("SELECT data FROM agents WHERE id = ?1", [id], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .ok()
        .flatten()
        .as_deref()
        .and_then(from_row)
    }

    /// Agents of a project (all projects with `None`), in board order.
    pub fn agents_list(&self, project_id: Option<&str>) -> Vec<Agent> {
        let conn = self.db.conn.lock();
        let rows = conn
            .prepare("SELECT data FROM agents WHERE (?1 IS NULL OR project_id = ?1)")
            .and_then(|mut stmt| {
                stmt.query_map([project_id], |r| r.get::<_, String>(0))?
                    .collect::<Sql<Vec<String>>>()
            })
            .unwrap_or_default();
        let mut agents: Vec<Agent> = rows
            .iter()
            .map(String::as_str)
            .filter_map(from_row)
            .collect();
        agents.sort_by_key(order_key);
        agents
    }

    /// Agents that are queued or running, oldest first: the manager's queue.
    pub fn agents_live(&self) -> Vec<Agent> {
        let conn = self.db.conn.lock();
        let rows = conn
            .prepare(
                "SELECT data FROM agents WHERE status IN ('QUEUED', 'RUNNING')
                 ORDER BY created_at, id",
            )
            .and_then(|mut stmt| {
                stmt.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Sql<Vec<String>>>()
            })
            .unwrap_or_default();
        rows.iter()
            .map(String::as_str)
            .filter_map(from_row)
            .collect()
    }

    /// The agent working in a session, if any.
    pub fn agent_of_session(&self, session_id: &str) -> Option<Agent> {
        let conn = self.db.conn.lock();
        conn.query_row(
            "SELECT data FROM agents WHERE session = ?1 ORDER BY created_at DESC LIMIT 1",
            [session_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .as_deref()
        .and_then(from_row)
    }

    /// Agents that worked on a task, oldest first.
    pub fn agents_of_task(&self, task_id: &str) -> Vec<Agent> {
        let conn = self.db.conn.lock();
        let rows = conn
            .prepare("SELECT data FROM agents WHERE task = ?1 ORDER BY created_at, id")
            .and_then(|mut stmt| {
                stmt.query_map([task_id], |r| r.get::<_, String>(0))?
                    .collect::<Sql<Vec<String>>>()
            })
            .unwrap_or_default();
        rows.iter()
            .map(String::as_str)
            .filter_map(from_row)
            .collect()
    }

    /// Takes the lock of `path` for an agent. `Ok(None)` means it was
    /// taken (or the agent already had it); `Ok(Some(lock))` names who
    /// holds it. One statement, so two agents racing cannot both win.
    pub fn lock_take(&self, lock: &FileLock) -> Result<Option<FileLock>, String> {
        let conn = self.db.conn.lock();
        let data = serde_json::to_string(lock).expect("a lock serializes");
        let taken = conn
            .execute(
                "INSERT OR IGNORE INTO file_locks (project_id, path, agent_id, task, at, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    lock.project_id,
                    lock.path,
                    lock.agent_id.as_str(),
                    lock.task.as_str(),
                    ts(&lock.at),
                    data,
                ],
            )
            .map_err(|e| e.to_string())?;
        if taken == 1 {
            return Ok(None);
        }
        let holder: Option<String> = conn
            .query_row(
                "SELECT data FROM file_locks WHERE project_id = ?1 AND path = ?2",
                params![lock.project_id, lock.path],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let holder: Option<FileLock> = holder.and_then(|data| serde_json::from_str(&data).ok());
        match holder {
            // The agent already holds it: nothing to do.
            Some(held) if held.agent_id == lock.agent_id => Ok(None),
            other => Ok(other),
        }
    }

    /// Frees everything an agent holds (it ended, one way or another).
    pub fn locks_release(&self, agent_id: &str) -> Result<usize, String> {
        let conn = self.db.conn.lock();
        conn.execute("DELETE FROM file_locks WHERE agent_id = ?1", [agent_id])
            .map_err(|e| e.to_string())
    }

    /// Locks held in a project (all projects with `None`), oldest first.
    pub fn locks_list(&self, project_id: Option<&str>) -> Vec<FileLock> {
        let conn = self.db.conn.lock();
        let rows = conn
            .prepare(
                "SELECT data FROM file_locks WHERE (?1 IS NULL OR project_id = ?1)
                 ORDER BY at, path",
            )
            .and_then(|mut stmt| {
                stmt.query_map([project_id], |r| r.get::<_, String>(0))?
                    .collect::<Sql<Vec<String>>>()
            })
            .unwrap_or_default();
        rows.iter()
            .filter_map(|data| serde_json::from_str(data).ok())
            .collect()
    }

    /// On startup: no agent is running, so nothing may stay locked. Agents
    /// left mid-flight are closed with `why`, and their locks fall. Returns
    /// how many agents were closed.
    pub fn agents_recover(&self, why: &str) -> Result<usize, String> {
        let live = self.agents_live();
        let closed = live.len();
        let now = Utc::now();
        for mut agent in live {
            agent.status = AgentStatus::Failed;
            agent.error = Some(why.to_owned());
            agent.updated_at = now;
            agent.finished_at = Some(now);
            self.agent_save(&agent)?;
        }
        self.db
            .conn
            .lock()
            .execute("DELETE FROM file_locks", [])
            .map_err(|e| e.to_string())?;
        Ok(closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator_core::{AgentId, ProviderId, SessionId, TaskId};

    fn store() -> MemoryStore {
        let store = MemoryStore::in_memory();
        store
            .db
            .conn
            .lock()
            .execute_batch(
                "INSERT INTO projects (id, path, name, created_at, last_opened_at)
                     VALUES ('p', '/x', 'x', 'a', 'a');
                 INSERT INTO tasks (id, project_id, status, priority, created_at, updated_at, data)
                     VALUES ('t', 'p', 'TODO', 1, 'a', 'a', '{}');",
            )
            .unwrap();
        store
    }

    fn agent(status: AgentStatus) -> Agent {
        Agent {
            id: AgentId::new(),
            project_id: "p".into(),
            task: TaskId::from("t"),
            title: "Trabalho".into(),
            provider: ProviderId::from("eco"),
            model: None,
            session: None,
            parent_agent: None,
            status,
            tools: Vec::new(),
            context: None,
            turns: 0,
            max_turns: 12,
            files: Vec::new(),
            result: String::new(),
            error: None,
            handoff: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            started_at: None,
            finished_at: None,
        }
    }

    fn lock(agent: &Agent, path: &str) -> FileLock {
        FileLock {
            project_id: agent.project_id.clone(),
            path: path.into(),
            agent_id: agent.id.clone(),
            agent_title: agent.title.clone(),
            task: agent.task.clone(),
            at: Utc::now(),
        }
    }

    #[test]
    fn agents_are_listed_by_state_and_found_by_session_and_task() {
        let store = store();
        let mut running = agent(AgentStatus::Running);
        running.session = Some(SessionId::from("s1"));
        let queued = agent(AgentStatus::Queued);
        let mut done = agent(AgentStatus::Done);
        done.result = "pronto".into();
        for a in [&running, &queued, &done] {
            store.agent_save(a).unwrap();
        }
        let ids: Vec<_> = store
            .agents_list(Some("p"))
            .into_iter()
            .map(|a| a.status)
            .collect();
        assert_eq!(
            ids,
            [AgentStatus::Running, AgentStatus::Queued, AgentStatus::Done]
        );
        // The queue only has what still holds a slot.
        assert_eq!(store.agents_live().len(), 2);
        assert_eq!(
            store.agent_of_session("s1").map(|a| a.id),
            Some(running.id.clone())
        );
        assert_eq!(store.agents_of_task("t").len(), 3);
        assert_eq!(store.agent(done.id.as_str()).unwrap().result, "pronto");
    }

    #[test]
    fn a_file_has_one_owner_and_the_loser_learns_who() {
        let store = store();
        let first = agent(AgentStatus::Running);
        let second = agent(AgentStatus::Running);
        store.agent_save(&first).unwrap();
        store.agent_save(&second).unwrap();

        assert_eq!(store.lock_take(&lock(&first, "src/a.ts")).unwrap(), None);
        // Taking it again is not an error for the agent that has it.
        assert_eq!(store.lock_take(&lock(&first, "src/a.ts")).unwrap(), None);
        let denied = store.lock_take(&lock(&second, "src/a.ts")).unwrap();
        assert_eq!(denied.map(|l| l.agent_id), Some(first.id.clone()));
        // Another file is free.
        assert_eq!(store.lock_take(&lock(&second, "src/b.ts")).unwrap(), None);
        assert_eq!(store.locks_list(Some("p")).len(), 2);

        store.locks_release(first.id.as_str()).unwrap();
        assert_eq!(store.locks_list(Some("p")).len(), 1);
        // With the first agent gone, the file is free for the second.
        assert_eq!(store.lock_take(&lock(&second, "src/a.ts")).unwrap(), None);
    }

    #[test]
    fn the_app_closing_mid_flight_does_not_leave_the_project_locked() {
        let store = store();
        let running = agent(AgentStatus::Running);
        store.agent_save(&running).unwrap();
        store.lock_take(&lock(&running, "src/a.ts")).unwrap();

        let closed = store.agents_recover("o app foi encerrado").unwrap();
        assert_eq!(closed, 1);
        assert!(store.locks_list(None).is_empty());
        let agent = store.agent(running.id.as_str()).unwrap();
        assert_eq!(agent.status, AgentStatus::Failed);
        assert_eq!(agent.error.as_deref(), Some("o app foi encerrado"));
        assert!(store.agents_live().is_empty());
    }
}
