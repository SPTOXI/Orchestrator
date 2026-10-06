//! L2 project memory entries and decisions (ADR-0012).

use crate::model::{
    Decision, DecisionInput, DecisionStatus, MemoryEntry, MemoryInput, MemoryKind, Project, Source,
};
use crate::search;
use crate::store::{new_id, parse_ts, ts, MemoryStore, Sql};
use chrono::Utc;
use orchestrator_core::{AuditEvent, CallOrigin, EventKind};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

const MAX_TITLE: usize = 200;
const MAX_TEXT: usize = 20_000;
const MAX_TAGS: usize = 20;

fn kind_label(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Architecture => "arquitetura",
        MemoryKind::Stack => "stack",
        MemoryKind::Convention => "convenção",
        MemoryKind::Rule => "regra",
        MemoryKind::Note => "nota",
    }
}

fn status_label(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Proposed => "proposta",
        DecisionStatus::Accepted => "aceita",
        DecisionStatus::Superseded => "substituída",
        DecisionStatus::Rejected => "rejeitada",
    }
}

fn entry_from_row(row: &rusqlite::Row<'_>) -> Sql<MemoryEntry> {
    let tags: String = row.get("tags")?;
    Ok(MemoryEntry {
        id: row.get("id")?,
        project_id: row.get("project_id")?,
        kind: MemoryKind::parse(&row.get::<_, String>("kind")?).unwrap_or(MemoryKind::Note),
        title: row.get("title")?,
        content: row.get("content")?,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        pinned: row.get::<_, i64>("pinned")? != 0,
        source: Source::parse(&row.get::<_, String>("source")?),
        created_at: parse_ts(&row.get::<_, String>("created_at")?),
        updated_at: parse_ts(&row.get::<_, String>("updated_at")?),
    })
}

fn decision_from_row(row: &rusqlite::Row<'_>) -> Sql<Decision> {
    Ok(Decision {
        id: row.get("id")?,
        project_id: row.get("project_id")?,
        title: row.get("title")?,
        context: row.get("context")?,
        decision: row.get("decision")?,
        consequences: row.get("consequences")?,
        status: DecisionStatus::parse(&row.get::<_, String>("status")?),
        source: Source::parse(&row.get::<_, String>("source")?),
        created_at: parse_ts(&row.get::<_, String>("created_at")?),
        updated_at: parse_ts(&row.get::<_, String>("updated_at")?),
    })
}

/// `MEMORY_SAVED` for `entry`.
pub(crate) fn memory_event(entry: &MemoryEntry, created: bool, origin: &CallOrigin) -> AuditEvent {
    AuditEvent::new(
        EventKind::MemorySaved,
        origin.clone(),
        format!("memória {} · {}", kind_label(entry.kind), entry.title),
        json!({
            "projectId": entry.project_id,
            "entryId": entry.id,
            "kind": entry.kind,
            "title": entry.title,
            "source": entry.source,
            "pinned": entry.pinned,
            "created": created,
            "chars": entry.content.chars().count(),
        }),
    )
}

fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if !tag.is_empty() && !out.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            out.push(tag.chars().take(40).collect());
        }
    }
    out.truncate(MAX_TAGS);
    out
}

fn check_text(field: &str, text: &str, max: usize, required: bool) -> Result<(), String> {
    if required && text.trim().is_empty() {
        return Err(format!("{field} é obrigatório"));
    }
    if text.chars().count() > max {
        return Err(format!("{field} passa de {max} caracteres"));
    }
    Ok(())
}

