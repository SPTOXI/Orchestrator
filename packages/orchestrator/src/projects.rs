//! The projects that work together (ADR-0023): what the AI of one project
//! can do with the others the user linked to it.
//!
//! - `projects.related`: who they are — path, stack, how they relate, what
//!   their memory says, the AIs working on them.
//! - `projects.ask`: a question to the AI of a related project. It runs as
//!   a turn of a session of that project ("Conversa com <projeto>"), with
//!   that project's context, rules and autonomy, and the user sees it
//!   there; the answer comes back as the tool's output.
//! - `projects.request`: a task left in the related project's list.
//!
//! Only linked projects are reachable: the link is the user's consent. A
//! session that is answering another project's question does not ask back,
//! so two AIs never keep each other busy.

use crate::task::TaskService;
use crate::text::clip;
use crate::tools::{audit_args, project_of};
use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, EventSink, SessionId, SessionInfo, SessionStatus, TaskInput,
    TaskPriority, TaskStatus, ToolCall, ToolDefinition, ToolError, ToolErrorKind, ToolResult,
    TurnStatus,
};
use orchestrator_memory::{DecisionStatus, MemoryStore, ProjectLink};
use orchestrator_providers::{SessionManager, StartRequest, ToolExecutor};
use parking_lot::Mutex;
use schemars::generate::SchemaSettings;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const GROUP: &str = "projects";
/// Longest answer handed back to the asking AI.
const ANSWER_MAX: usize = 24_000;
/// How long a question waits for the other project's conversation to be
/// free.
const BUSY_WAIT: Duration = Duration::from_secs(600);
/// What `projects.related` lists of each project's memory.
const PINNED: usize = 6;
const DECISIONS: usize = 6;
const SESSIONS: usize = 4;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AskArgs {
    /// Name (or path) of a related project.
    project: String,
    /// What you need to know, with the context the other AI needs to
    /// answer it.
    question: String,
    /// Start a new conversation instead of continuing the one this project
    /// already has with it.
    fresh: Option<bool>,
}

#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum PriorityArg {
    Low,
    Normal,
    High,
    Urgent,
}

impl From<PriorityArg> for TaskPriority {
    fn from(priority: PriorityArg) -> Self {
        match priority {
            PriorityArg::Low => Self::Low,
            PriorityArg::Normal => Self::Normal,
            PriorityArg::High => Self::High,
            PriorityArg::Urgent => Self::Urgent,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RequestArgs {
    /// Name (or path) of a related project.
    project: String,
    /// One line naming what has to be done there.
    title: String,
    /// What is needed and why, with what the other project's AI needs to
    /// know to do it.
    description: Option<String>,
    priority: Option<PriorityArg>,
}

/// Draft-7 schema with every subschema inlined, like the other tools
/// (ADR-0010).
fn schema<T: JsonSchema>() -> Value {
    let generator = SchemaSettings::draft07()
        .with(|settings| {
            settings.inline_subschemas = true;
            settings.meta_schema = None;
        })
        .into_generator();
    let mut value = generator.into_root_schema_for::<T>().to_value();
    if let Some(object) = value.as_object_mut() {
        object.remove("title");
        object.remove("description");
        object
            .entry("properties")
            .or_insert_with(|| Value::Object(Default::default()));
    }
    value
}

fn def(name: &str, read_only: bool, description: &str, parameters: Value) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        group: GROUP.into(),
        description: description.into(),
        read_only,
        parameters,
    }
}

pub fn definitions() -> &'static [ToolDefinition] {
    static DEFINITIONS: OnceLock<Vec<ToolDefinition>> = OnceLock::new();
    DEFINITIONS.get_or_init(|| {
        vec![
            def(
                "projects.related",
                true,
                "The projects the user linked to this one because they work together (an API \
                 and the app that uses it, a library and who depends on it): path, stack, how \
                 they relate, their pinned memory and accepted decisions, open tasks and the AI \
                 sessions working on them. Their files can be read by absolute path.",
                schema::<Empty>(),
            ),
            def(
                "projects.ask",
                false,
                "Asks the AI that works on a related project and waits for its answer. It \
                 answers from that project's code, memory and history, in a conversation of \
                 that project the user can follow. Use it for what only that project knows \
                 (its API, contracts, conventions, plans); include what it needs to answer. It \
                 costs a turn of that AI, so ask once and well.",
                schema::<AskArgs>(),
            ),
            def(
                "projects.request",
                false,
                "Leaves a task in a related project's list, for the user or that project's AI \
                 to do: a change this project needs there. Say what and why.",
                schema::<RequestArgs>(),
            ),
        ]
    })
}

