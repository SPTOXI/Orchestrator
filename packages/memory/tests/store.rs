//! The local database end to end: history with project tagging, projects,
//! sessions, L1/L2/L3 memory, decisions and deliberations.

use chrono::{Duration, Utc};
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, SessionEvent, SessionId, SessionInfo, SessionLogEntry,
    SessionStatus, TokenUsage, ToolCallId, TurnId,
};
use orchestrator_memory::{
    DecisionInput, DecisionStatus, HistoryQuery, MemoryInput, MemoryKind, MemoryStore,
    RecentImport, Source, StoredSession,
};
use serde_json::{json, Value};

fn opened(path: &str, name: &str, languages: &[&str]) -> AuditEvent {
    AuditEvent::new(
        EventKind::ProjectOpened,
        CallOrigin::User,
        format!("opened project {name}"),
        json!({
            "name": name, "path": path, "languages": languages, "frameworks": ["React"],
            "packageManagers": ["pnpm"], "runtimes": [{"name": "node", "version": "22"}],
            "databases": [], "tools": ["Vitest"],
            "docker": {"dockerfiles": ["Dockerfile"], "composeFiles": [], "images": []},
            "monorepo": false,
        }),
    )
}

fn event(kind: EventKind, origin: CallOrigin, summary: &str, data: Value) -> AuditEvent {
    AuditEvent::new(kind, origin, summary, data)
}

/// Records like the app's sink: the event, then its follow-ups.
fn record(store: &MemoryStore, event: &AuditEvent) -> Vec<AuditEvent> {
    let follow = store.record(event);
    for next in &follow {
        assert!(store.record(next).is_empty());
    }
    follow
}

fn session_info(id: &SessionId, project: &str, title: &str) -> SessionInfo {
    let now = Utc::now();
    SessionInfo {
        id: id.clone(),
        provider: "nuvem".into(),
        title: title.into(),
        model: Some("gpt-medio".into()),
        project_path: project.into(),
        parent_id: None,
        status: SessionStatus::Idle,
        native_ref: Some("ref-1".into()),
        created_at: now,
        updated_at: now,
        turns: 0,
        usage: TokenUsage::default(),
        last_error: None,
    }
}

#[test]
fn projects_are_registered_from_the_history() {
    let store = MemoryStore::in_memory();
    let follow = record(&store, &opened("/p/saas", "saas", &["TypeScript"]));
    let kinds: Vec<_> = follow.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, [EventKind::ProjectCreated, EventKind::MemorySaved]);
    let project = store.current_project().unwrap();
    assert_eq!(
        (project.name.as_str(), project.path.as_str()),
        ("saas", "/p/saas")
    );
    assert_eq!(follow[0].data["projectId"], json!(project.id));

    // The detector's stack is L2 memory.
    let entries = store.memory_list(&project.id).unwrap();
    assert_eq!(entries.len(), 1);
    let stack = &entries[0];
    assert_eq!(
        (stack.kind, stack.source, stack.pinned),
        (MemoryKind::Stack, Source::Detector, true)
    );
    assert_eq!(
        stack.content,
        "Linguagens: TypeScript\nFrameworks: React\nGerenciadores de pacote: pnpm\nRuntimes: node 22\nFerramentas: Vitest\nDocker: Dockerfile"
    );

    // Reopening: nothing new; a changed stack refreshes the entry.
    assert!(record(&store, &opened("/p/saas", "saas", &["TypeScript"])).is_empty());
    let follow = record(&store, &opened("/p/saas", "saas", &["TypeScript", "Rust"]));
    assert_eq!(follow.len(), 1);
    assert!(store.memory_list(&project.id).unwrap()[0]
        .content
        .starts_with("Linguagens: TypeScript, Rust"));

    // Once the user edits it, the detector leaves it alone.
    let mut edited = store.memory_list(&project.id).unwrap().remove(0);
    edited.content = "Rust + TypeScript, Tauri".into();
    store
        .memory_save(
            MemoryInput {
                id: Some(edited.id.clone()),
                project_id: project.id.clone(),
                kind: MemoryKind::Stack,
                title: edited.title.clone(),
                content: edited.content.clone(),
                tags: edited.tags.clone(),
                pinned: true,
            },
            &CallOrigin::User,
        )
        .unwrap();
    assert!(record(&store, &opened("/p/saas", "saas", &["Go"])).is_empty());
    let entries = store.memory_list(&project.id).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (entries[0].source, entries[0].content.as_str()),
        (Source::User, "Rust + TypeScript, Tauri")
    );

    // Recent list, forgetting and the import of the old localStorage list.
    record(&store, &opened("/p/api", "api", &["Python"]));
    let recent: Vec<_> = store
        .projects_recent(10)
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(recent, ["api", "saas"]);
    store.project_forget(&project.id).unwrap();
    assert_eq!(store.projects_recent(10).len(), 1);
    let added = store
        .projects_import_recent(&[
            RecentImport {
                path: "/p/api".into(),
                name: "api".into(),
                opened_at: Utc::now(),
            },
            RecentImport {
                path: "/p/old".into(),
                name: "old".into(),
                opened_at: Utc::now() - Duration::days(3),
            },
        ])
        .unwrap();
    assert_eq!(added, 1);
    let recent: Vec<_> = store
        .projects_recent(10)
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(recent, ["api", "old"]);
}

