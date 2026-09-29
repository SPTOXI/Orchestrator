//! L3 search: full-text index (FTS5, accents and case ignored) over memory
//! entries, decisions, session messages and notable events (ADR-0012).

use crate::model::{Decision, MemoryEntry, SearchHit};
use crate::store::{parse_ts, ts, MemoryStore, Sql};
use chrono::{DateTime, Utc};
use orchestrator_core::{AuditEvent, EventKind, Handoff};
use rusqlite::{params, Connection};
use serde_json::Value;

const MAX_BODY: usize = 8_000;

fn put(
    conn: &Connection,
    kind: &str,
    ref_id: &str,
    project_id: Option<&str>,
    at: &DateTime<Utc>,
    title: &str,
    body: &str,
) -> Sql<()> {
    unindex(conn, kind, ref_id)?;
    let body: String = body.chars().take(MAX_BODY).collect();
    conn.execute(
        "INSERT INTO search_index (kind, ref_id, project_id, at, title, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![kind, ref_id, project_id, ts(at), title, body],
    )?;
    Ok(())
}

pub(crate) fn unindex(conn: &Connection, kind: &str, ref_id: &str) -> Sql<()> {
    conn.execute(
        "DELETE FROM search_index WHERE kind = ?1 AND ref_id = ?2",
        params![kind, ref_id],
    )?;
    Ok(())
}

pub(crate) fn index_memory(conn: &Connection, entry: &MemoryEntry) -> Sql<()> {
    let body = format!("{}\n{}", entry.content, entry.tags.join(" "));
    put(
        conn,
        "memory",
        &entry.id,
        Some(&entry.project_id),
        &entry.updated_at,
        &entry.title,
        &body,
    )
}

pub(crate) fn index_decision(conn: &Connection, decision: &Decision) -> Sql<()> {
    let body = format!(
        "{}\n{}\n{}",
        decision.context, decision.decision, decision.consequences
    );
    put(
        conn,
        "decision",
        &decision.id,
        Some(&decision.project_id),
        &decision.updated_at,
        &decision.title,
        &body,
    )
}

/// A message of a session (`ref_id` = `<session>:<seq>`).
pub(crate) fn index_message(
    conn: &Connection,
    session_id: &str,
    seq: u64,
    project_id: Option<&str>,
    at: &DateTime<Utc>,
    title: &str,
    text: &str,
) -> Sql<()> {
    put(
        conn,
        "message",
        &format!("{session_id}:{seq}"),
        project_id,
        at,
        title,
        text,
    )
}