pub fn is_project_tool(name: &str) -> bool {
    definitions().iter().any(|d| d.name == name)
}

fn parse<T: DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    let args = if args.is_null() {
        json!({})
    } else {
        args.clone()
    };
    serde_json::from_value(args).map_err(|e| ToolError::invalid_args(e.to_string()))
}

/// Title of the conversation a project keeps with another one's AI.
fn conversation_title(asking: &str) -> String {
    format!("Conversa com {asking}")
}

fn same_path(a: &Path, b: &str) -> bool {
    let trim = |text: &str| text.trim_end_matches(['/', '\\']).to_owned();
    trim(&a.to_string_lossy()) == trim(b)
}

/// The linked project a call names, by name, path or id.
fn find_link<'a>(links: &'a [ProjectLink], wanted: &str) -> Option<&'a ProjectLink> {
    let wanted = wanted.trim();
    let lower = wanted.to_lowercase();
    links
        .iter()
        .find(|l| l.project.id == wanted || same_path(Path::new(&l.project.path), wanted))
        .or_else(|| {
            links
                .iter()
                .find(|l| l.project.name.to_lowercase() == lower)
        })
}

fn unknown_project(links: &[ProjectLink], wanted: &str) -> ToolError {
    let names: Vec<&str> = links.iter().map(|l| l.project.name.as_str()).collect();
    let known = if names.is_empty() {
        "Este projeto não tem projetos relacionados; quem os liga é o usuário, no painel \
         PROJECT."
            .to_owned()
    } else {
        format!("Os relacionados são: {}.", names.join(", "))
    };
    ToolError::not_found(format!(
        "\"{wanted}\" não é um projeto relacionado. {known}"
    ))
}

/// What the other AI receives: who asks, why it may answer, the question.
fn framed_question(asking: &str, asking_path: &str, note: &str, question: &str) -> String {
    let relation = if note.is_empty() {
        String::new()
    } else {
        format!(" (relação: {note})")
    };
    format!(
        "[Pergunta da IA do projeto {asking}, em {asking_path}, que trabalha junto com este \
         projeto{relation}.]\n\
         Responda com o que ela precisa saber deste projeto: consulte os arquivos, a memória e \
         o histórico daqui. Responda de forma direta e completa. Não mude nada aqui a não ser \
         que ela peça e faça sentido para este projeto.\n\n{question}"
    )
}

/// Removes the session from the answering set when the question ends,
/// whatever way it ends.
struct Answering<'a> {
    set: &'a Mutex<HashSet<SessionId>>,
    id: SessionId,
}

impl Drop for Answering<'_> {
    fn drop(&mut self) {
        self.set.lock().remove(&self.id);
    }
}

/// The services the tools need, which exist only after the session manager
/// that uses these tools.
struct Peers {
    sessions: SessionManager,
    tasks: TaskService,
}

type Outcome = Result<(Value, Vec<AuditEvent>), ToolError>;

/// The app's tools plus the tools between related projects.
pub struct ProjectTools {
    inner: Arc<dyn ToolExecutor>,
    store: Arc<MemoryStore>,
    sink: Arc<dyn EventSink>,
    peers: OnceLock<Peers>,
    /// Sessions answering another project's question now.
    answering: Mutex<HashSet<SessionId>>,
}