fn project_exists(conn: &Connection, id: &str) -> Result<(), String> {
    let found: Option<i64> = conn
        .query_row("SELECT 1 FROM projects WHERE id = ?1", [id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    found
        .map(|_| ())
        .ok_or_else(|| format!("projeto {id} não encontrado"))
}

/// Human-readable detected stack: one line per non-empty group.
pub(crate) fn stack_text(stack: &Value) -> String {
    let names =
        |key: &str| -> Vec<String> {
            stack
                .get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| match item {
                            Value::String(s) => Some(s.clone()),
                            Value::Object(o) => o.get("name").and_then(Value::as_str).map(|name| {
                                match o.get("version").and_then(Value::as_str) {
                                    Some(v) => format!("{name} {v}"),
                                    None => name.to_owned(),
                                }
                            }),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
    let mut lines = Vec::new();
    for (key, label) in [
        ("languages", "Linguagens"),
        ("frameworks", "Frameworks"),
        ("packageManagers", "Gerenciadores de pacote"),
        ("runtimes", "Runtimes"),
        ("databases", "Bancos de dados"),
        ("tools", "Ferramentas"),
    ] {
        let values = names(key);
        if !values.is_empty() {
            lines.push(format!("{label}: {}", values.join(", ")));
        }
    }
    if let Some(docker) = stack.get("docker") {
        let mut files = Vec::new();
        for key in ["dockerfiles", "composeFiles"] {
            if let Some(items) = docker.get(key).and_then(Value::as_array) {
                files.extend(items.iter().filter_map(Value::as_str).map(str::to_owned));
            }
        }
        if !files.is_empty() {
            lines.push(format!("Docker: {}", files.join(", ")));
        }
    }
    if stack.get("monorepo").and_then(Value::as_bool) == Some(true) {
        lines.push("Monorepo: sim".into());
    }
    lines.join("\n")
}

/// Creates or refreshes the detector's "Stack" entry. Returns it when it
/// changed. An entry taken over by the user (source `user`) is left alone.
pub(crate) fn upsert_stack(conn: &Connection, project: &Project) -> Sql<Option<MemoryEntry>> {
    let content = project.stack.as_ref().map(stack_text).unwrap_or_default();
    if content.is_empty() {
        return Ok(None);
    }
    let user_owned: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM memory_entries WHERE project_id = ?1 AND kind = 'stack' AND source = 'user' LIMIT 1",
            [&project.id],
            |r| r.get(0),
        )
        .optional()?;
    if user_owned.is_some() {
        return Ok(None);
    }
    let now = ts(&Utc::now());
    let existing: Option<(String, String)> = conn
        .query_row(
            "SELECT id, content FROM memory_entries WHERE project_id = ?1 AND kind = 'stack' AND source = 'detector'",
            [&project.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let id = match existing {
        Some((_, old)) if old == content => return Ok(None),
        Some((id, _)) => {
            conn.execute(
                "UPDATE memory_entries SET content = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, content, now],
            )?;
            id
        }
        None => {
            let id = new_id();
            conn.execute(
                "INSERT INTO memory_entries (id, project_id, kind, title, content, tags, pinned, source, created_at, updated_at)
                 VALUES (?1, ?2, 'stack', 'Stack detectada', ?3, '[\"detectado\"]', 1, 'detector', ?4, ?4)",
                params![id, project.id, content, now],
            )?;
            id
        }
    };
    let entry = conn.query_row(
        "SELECT * FROM memory_entries WHERE id = ?1",
        [&id],
        entry_from_row,
    )?;
    search::index_memory(conn, &entry)?;
    Ok(Some(entry))
}

impl MemoryStore {
    /// Memory entries of a project: pinned first, then by kind and update.
    pub fn memory_list(&self, project_id: &str) -> Result<Vec<MemoryEntry>, String> {
        let conn = self.db.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT * FROM memory_entries WHERE project_id = ?1
                 ORDER BY pinned DESC, kind, updated_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([project_id], entry_from_row)
            .map_err(|e| e.to_string())?;
        rows.collect::<Sql<Vec<_>>>().map_err(|e| e.to_string())
    }

    /// Creates or updates an entry. Returns it and the `MEMORY_SAVED` event
    /// for the caller to publish.
    pub fn memory_save(
        &self,
        input: MemoryInput,
        origin: &CallOrigin,
    ) -> Result<(MemoryEntry, AuditEvent), String> {
        check_text("título", &input.title, MAX_TITLE, true)?;
        check_text("conteúdo", &input.content, MAX_TEXT, false)?;
        let conn = self.db.conn.lock();
        project_exists(&conn, &input.project_id)?;
        let now = ts(&Utc::now());
        let tags = serde_json::to_string(&clean_tags(&input.tags)).unwrap_or_else(|_| "[]".into());
        let source = Source::of(origin).id();
        let created = match &input.id {
            Some(id) => {
                let changed = conn
                    .execute(
                        "UPDATE memory_entries SET kind = ?3, title = ?4, content = ?5, tags = ?6,
                                pinned = ?7, source = ?8, updated_at = ?9
                         WHERE id = ?1 AND project_id = ?2",
                        params![
                            id,
                            input.project_id,
                            input.kind.id(),
                            input.title.trim(),
                            input.content,
                            tags,
                            input.pinned as i64,
                            source,
                            now
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                if changed == 0 {
                    return Err(format!("entrada de memória {id} não encontrada"));
                }
                false
            }
            None => true,
        };
        let id = input.id.clone().unwrap_or_else(new_id);
        if created {
            conn.execute(
                "INSERT INTO memory_entries (id, project_id, kind, title, content, tags, pinned, source, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                params![
                    id,
                    input.project_id,
                    input.kind.id(),
                    input.title.trim(),
                    input.content,
                    tags,
                    input.pinned as i64,
                    source,
                    now
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        let entry = conn
            .query_row(
                "SELECT * FROM memory_entries WHERE id = ?1",
                [&id],
                entry_from_row,
            )
            .map_err(|e| e.to_string())?;
        search::index_memory(&conn, &entry).map_err(|e| e.to_string())?;
        let event = memory_event(&entry, created, origin);
        Ok((entry, event))
    }

    /// Deletes an entry. Returns the `MEMORY_REMOVED` event.
    pub fn memory_delete(&self, id: &str, origin: &CallOrigin) -> Result<AuditEvent, String> {
        let conn = self.db.conn.lock();
        let entry = conn
            .query_row(
                "SELECT * FROM memory_entries WHERE id = ?1",
                [id],
                entry_from_row,
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("entrada de memória {id} não encontrada"))?;
        conn.execute("DELETE FROM memory_entries WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        search::unindex(&conn, "memory", id).map_err(|e| e.to_string())?;
        Ok(AuditEvent::new(
            EventKind::MemoryRemoved,
            origin.clone(),
            format!("memória removida · {}", entry.title),
            json!({
                "projectId": entry.project_id,
                "entryId": entry.id,
                "kind": entry.kind,
                "title": entry.title,
            }),
        ))
    }

    /// Decisions of a project, newest first.
    pub fn decisions_list(&self, project_id: &str) -> Result<Vec<Decision>, String> {
        let conn = self.db.conn.lock();
        let mut stmt = conn
            .prepare("SELECT * FROM decisions WHERE project_id = ?1 ORDER BY created_at DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([project_id], decision_from_row)
            .map_err(|e| e.to_string())?;
        rows.collect::<Sql<Vec<_>>>().map_err(|e| e.to_string())
    }

    /// Records or updates a decision. Returns it and `DECISION_SAVED`.
    pub fn decision_save(
        &self,
        input: DecisionInput,
        origin: &CallOrigin,
    ) -> Result<(Decision, AuditEvent), String> {
        check_text("título", &input.title, MAX_TITLE, true)?;
        check_text("decisão", &input.decision, MAX_TEXT, true)?;
        check_text("contexto", &input.context, MAX_TEXT, false)?;
        check_text("consequências", &input.consequences, MAX_TEXT, false)?;
        let conn = self.db.conn.lock();
        project_exists(&conn, &input.project_id)?;
        let now = ts(&Utc::now());
        let previous = match &input.id {
            Some(id) => {
                let old: Option<String> = conn
                    .query_row(
                        "SELECT status FROM decisions WHERE id = ?1 AND project_id = ?2",
                        params![id, input.project_id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?;
                Some(
                    old.map(|s| DecisionStatus::parse(&s))
                        .ok_or_else(|| format!("decisão {id} não encontrada"))?,
                )
            }
            None => None,
        };
        let id = input.id.clone().unwrap_or_else(new_id);
        match previous {
            Some(_) => conn.execute(
                "UPDATE decisions SET title = ?2, context = ?3, decision = ?4, consequences = ?5,
                        status = ?6, updated_at = ?7 WHERE id = ?1",
                params![
                    id,
                    input.title.trim(),
                    input.context,
                    input.decision,
                    input.consequences,
                    input.status.id(),
                    now
                ],
            ),
            None => conn.execute(
                "INSERT INTO decisions (id, project_id, title, context, decision, consequences, status, source, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                params![
                    id,
                    input.project_id,
                    input.title.trim(),
                    input.context,
                    input.decision,
                    input.consequences,
                    input.status.id(),
                    Source::of(origin).id(),
                    now
                ],
            ),
        }
        .map_err(|e| e.to_string())?;
        let decision = conn
            .query_row(
                "SELECT * FROM decisions WHERE id = ?1",
                [&id],
                decision_from_row,
            )
            .map_err(|e| e.to_string())?;
        search::index_decision(&conn, &decision).map_err(|e| e.to_string())?;
        let summary = match previous {
            Some(old) if old != decision.status => format!(
                "decisão {} → {} · {}",
                status_label(old),
                status_label(decision.status),
                decision.title
            ),
            Some(_) => format!("decisão atualizada · {}", decision.title),
            None => format!(
                "decisão registrada ({}) · {}",
                status_label(decision.status),
                decision.title
            ),
        };
        let event = AuditEvent::new(
            EventKind::DecisionSaved,
            origin.clone(),
            summary,
            json!({
                "projectId": decision.project_id,
                "decisionId": decision.id,
                "title": decision.title,
                "status": decision.status,
                "previousStatus": previous,
                "source": decision.source,
            }),
        );
        Ok((decision, event))
    }
}