#[test]
fn events_are_tagged_with_their_project_and_paged() {
    let store = MemoryStore::in_memory();
    record(&store, &opened("/p/a", "a", &[]));
    let a = store.current_project().unwrap().id;
    let session = SessionId::new();
    store
        .session_save(&StoredSession {
            info: session_info(&session, "/p/a", "Sessão A"),
            native: json!({}),
            spec: json!({}),
        })
        .unwrap();
    // B opened like the app does: `project.open` is recorded before its
    // PROJECT_OPENED, while A is still the current project.
    let call = ToolCallId::new();
    record(
        &store,
        &event(
            EventKind::ToolCalled,
            CallOrigin::User,
            "project.open /p/b",
            json!({"tool": "project.open", "args": {"path": "/p/b"}, "ok": true}),
        )
        .with_call(call.clone()),
    );
    record(&store, &opened("/p/b", "b", &[]).with_call(call));
    let b = store.current_project().unwrap().id;

    // A tool call of a session of project A, recorded while B is open.
    let agent = CallOrigin::session(&session, &"nuvem".into());
    record(
        &store,
        &event(
            EventKind::ToolCalled,
            agent,
            "filesystem.read",
            json!({"tool": "filesystem.read", "ok": true, "readOnly": true}),
        ),
    );
    record(
        &store,
        &event(
            EventKind::SessionStarted,
            CallOrigin::User,
            "session started",
            json!({"projectPath": "/p/a"}),
        ),
    );
    for i in 0..5 {
        record(
            &store,
            &event(
                EventKind::CommandExecuted,
                CallOrigin::User,
                &format!("comando {i}"),
                json!({"command": format!("echo {i}"), "exitCode": 0}),
            ),
        );
    }

    let page = |query: HistoryQuery| store.history(&query).unwrap();
    let in_a = page(HistoryQuery {
        project_id: Some(a.clone()),
        ..Default::default()
    });
    let kinds: Vec<_> = in_a.events.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            EventKind::ProjectOpened,
            EventKind::ProjectCreated,
            EventKind::MemorySaved,
            EventKind::ToolCalled,
            EventKind::SessionStarted
        ]
    );
    assert!(in_a.next.is_none());
    let in_b = page(HistoryQuery {
        project_id: Some(b.clone()),
        ..Default::default()
    });
    let kinds: Vec<_> = in_b.events.iter().take(4).map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            EventKind::ToolCalled,
            EventKind::ProjectOpened,
            EventKind::ProjectCreated,
            EventKind::MemorySaved
        ]
    );
    assert_eq!(in_b.events.len(), 4 + 5);

    // Pages go back in time; events inside a page are oldest first.
    let first = page(HistoryQuery {
        limit: Some(3),
        ..Default::default()
    });
    let summaries: Vec<_> = first.events.iter().map(|e| e.summary.as_str()).collect();
    assert_eq!(summaries, ["comando 2", "comando 3", "comando 4"]);
    let second = page(HistoryQuery {
        limit: Some(3),
        before: first.next.clone(),
        ..Default::default()
    });
    let summaries: Vec<_> = second.events.iter().map(|e| e.summary.as_str()).collect();
    assert_eq!(summaries, ["session started", "comando 0", "comando 1"]);

    // Filters.
    let reads = page(HistoryQuery {
        hide_reads: true,
        project_id: Some(a.clone()),
        ..Default::default()
    });
    assert_eq!(reads.events.len(), 4);
    let text = page(HistoryQuery {
        text: Some("COMANDO 3".into()),
        ..Default::default()
    });
    assert_eq!(text.events.len(), 1);
    let kinds = page(HistoryQuery {
        kinds: vec![EventKind::ProjectOpened, EventKind::ProjectCreated],
        ..Default::default()
    });
    assert_eq!(kinds.events.len(), 4);
}