impl ProjectTools {
    pub fn new(
        inner: Arc<dyn ToolExecutor>,
        store: Arc<MemoryStore>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            inner,
            store,
            sink,
            peers: OnceLock::new(),
            answering: Mutex::new(HashSet::new()),
        }
    }

    /// Hands in the services created after the session manager. Until then
    /// `projects.ask` and `projects.request` are unavailable.
    pub fn connect(&self, sessions: SessionManager, tasks: TaskService) {
        let _ = self.peers.set(Peers { sessions, tasks });
    }

    fn peers(&self) -> Result<&Peers, ToolError> {
        self.peers
            .get()
            .ok_or_else(|| ToolError::internal("the project tools are not ready yet"))
    }

    fn links_of(&self, call: &ToolCall) -> Result<(String, Vec<ProjectLink>), ToolError> {
        let project = project_of(&self.store, &call.origin)?;
        let links = self.store.project_links(&project);
        Ok((project, links))
    }

    fn related(&self, call: &ToolCall) -> Outcome {
        parse::<Empty>(&call.args)?;
        let (_, links) = self.links_of(call)?;
        let sessions = self
            .peers
            .get()
            .map(|p| p.sessions.list())
            .unwrap_or_default();
        let projects: Vec<Value> = links
            .iter()
            .map(|link| {
                let project = &link.project;
                let pinned: Vec<Value> = self
                    .store
                    .memory_list(&project.id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|m| m.pinned)
                    .take(PINNED)
                    .map(|m| json!({"title": m.title, "content": clip(&m.content, 400)}))
                    .collect();
                let decisions: Vec<Value> = self
                    .store
                    .decisions_list(&project.id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|d| d.status == DecisionStatus::Accepted)
                    .take(DECISIONS)
                    .map(|d| json!({"title": d.title, "decision": clip(&d.decision, 300)}))
                    .collect();
                let open_tasks = self
                    .store
                    .tasks_list(Some(&project.id))
                    .into_iter()
                    .filter(|t| !matches!(t.status, TaskStatus::Done | TaskStatus::Cancelled))
                    .count();
                let mut working: Vec<&SessionInfo> = sessions
                    .iter()
                    .filter(|s| {
                        same_path(&s.project_path, &project.path)
                            && s.status != SessionStatus::Closed
                    })
                    .collect();
                working.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
                let working: Vec<Value> = working
                    .into_iter()
                    .take(SESSIONS)
                    .map(|s| {
                        json!({
                            "title": s.title,
                            "provider": s.provider,
                            "model": s.model,
                            "running": s.status == SessionStatus::Running,
                        })
                    })
                    .collect();
                json!({
                    "name": project.name,
                    "path": project.path,
                    "relation": link.note,
                    "stack": project.stack,
                    "openInApp": project.open_rank.is_some(),
                    "exists": Path::new(&project.path).is_dir(),
                    "pinnedMemory": pinned,
                    "acceptedDecisions": decisions,
                    "openTasks": open_tasks,
                    "sessions": working,
                })
            })
            .collect();
        Ok((json!({ "projects": projects }), Vec::new()))
    }

    /// The conversation this project keeps with `asking`'s AI, or a new one
    /// with the provider of the latest session of the project (else the
    /// asking session's).
    async fn conversation(
        &self,
        sessions: &SessionManager,
        call: &ToolCall,
        asking_name: &str,
        target: &orchestrator_memory::Project,
        fresh: bool,
    ) -> Result<SessionInfo, ToolError> {
        let title = conversation_title(asking_name);
        let all = sessions.list();
        let mut of_target: Vec<&SessionInfo> = all
            .iter()
            .filter(|s| same_path(&s.project_path, &target.path) && s.parent_id.is_none())
            .collect();
        of_target.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        if !fresh {
            if let Some(open) = of_target
                .iter()
                .find(|s| s.title == title && s.status != SessionStatus::Closed)
            {
                return Ok((*open).clone());
            }
        }
        let asking_session = match &call.origin {
            CallOrigin::Agent {
                session_id: Some(id),
                ..
            } => sessions.info(id).ok(),
            _ => None,
        };
        let model_of = of_target
            .iter()
            .find(|s| self.store.agent_of_session(s.id.as_str()).is_none() && s.title != title)
            .map(|s| (s.provider.clone(), s.model.clone()))
            .or_else(|| {
                asking_session
                    .as_ref()
                    .map(|s| (s.provider.clone(), s.model.clone()))
            });
        let (provider, model) = match model_of {
            Some((provider, model)) => (Some(provider), model),
            None => (None, None),
        };
        sessions
            .start(
                StartRequest {
                    provider,
                    model,
                    title: Some(title),
                    ..StartRequest::default()
                },
                PathBuf::from(&target.path),
                call.origin.clone(),
            )
            .await
            .map_err(|e| ToolError::new(ToolErrorKind::Spawn, e.message))
    }

    async fn ask(&self, call: &ToolCall, cancel: &CancellationToken) -> Outcome {
        let args: AskArgs = parse(&call.args)?;
        if args.question.trim().is_empty() {
            return Err(ToolError::invalid_args("diga a pergunta"));
        }
        if let CallOrigin::Agent {
            session_id: Some(id),
            ..
        } = &call.origin
        {
            if self.answering.lock().contains(id) {
                return Err(ToolError::new(
                    ToolErrorKind::Denied,
                    "Esta conversa está respondendo a uma pergunta de outro projeto: responda \
                     com o que você sabe ou consegue ver aqui. Quem perguntou pode consultar \
                     mais depois.",
                ));
            }
        }
        let peers = self.peers()?;
        let (asking_id, links) = self.links_of(call)?;
        let link = find_link(&links, &args.project)
            .ok_or_else(|| unknown_project(&links, &args.project))?;
        let target = &link.project;
        if !Path::new(&target.path).is_dir() {
            return Err(ToolError::not_found(format!(
                "a pasta do projeto {} não existe mais ({})",
                target.name, target.path
            )));
        }
        let asking = self
            .store
            .project(&asking_id)
            .ok_or_else(|| ToolError::not_found("o projeto desta sessão não está registrado"))?;

        let session = self
            .conversation(
                &peers.sessions,
                call,
                &asking.name,
                target,
                args.fresh.unwrap_or(false),
            )
            .await?;
        // Another question of the same conversation goes first.
        let started = Instant::now();
        loop {
            let busy = peers
                .sessions
                .info(&session.id)
                .map(|info| info.status == SessionStatus::Running)
                .unwrap_or(false);
            if !busy {
                break;
            }
            if started.elapsed() > BUSY_WAIT {
                return Err(ToolError::new(
                    ToolErrorKind::Locked,
                    format!(
                        "a conversa com o projeto {} ficou ocupada por mais de {} minutos",
                        target.name,
                        BUSY_WAIT.as_secs() / 60
                    ),
                ));
            }
            tokio::select! {
                _ = cancel.cancelled() => {
                    return Err(ToolError::new(ToolErrorKind::Cancelled, "turno cancelado"));
                }
                _ = tokio::time::sleep(Duration::from_millis(300)) => {}
            }
        }

        self.answering.lock().insert(session.id.clone());
        let _answering = Answering {
            set: &self.answering,
            id: session.id.clone(),
        };
        let message = framed_question(&asking.name, &asking.path, &link.note, &args.question);
        let turn = peers
            .sessions
            .execute(&session.id, message, call.origin.clone());
        let result = tokio::select! {
            result = turn => result,
            _ = cancel.cancelled() => {
                let _ = peers.sessions.cancel(&session.id).await;
                return Err(ToolError::new(
                    ToolErrorKind::Cancelled,
                    format!("turno cancelado: a pergunta ao projeto {} foi interrompida", target.name),
                ));
            }
        }
        .map_err(|e| ToolError::new(ToolErrorKind::Spawn, e.message))?;
        match result.status {
            TurnStatus::Completed => {}
            TurnStatus::Cancelled => {
                return Err(ToolError::new(
                    ToolErrorKind::Cancelled,
                    format!("a resposta do projeto {} foi cancelada", target.name),
                ))
            }
            TurnStatus::Failed => {
                return Err(ToolError::new(
                    ToolErrorKind::Io,
                    format!(
                        "a IA do projeto {} não respondeu: {}",
                        target.name,
                        result.error.as_deref().unwrap_or("erro desconhecido")
                    ),
                ))
            }
        }
        let event = AuditEvent::new(
            EventKind::ProjectAsked,
            call.origin.clone(),
            format!(
                "{} perguntou a {}: {}",
                asking.name,
                target.name,
                clip(&args.question, 160)
            ),
            json!({
                "projectId": asking.id,
                "targetProjectId": target.id,
                "targetProject": target.name,
                "targetSessionId": session.id,
                "question": clip(&args.question, 2000),
                "answer": clip(&result.text, 2000),
            }),
        );
        Ok((
            json!({
                "project": target.name,
                "sessionId": session.id,
                "answer": clip(&result.text, ANSWER_MAX),
            }),
            vec![event],
        ))
    }

    fn request(&self, call: &ToolCall) -> Outcome {
        let args: RequestArgs = parse(&call.args)?;
        let peers = self.peers()?;
        let (asking_id, links) = self.links_of(call)?;
        let link = find_link(&links, &args.project)
            .ok_or_else(|| unknown_project(&links, &args.project))?;
        let target = &link.project;
        let asking = self
            .store
            .project(&asking_id)
            .map(|p| p.name)
            .unwrap_or_else(|| "outro projeto".to_owned());
        let description = format!(
            "{}\n\nPedido pela IA do projeto {asking} (projeto relacionado).",
            args.description.unwrap_or_default().trim()
        );
        let task = peers
            .tasks
            .save(
                TaskInput {
                    project_id: Some(target.id.clone()),
                    title: Some(args.title),
                    description: Some(description.trim().to_owned()),
                    priority: args.priority.map(Into::into),
                    ..TaskInput::default()
                },
                call.origin.clone(),
            )
            .map_err(|e| ToolError::invalid_args(e.message))?;
        Ok((
            json!({
                "project": target.name,
                "taskId": task.id,
                "title": task.title,
                "status": task.status,
            }),
            Vec::new(),
        ))
    }

    fn record(
        &self,
        call: &ToolCall,
        started_at: chrono::DateTime<Utc>,
        clock: Instant,
        outcome: Outcome,
    ) -> ToolResult {
        let duration_ms = clock.elapsed().as_millis() as u64;
        let read_only = definitions()
            .iter()
            .find(|d| d.name == call.tool)
            .map(|d| d.read_only);
        let (ok, output, error, events) = match outcome {
            Ok((output, events)) => (true, output, None, events),
            Err(error) => (false, Value::Null, Some(error), Vec::new()),
        };
        let target = call
            .args
            .get("project")
            .and_then(Value::as_str)
            .map(|p| format!(" {}", clip(p, 80)))
            .unwrap_or_default();
        let summary = match &error {
            None => format!("{}{target}", call.tool),
            Some(err) => format!("{}{target} failed: {}", call.tool, clip(&err.message, 200)),
        };
        self.sink.audit(
            AuditEvent::new(
                EventKind::ToolCalled,
                call.origin.clone(),
                summary,
                json!({
                    "tool": call.tool,
                    "readOnly": read_only,
                    "args": audit_args(&call.args),
                    "ok": ok,
                    "error": error,
                    "durationMs": duration_ms,
                }),
            )
            .with_call(call.id.clone()),
        );
        for event in events {
            self.sink.audit(event.with_call(call.id.clone()));
        }
        ToolResult {
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok,
            output,
            error,
            started_at,
            finished_at: Utc::now(),
            duration_ms,
        }
    }
}

