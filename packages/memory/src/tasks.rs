//! Tasks of a project (ADR-0014).
//!
//! The rows keep what the panel needs to order and filter; the rest of the
//! task travels as JSON. Dependencies live in `task_dependencies` and are
//! the truth: a task is always loaded with what those rows say, so the two
//! cannot drift.
//!
//! The rules (valid transitions, readiness, cycles) are the engine's.

use crate::search;
use crate::store::{ts, MemoryStore, Sql};
use orchestrator_core::{Task, TaskId, TaskPriority};
use rusqlite::{params, Connection, OptionalExtension};

fn priority_rank(priority: TaskPriority) -> i64 {
    match priority {
        TaskPriority::Low => 0,
        TaskPriority::Normal => 1,
        TaskPriority::High => 2,
        TaskPriority::Urgent => 3,
    }
}

/// Panel and board order: what is moving first, then priority, then age.
fn order_key(task: &Task) -> (u8, i64, String) {
    (
        task.status.rank(),
        -priority_rank(task.priority),
        ts(&task.created_at),
    )
}

fn dependencies_of(conn: &Connection, id: &str) -> Sql<Vec<TaskId>> {
    let mut stmt = conn.prepare(
        "SELECT depends_on FROM task_dependencies WHERE task_id = ?1 ORDER BY depends_on",
    )?;
    let rows = stmt.query_map([id], |r| r.get::<_, String>(0))?;
    rows.map(|id| id.map(TaskId::from)).collect()
}

fn from_row(conn: &Connection, data: &str) -> Option<Task> {
    let mut task: Task = serde_json::from_str(data).ok()?;
    task.dependencies = dependencies_of(conn, task.id.as_str()).unwrap_or_default();
    Some(task)
}

fn save(conn: &Connection, task: &Task) -> Sql<()> {
    let data = serde_json::to_string(task).expect("a task serializes");
    conn.execute(
        "INSERT INTO tasks (id, project_id, parent_task, status, priority, created_at, updated_at, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
             parent_task = excluded.parent_task, status = excluded.status,
             priority = excluded.priority, updated_at = excluded.updated_at,
             data = excluded.data",
        params![
            task.id.as_str(),
            task.project_id,
            task.parent_task.as_ref().map(|p| p.as_str().to_owned()),
            task.status.as_str(),
            priority_rank(task.priority),
            ts(&task.created_at),
            ts(&task.updated_at),
            data,
        ],
    )?;
    conn.execute(
        "DELETE FROM task_dependencies WHERE task_id = ?1",
        [task.id.as_str()],
    )?;
    for depends_on in &task.dependencies {
        conn.execute(
            "INSERT OR IGNORE INTO task_dependencies (task_id, depends_on) VALUES (?1, ?2)",
            params![task.id.as_str(), depends_on.as_str()],
        )?;
    }
    search::index_task(conn, task)
}

impl MemoryStore {
    /// Records a task (new or changed) and indexes it for L3.
    pub fn task_save(&self, task: &Task) -> Result<(), String> {
        let conn = self.db.conn.lock();
        save(&conn, task).map_err(|e| e.to_string())
    }

    pub fn task(&self, id: &str) -> Option<Task> {
        let conn = self.db.conn.lock();
        conn.query_row("SELECT data FROM tasks WHERE id = ?1", [id], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .ok()
        .flatten()
        .and_then(|data| from_row(&conn, &data))
    }

    /// Tasks of a project (all projects with `None`), in panel order.
    pub fn tasks_list(&self, project_id: Option<&str>) -> Vec<Task> {
        let conn = self.db.conn.lock();
        let rows = conn
            .prepare("SELECT data FROM tasks WHERE (?1 IS NULL OR project_id = ?1)")
            .and_then(|mut stmt| {
                stmt.query_map([project_id], |r| r.get::<_, String>(0))?
                    .collect::<Sql<Vec<String>>>()
            })
            .unwrap_or_default();
        let mut tasks: Vec<Task> = rows
            .iter()
            .filter_map(|data| from_row(&conn, data))
            .collect();
        tasks.sort_by_key(order_key);
        tasks
    }

    /// Tasks that depend on this one (used to warn before cancelling).
    pub fn task_dependents(&self, id: &str) -> Vec<TaskId> {
        let conn = self.db.conn.lock();
        conn.prepare("SELECT task_id FROM task_dependencies WHERE depends_on = ?1")
            .and_then(|mut stmt| {
                stmt.query_map([id], |r| r.get::<_, String>(0))?
                    .collect::<Sql<Vec<String>>>()
            })
            .unwrap_or_default()
            .into_iter()
            .map(TaskId::from)
            .collect()
    }

    /// The task a session was opened for, when it was opened from one.
    pub fn task_of_session(&self, session_id: &str) -> Option<Task> {
        let conn = self.db.conn.lock();
        let data = conn
            .query_row(
                "SELECT data FROM tasks WHERE EXISTS (
                     SELECT 1 FROM json_each(json_extract(data, '$.sessions'))
                     WHERE json_each.value = ?1
                 ) ORDER BY updated_at DESC LIMIT 1",
                [session_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()?;
        from_row(&conn, &data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator_core::TaskStatus;

    fn task(status: TaskStatus, priority: TaskPriority) -> Task {
        Task {
            id: TaskId::new(),
            project_id: "p".into(),
            title: "t".into(),
            description: String::new(),
            status,
            priority,
            provider: None,
            model: None,
            agent: None,
            parent_task: None,
            dependencies: Vec::new(),
            files: Vec::new(),
            sessions: Vec::new(),
            result: String::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn panel_order_is_status_then_priority_then_age() {
        let todo = task(TaskStatus::Todo, TaskPriority::Normal);
        let urgent = task(TaskStatus::Todo, TaskPriority::Urgent);
        let running = task(TaskStatus::InProgress, TaskPriority::Low);
        let done = task(TaskStatus::Done, TaskPriority::Urgent);
        let mut all = [done, todo, urgent, running];
        all.sort_by_key(order_key);
        let order: Vec<_> = all.iter().map(|t| (t.status, t.priority)).collect();
        assert_eq!(
            order,
            [
                (TaskStatus::InProgress, TaskPriority::Low),
                (TaskStatus::Todo, TaskPriority::Urgent),
                (TaskStatus::Todo, TaskPriority::Normal),
                (TaskStatus::Done, TaskPriority::Urgent),
            ]
        );
    }
}