#[test]
fn the_old_jsonl_history_is_imported_once() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("audit.jsonl");
    let old = [
        opened("/p/legacy", "legacy", &["Go"]),
        event(
            EventKind::GitCommit,
            CallOrigin::User,
            "commit abc: corrige fila",
            json!({"projectPath": "/p/legacy", "hash": "abc"}),
        ),
    ];
    let mut text: String = old
        .iter()
        .map(|e| serde_json::to_string(e).unwrap() + "\n")
        .collect();
    text.push_str("{\"torn\": ");
    std::fs::write(&jsonl, text).unwrap();

    let (store, warning) = MemoryStore::open(&dir.path().join("orchestrator.db"));
    assert!(warning.is_none());
    assert_eq!(store.import_jsonl(&jsonl).unwrap(), 2);
    assert!(!jsonl.exists());
    assert!(dir.path().join("audit.jsonl.imported").exists());
    assert_eq!(store.import_jsonl(&jsonl).unwrap(), 0);

    // The project came from the history, with its original date and no
    // PROJECT_CREATED or stack memory made up after the fact.
    let project = store.project_by_path("/p/legacy").unwrap();
    assert_eq!(project.created_at, old[0].at);
    let page = store.history(&HistoryQuery::default()).unwrap();
    assert_eq!(page.events.len(), 2);
    assert!(store.memory_list(&project.id).unwrap().is_empty());
    // The commit is searchable (L3).
    let hits = store.search(&project.id, "fila", 10).unwrap();
    assert_eq!(hits[0].kind, "event");
}

#[test]
fn sessions_and_transcripts_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("orchestrator.db");
    let id = SessionId::new();
    let turn = TurnId::new();
    {
        let (store, _) = MemoryStore::open(&path);
        record(&store, &opened("/p/a", "a", &[]));
        store
            .session_save(&StoredSession {
                info: session_info(&id, "/p/a", "Corrigir timeout"),
                native: json!({"reference": "ref-1", "model": "gpt-medio", "data": {"conversation": [1, 2]}}),
                spec: json!({"instructions": null}),
            })
            .unwrap();
        let entry = |seq, event| SessionLogEntry {
            seq,
            at: Utc::now(),
            event,
        };
        store
            .session_append(
                &id,
                &[
                    entry(
                        1,
                        SessionEvent::TurnStarted {
                            turn_id: turn.clone(),
                            input: "Corrija o timeout do worker de e-mails".into(),
                        },
                    ),
                    entry(
                        3,
                        SessionEvent::TextDelta {
                            turn_id: turn.clone(),
                            text: "O timeout vem da fila de retentativas.".into(),
                        },
                    ),
                ],
            )
            .unwrap();
        // Replacing an entry keeps one row (and one search hit).
        store
            .session_append(
                &id,
                &[entry(
                    3,
                    SessionEvent::TextDelta {
                        turn_id: turn.clone(),
                        text: "O timeout vem da fila de retentativas, corrigido.".into(),
                    },
                )],
            )
            .unwrap();
        let mut closed = session_info(&id, "/p/a", "Corrigir timeout");
        closed.status = SessionStatus::Closed;
        closed.turns = 1;
        store
            .session_save(&StoredSession {
                info: closed,
                native: json!({"reference": "ref-2"}),
                spec: json!({}),
            })
            .unwrap();
    }
    let (store, _) = MemoryStore::open(&path);
    let loaded = store.sessions_load().unwrap();
    assert_eq!(loaded.len(), 1);
    let (session, entries) = &loaded[0];
    assert_eq!(
        (session.info.status, session.info.turns),
        (SessionStatus::Closed, 1)
    );
    assert_eq!(session.native["reference"], "ref-2");
    assert_eq!(entries.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 3]);

    let project = store.project_by_path("/p/a").unwrap();
    let hits = store.search(&project.id, "retentativas", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        (hits[0].kind.as_str(), hits[0].ref_id.as_str()),
        ("message", id.as_str())
    );
    assert_eq!(hits[0].title, "Corrigir timeout");
    assert!(
        hits[0].snippet.contains("[retentativas]"),
        "{}",
        hits[0].snippet
    );
    let overview = store.overview(&project.id).unwrap();
    assert_eq!(overview.sessions, 1);
    assert_eq!(overview.working.sessions[0].title, "Corrigir timeout");
}