/// Events worth finding later: commits, commands and failures.
pub(crate) fn index_event(
    conn: &Connection,
    event: &AuditEvent,
    project_id: Option<&str>,
) -> Sql<()> {
    let data = &event.data;
    let detail = match event.kind {
        EventKind::GitCommit | EventKind::GitPush | EventKind::CommandExecuted => {
            Some(String::new())
        }
        EventKind::ToolCalled if data.get("ok") == Some(&Value::Bool(false)) => Some(
            data.pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        EventKind::TurnCompleted
            if data.get("status").and_then(Value::as_str) == Some("failed") =>
        {
            Some(
                data.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            )
        }
        _ => None,
    };
    match detail {
        Some(detail) => put(
            conn,
            "event",
            event.id.as_str(),
            project_id,
            &event.at,
            &event.summary,
            &detail,
        ),
        None => Ok(()),
    }
}

/// A handoff between AIs (ADR-0013): goal, status and what remains.
pub(crate) fn index_handoff(conn: &Connection, handoff: &Handoff) -> Sql<()> {
    let packet = &handoff.packet;
    let body = format!(
        "{}\n{}\n{}\n{}",
        packet.status,
        packet.next_action,
        packet.remaining.join("\n"),
        packet.completed.join("\n")
    );
    put(
        conn,
        "handoff",
        handoff.id.as_str(),
        handoff.project_id.as_deref(),
        &handoff.created_at,
        &packet.goal,
        &body,
    )
}

/// Words that say nothing about a task, in Portuguese and English.
const STOPWORDS: &[&str] = &[
    "a", "ao", "aos", "as", "com", "como", "da", "das", "de", "do", "dos", "e", "ela", "ele", "em",
    "entre", "era", "essa", "esse", "esta", "este", "eu", "foi", "isso", "isto", "ja", "mais",
    "mas", "me", "meu", "minha", "na", "nas", "nao", "no", "nos", "num", "numa", "o", "os", "ou",
    "para", "pela", "pelo", "por", "pra", "qual", "quando", "que", "se", "sem", "ser", "seu",
    "sua", "tem", "um", "uma", "voce", "the", "and", "for", "with", "from", "that", "this", "into",
    "then", "than", "are", "was", "were", "you", "your", "our", "can", "please", "fix", "faca",
    "faz", "fazer", "vamos", "agora", "aqui",
];

/// Folds accents so stop words match ("não" → "nao").
fn fold(word: &str) -> String {
    word.chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

/// `"webhook"* OR "stripe"*`-style query for relevance (ADR-0013): any
/// significant word counts, ranking does the rest. Short words and stop
/// words are dropped; at most `MAX_TERMS` words.
pub(crate) fn fts_any_query(text: &str) -> Option<String> {
    const MAX_TERMS: usize = 12;
    let mut terms: Vec<String> = Vec::new();
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        let word = word.to_lowercase();
        if word.chars().count() < 3 || STOPWORDS.contains(&fold(&word).as_str()) {
            continue;
        }
        if word.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let term = format!("\"{word}\"*");
        if !terms.contains(&term) {
            terms.push(term);
        }
        if terms.len() == MAX_TERMS {
            break;
        }
    }
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

/// `"filas" "distrib"*`-style query: every word must appear, the last
/// one may be a prefix. `None` when nothing searchable is left.
pub(crate) fn fts_query(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"*", w.to_lowercase()))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

fn hit_from_row(row: &rusqlite::Row<'_>) -> Sql<SearchHit> {
    let kind: String = row.get(0)?;
    let ref_id: String = row.get(1)?;
    let snippet: String = row.get(3)?;
    let title: String = row.get(2)?;
    Ok(SearchHit {
        ref_id: match kind.as_str() {
            "message" => ref_id.split(':').next().unwrap_or_default().to_owned(),
            _ => ref_id,
        },
        snippet: if snippet.trim().is_empty() {
            title.clone()
        } else {
            snippet
        },
        title,
        kind,
        at: parse_ts(&row.get::<_, String>(4)?),
    })
}

impl MemoryStore {
    /// Entries related to a task (ADR-0013): any significant word of `text`
    /// matches, best first, only the given kinds (`memory`, `decision`,
    /// `message`, `event`, `handoff`; empty = all).
    pub fn search_related(
        &self,
        project_id: &str,
        text: &str,
        kinds: &[&str],
        limit: usize,
    ) -> Result<Vec<SearchHit>, String> {
        let Some(query) = fts_any_query(text) else {
            return Ok(Vec::new());
        };
        let kinds_sql = if kinds.is_empty() {
            String::new()
        } else {
            let list: Vec<String> = kinds
                .iter()
                .filter(|k| k.chars().all(|c| c.is_ascii_lowercase()))
                .map(|k| format!("'{k}'"))
                .collect();
            format!(" AND kind IN ({})", list.join(", "))
        };
        let conn = self.db.conn.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT kind, ref_id, title, snippet(search_index, 5, '[', ']', '…', 16) AS snip, at
                 FROM search_index
                 WHERE search_index MATCH ?1 AND project_id = ?2{kinds_sql}
                 ORDER BY rank LIMIT ?3"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                params![query, project_id, limit.clamp(1, 200) as i64],
                hit_from_row,
            )
            .map_err(|e| e.to_string())?;
        rows.collect::<Sql<Vec<_>>>().map_err(|e| e.to_string())
    }

    /// Searches a project's memory, decisions, session messages and notable
    /// events (L3), best matches first.
    pub fn search(
        &self,
        project_id: &str,
        text: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>, String> {
        let Some(query) = fts_query(text) else {
            return Ok(Vec::new());
        };
        let conn = self.db.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT kind, ref_id, title, snippet(search_index, 5, '[', ']', '…', 16) AS snip, at
                 FROM search_index
                 WHERE search_index MATCH ?1 AND project_id = ?2
                 ORDER BY rank LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                params![query, project_id, limit.clamp(1, 200) as i64],
                hit_from_row,
            )
            .map_err(|e| e.to_string())?;
        rows.collect::<Sql<Vec<_>>>().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{fts_any_query, fts_query};

    #[test]
    fn related_queries_keep_significant_words() {
        assert_eq!(
            fts_any_query("Corrija a validação da assinatura do webhook do Stripe").unwrap(),
            "\"corrija\"* OR \"validação\"* OR \"assinatura\"* OR \"webhook\"* OR \"stripe\"*"
        );
        assert_eq!(
            fts_any_query("não para the webhook webhook 2024").unwrap(),
            "\"webhook\"*"
        );
        assert!(fts_any_query("ok, e aí?").is_none());
    }

    #[test]
    fn queries_are_sanitized() {
        assert_eq!(
            fts_query("fila de e-mails").unwrap(),
            "\"fila\"* \"de\"* \"e\"* \"mails\"*"
        );
        assert_eq!(
            fts_query("Decisão OR x\"").unwrap(),
            "\"decisão\"* \"or\"* \"x\"*"
        );
        assert!(fts_query("  -- ").is_none());
    }
}
