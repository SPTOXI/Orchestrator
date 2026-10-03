//! Projects open side by side in the app, and the projects that work
//! together (ADR-0023).
//!
//! Opening a project (`PROJECT_OPENED`) puts it at the end of the open
//! list; the user closes it from the sidebar. A link between two projects
//! is kept once per pair and read from either side, with the user's note
//! of how they relate: it is what lets the AI of one consult the other's.

use crate::model::{Project, ProjectLink};
use crate::store::{parse_ts, project_from_row, ts, MemoryStore, Sql};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

/// Longest note kept on a link.
const NOTE_MAX: usize = 500;

/// Puts the project at the end of the open list, unless it is there.
pub(crate) fn mark_open(conn: &Connection, id: &str) -> Sql<()> {
    conn.execute(
        "UPDATE projects
         SET open_rank = (SELECT COALESCE(MAX(open_rank), 0) + 1 FROM projects)
         WHERE id = ?1 AND open_rank IS NULL",
        [id],
    )?;
    Ok(())
}

/// The pair as stored: the smaller id first.
fn pair<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

impl MemoryStore {
    /// Projects open in the app, in their sidebar order.
    pub fn projects_open(&self) -> Vec<Project> {
        let conn = self.db.conn.lock();
        let rows = conn
            .prepare("SELECT * FROM projects WHERE open_rank IS NOT NULL ORDER BY open_rank")
            .and_then(|mut stmt| {
                stmt.query_map([], project_from_row)?
                    .collect::<Sql<Vec<_>>>()
            });
        rows.unwrap_or_else(|err| {
            eprintln!("[orchestrator] cannot list the open projects: {err}");
            Vec::new()
        })
    }

    /// Closes a project in the app: it leaves the sidebar, and everything
    /// it has stays. The open project of the app becomes the next one in
    /// the list (or none), which is returned.
    pub fn project_close(&self, id: &str) -> Result<Option<Project>, String> {
        let conn = self.db.conn.lock();
        conn.execute("UPDATE projects SET open_rank = NULL WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        drop(conn);
        let mut current = self.current.lock();
        if current.as_deref() == Some(id) {
            *current = None;
        }
        drop(current);
        Ok(self.projects_open().into_iter().next())
    }

    /// Puts the open projects in this order (ids not open are ignored, open
    /// ones left out keep their place after these).
    pub fn projects_reorder(&self, ids: &[String]) -> Result<(), String> {
        let mut conn = self.db.conn.lock();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let open: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT id FROM projects WHERE open_rank IS NOT NULL ORDER BY open_rank")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.collect::<Sql<Vec<_>>>().map_err(|e| e.to_string())?
        };
        let mut order: Vec<&String> = ids.iter().filter(|id| open.contains(id)).collect();
        order.dedup();
        for id in &open {
            if !order.contains(&id) {
                order.push(id);
            }
        }
        for (rank, id) in order.iter().enumerate() {
            tx.execute(
                "UPDATE projects SET open_rank = ?2 WHERE id = ?1",
                params![id, rank as i64 + 1],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// Links two projects (or changes the note of their link).
    pub fn project_link(&self, a: &str, b: &str, note: &str) -> Result<ProjectLink, String> {
        if a == b {
            return Err("um projeto não se relaciona com ele mesmo".to_owned());
        }
        let note: String = note.trim().chars().take(NOTE_MAX).collect();
        let conn = self.db.conn.lock();
        for id in [a, b] {
            let known: Option<String> = conn
                .query_row("SELECT id FROM projects WHERE id = ?1", [id], |r| r.get(0))
                .optional()
                .map_err(|e| e.to_string())?;
            if known.is_none() {
                return Err(format!("projeto desconhecido: {id}"));
            }
        }
        let (first, second) = pair(a, b);
        conn.execute(
            "INSERT INTO project_links (a, b, note, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(a, b) DO UPDATE SET note = excluded.note",
            params![first, second, note, ts(&Utc::now())],
        )
        .map_err(|e| e.to_string())?;
        drop(conn);
        self.project_links(a)
            .into_iter()
            .find(|link| link.project.id == b)
            .ok_or_else(|| "a relação não foi gravada".to_owned())
    }

    /// Removes the link between two projects; their data stays.
    pub fn project_unlink(&self, a: &str, b: &str) -> Result<bool, String> {
        let (first, second) = pair(a, b);
        let conn = self.db.conn.lock();
        conn.execute(
            "DELETE FROM project_links WHERE a = ?1 AND b = ?2",
            params![first, second],
        )
        .map(|n| n > 0)
        .map_err(|e| e.to_string())
    }

    /// The projects that work with `id`, by name.
    pub fn project_links(&self, id: &str) -> Vec<ProjectLink> {
        let conn = self.db.conn.lock();
        let rows = conn
            .prepare(
                "SELECT p.*, l.note AS link_note, l.created_at AS link_at
                 FROM project_links l
                 JOIN projects p ON p.id = CASE WHEN l.a = ?1 THEN l.b ELSE l.a END
                 WHERE l.a = ?1 OR l.b = ?1
                 ORDER BY p.name COLLATE NOCASE",
            )
            .and_then(|mut stmt| {
                stmt.query_map([id], |row| {
                    Ok(ProjectLink {
                        project: project_from_row(row)?,
                        note: row.get("link_note")?,
                        created_at: parse_ts(&row.get::<_, String>("link_at")?),
                    })
                })?
                .collect::<Sql<Vec<_>>>()
            });
        rows.unwrap_or_else(|err| {
            eprintln!("[orchestrator] cannot list the related projects: {err}");
            Vec::new()
        })
    }
}