#[test]
fn memory_entries_and_decisions() {
    let store = MemoryStore::in_memory();
    record(&store, &opened("/p/a", "a", &[]));
    let project = store.current_project().unwrap().id;

    let (rule, event) = store
        .memory_save(
            MemoryInput {
                id: None,
                project_id: project.clone(),
                kind: MemoryKind::Rule,
                title: "  Nunca editar migrações aplicadas ".into(),
                content: "Crie uma migração nova.".into(),
                tags: vec!["banco".into(), "Banco".into(), " ".into(), "sql".into()],
                pinned: false,
            },
            &CallOrigin::User,
        )
        .unwrap();
    assert_eq!(rule.title, "Nunca editar migrações aplicadas");
    assert_eq!(rule.tags, ["banco", "sql"]);
    assert_eq!(event.kind, EventKind::MemorySaved);
    assert_eq!(event.data["created"], true);
    assert!(event.summary.starts_with("memória regra · "));

    // An agent's edit changes the source.
    let (updated, _) = store
        .memory_save(
            MemoryInput {
                id: Some(rule.id.clone()),
                content: "Crie uma migração nova (nunca edite).".into(),
                pinned: true,
                ..MemoryInput {
                    id: None,
                    project_id: project.clone(),
                    kind: MemoryKind::Rule,
                    title: rule.title.clone(),
                    content: String::new(),
                    tags: vec![],
                    pinned: false,
                }
            },
            &CallOrigin::session(&SessionId::new(), &"nuvem".into()),
        )
        .unwrap();
    assert_eq!(
        (updated.source, updated.pinned, updated.created_at),
        (Source::Agent, true, rule.created_at)
    );

    let bad = |input: MemoryInput| store.memory_save(input, &CallOrigin::User).unwrap_err();
    let base = MemoryInput {
        id: None,
        project_id: project.clone(),
        kind: MemoryKind::Note,
        title: "x".into(),
        content: String::new(),
        tags: vec![],
        pinned: false,
    };
    assert_eq!(
        bad(MemoryInput {
            title: " ".into(),
            ..base.clone()
        }),
        "título é obrigatório"
    );
    assert!(bad(MemoryInput {
        project_id: "nope".into(),
        ..base.clone()
    })
    .contains("não encontrado"));
    assert!(bad(MemoryInput {
        id: Some("nope".into()),
        ..base.clone()
    })
    .contains("não encontrada"));

    let removed = store.memory_delete(&rule.id, &CallOrigin::User).unwrap();
    assert_eq!(removed.kind, EventKind::MemoryRemoved);
    assert!(store
        .memory_list(&project)
        .unwrap()
        .iter()
        .all(|e| e.id != rule.id));
    assert!(store.search(&project, "migração", 10).unwrap().is_empty());

    // Decisions change status, never disappear.
    let input = DecisionInput {
        id: None,
        project_id: project.clone(),
        title: "Filas no Redis".into(),
        context: "Precisamos de retentativas.".into(),
        decision: "Usar Redis Streams.".into(),
        consequences: "Mais um serviço no compose.".into(),
        status: DecisionStatus::Proposed,
    };
    let (decision, event) = store
        .decision_save(input.clone(), &CallOrigin::User)
        .unwrap();
    assert_eq!(
        event.summary,
        "decisão registrada (proposta) · Filas no Redis"
    );
    let (accepted, event) = store
        .decision_save(
            DecisionInput {
                id: Some(decision.id.clone()),
                status: DecisionStatus::Accepted,
                ..input.clone()
            },
            &CallOrigin::User,
        )
        .unwrap();
    assert_eq!(accepted.status, DecisionStatus::Accepted);
    assert_eq!(event.summary, "decisão proposta → aceita · Filas no Redis");
    assert_eq!(event.data["previousStatus"], "proposed");
    assert_eq!(store.decisions_list(&project).unwrap().len(), 1);
    let hits = store.search(&project, "redis streams", 10).unwrap();
    assert_eq!(
        (hits[0].kind.as_str(), hits[0].ref_id.as_str()),
        ("decision", decision.id.as_str())
    );
    assert!(store
        .decision_save(
            DecisionInput {
                decision: " ".into(),
                ..input
            },
            &CallOrigin::User
        )
        .is_err());
}