#[async_trait]
impl ToolExecutor for ProjectTools {
    fn tools(&self) -> Vec<ToolDefinition> {
        let mut tools = self.inner.tools();
        tools.extend(definitions().iter().cloned());
        tools
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        self.execute_with(call, CancellationToken::new()).await
    }

    async fn execute_with(&self, call: ToolCall, cancel: CancellationToken) -> ToolResult {
        if !is_project_tool(&call.tool) {
            return self.inner.execute_with(call, cancel).await;
        }
        let started_at = Utc::now();
        let clock = Instant::now();
        let outcome = match call.tool.as_str() {
            "projects.related" => self.related(&call),
            "projects.ask" => self.ask(&call, &cancel).await,
            "projects.request" => self.request(&call),
            other => Err(ToolError::new(
                ToolErrorKind::UnknownTool,
                format!("unknown tool {other}"),
            )),
        };
        self.record(&call, started_at, clock, outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_are_portable() {
        let names: Vec<_> = definitions().iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            ["projects.related", "projects.ask", "projects.request"]
        );
        for def in definitions() {
            let text = def.parameters.to_string();
            assert!(!text.contains("$ref"), "{}: {text}", def.name);
            assert_eq!(def.parameters["type"], "object", "{}", def.name);
        }
        assert_eq!(
            definitions()[1].parameters["required"],
            json!(["project", "question"])
        );
    }

    #[test]
    fn the_question_says_who_asks_and_why() {
        let text = framed_question(
            "app",
            "/p/app",
            "o app usa a API",
            "Qual rota lista usuários?",
        );
        assert!(text.contains("projeto app, em /p/app"), "{text}");
        assert!(text.contains("(relação: o app usa a API)"), "{text}");
        assert!(text.ends_with("Qual rota lista usuários?"), "{text}");
        assert!(!framed_question("app", "/p/app", "", "?").contains("relação"));
    }
}