#[test]
fn working_memory_comes_from_the_history() {
    let store = MemoryStore::in_memory();
    record(&store, &opened("/p/a", "a", &[]));
    let project = store.current_project().unwrap().id;
    let agent = CallOrigin::session(&SessionId::new(), &"nuvem".into());
    let files = [
        ("src/a.rs", "modified"),
        ("src/b.rs", "created"),
        ("src/a.rs", "modified"),
    ];
    for (path, change) in files {
        record(
            &store,
            &event(
                EventKind::FileChanged,
                agent.clone(),
                path,
                json!({"path": path, "change": change}),
            ),
        );
    }
    record(
        &store,
        &event(
            EventKind::CommandExecuted,
            CallOrigin::User,
            "cargo test",
            json!({"command": "cargo test", "exitCode": 101, "stderrTail": "test falhou"}),
        ),
    );
    record(
        &store,
        &event(
            EventKind::ToolCalled,
            agent.clone(),
            "filesystem.read falhou",
            json!({"tool": "filesystem.read", "ok": false, "error": {"kind": "NOT_FOUND", "message": "no such file"}}),
        ),
    );
    record(
        &store,
        &event(
            EventKind::TurnCompleted,
            agent.clone(),
            "turn failed",
            json!({"status": "failed", "error": "429 rate limited"}),
        ),
    );
    record(
        &store,
        &event(
            EventKind::ProcessExited,
            CallOrigin::User,
            "stopped",
            json!({"exitCode": 143, "stopped": true}),
        ),
    );

    let working = store.working_memory(&project).unwrap();
    let paths: Vec<_> = working.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["src/a.rs", "src/b.rs"], "latest first, no repeats");
    assert_eq!(working.files[0].by, "agent");
    assert_eq!(working.commands[0].exit_code, Some(101));
    let details: Vec<_> = working
        .errors
        .iter()
        .map(|e| e.detail.clone().unwrap_or_default())
        .collect();
    assert_eq!(
        details,
        ["429 rate limited", "no such file", "test falhou"],
        "a stopped process is not an error"
    );
    let hits = store.search(&project, "rate limited", 10).unwrap();
    assert_eq!(hits[0].kind, "event");
}

#[test]
fn deliberations_keep_the_cache_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("orchestrator.db");
    let now = Utc::now();
    {
        let (store, _) = MemoryStore::open(&path);
        store
            .deliberation_save(
                "d1",
                &now,
                Some(("k1", now + Duration::minutes(60))),
                &json!({"id": "d1"}),
            )
            .unwrap();
        store
            .deliberation_save(
                "d0",
                &(now - Duration::hours(3)),
                Some(("k0", now - Duration::hours(2))),
                &json!({"id": "d0"}),
            )
            .unwrap();
        store
            .deliberation_save(
                "d2",
                &(now + Duration::seconds(1)),
                None,
                &json!({"id": "d2"}),
            )
            .unwrap();
    }
    let (store, _) = MemoryStore::open(&path);
    assert_eq!(
        store.deliberation_cached("k1", &Utc::now()).unwrap()["id"],
        "d1"
    );
    assert!(
        store.deliberation_cached("k0", &Utc::now()).is_none(),
        "expired"
    );
    let ids: Vec<_> = store
        .deliberations_recent(10)
        .iter()
        .map(|d| d["id"].clone())
        .collect();
    assert_eq!(ids, [json!("d2"), json!("d1"), json!("d0")]);
    store.deliberations_clear_cache().unwrap();
    assert!(store.deliberation_cached("k1", &Utc::now()).is_none());
    assert_eq!(store.deliberations_recent(10).len(), 3);
}

#[test]
fn an_unusable_file_falls_back_to_memory() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("not-a-dir");
    std::fs::write(&blocker, "x").unwrap();
    let (store, warning) = MemoryStore::open(&blocker.join("orchestrator.db"));
    assert!(warning.unwrap().contains("kept in memory"));
    assert!(store.path().is_none());
    record(&store, &opened("/p/a", "a", &[]));
    assert_eq!(store.projects_recent(5).len(), 1);
}
