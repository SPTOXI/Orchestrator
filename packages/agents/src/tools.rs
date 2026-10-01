//! What an agent can do that a plain session cannot (ADR-0015), and the
//! file locks, checked on the one path every tool call takes.
//!
//! `AgentTools` wraps the executor the app gives the sessions
//! (`RuntimeTools` → `EngineTools` → this, under the autonomy gate), so no
//! tool escapes the check and the Tool Runtime does not need to know that
//! agents exist. What it answers itself — its own tools and the writes the
//! locks refuse — never reaches the runtime, so it records them.

use crate::locks::LockManager;
use crate::service::AgentService;
use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::{
    Agent, AgentStatus, AuditEvent, CallOrigin, EventKind, EventSink, SessionId, ToolCall,
    ToolDefinition, ToolError, ToolErrorKind, ToolResult,
};
use orchestrator_engine::{audit_args, clip};
use orchestrator_memory::MemoryStore;
use orchestrator_providers::ToolExecutor;
use parking_lot::RwLock;
use schemars::generate::SchemaSettings;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

const GROUP: &str = "agent";

/// Tools that change files: what the locks are about. Reading is never
/// locked.
const WRITES: [(&str, &[&str]); 3] = [
    ("filesystem.write", &["path"]),
    ("filesystem.move", &["from", "to"]),
    ("filesystem.delete", &["path"]),
];

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FinishArgs {
    /// What was done, in a few lines: it becomes the task's result.
    result: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DelegateArgs {
    /// One line naming the subtask.
    title: String,
    /// What the subagent has to do, and what it already knows.
    description: Option<String>,
    /// Paths the subagent will work on, so it does not fight over files.
    files: Option<Vec<String>>,
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

/// The tools an agent has and a plain session does not.
pub fn definitions() -> &'static [ToolDefinition] {
    static DEFINITIONS: OnceLock<Vec<ToolDefinition>> = OnceLock::new();
    DEFINITIONS.get_or_init(|| {
        vec![
            ToolDefinition {
                name: "agent.finish".into(),
                group: GROUP.into(),
                description: "Ends this agent and reports what was done. The result goes to the \
                              task, which moves to review for a person to check. Call it also \
                              when the work cannot go on, saying what is missing."
                    .into(),
                read_only: false,
                parameters: schema::<FinishArgs>(),
            },
            ToolDefinition {
                name: "agent.delegate".into(),
                group: GROUP.into(),
                description: "Creates a subtask of this agent's task and queues a subagent for \
                              it. Only when it is justified: a large piece of work that can run \
                              on its own, in files this agent does not need. Small or coupled \
                              work is cheaper done here. The subagent starts when there is a \
                              free slot; this agent does not wait for it."
                    .into(),
                read_only: false,
                parameters: schema::<DelegateArgs>(),
            },
        ]
    })
}

pub fn is_agent_tool(name: &str) -> bool {
    definitions().iter().any(|d| d.name == name)
}

/// Where the `AgentService` is put after the `SessionManager` exists: the
/// manager needs the tools, and the tools need the manager.
#[derive(Default)]
pub struct AgentSlot(RwLock<Option<AgentService>>);

impl AgentSlot {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn install(&self, service: AgentService) {
        *self.0.write() = Some(service);
    }

    pub fn get(&self) -> Option<AgentService> {
        self.0.read().clone()
    }
}

/// The app's tool executor plus the agent tools and the file locks.
pub struct AgentTools {
    inner: Arc<dyn ToolExecutor>,
    store: Arc<MemoryStore>,
    locks: Arc<LockManager>,
    service: Arc<AgentSlot>,
    sink: Arc<dyn EventSink>,
}

impl AgentTools {
    pub fn new(
        inner: Arc<dyn ToolExecutor>,
        store: Arc<MemoryStore>,
        locks: Arc<LockManager>,
        service: Arc<AgentSlot>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            inner,
            store,
            locks,
            service,
            sink,
        }
    }

    /// `TOOL_CALLED` for what this layer answered itself, in the shape the
    /// runtime uses.
    fn record(&self, call: &ToolCall, result: &ToolResult, read_only: bool) {
        let summary = match &result.error {
            None => call.tool.clone(),
            Some(err) => format!("{} failed: {}", call.tool, clip(&err.message, 200)),
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
                    "ok": result.ok,
                    "error": result.error,
                    "durationMs": result.duration_ms,
                }),
            )
            .with_call(call.id.clone()),
        );
    }

    /// The agent behind a call, when an agent is driving the session. A
    /// session the user drives has no agent — and is never locked out.
    fn agent_of(&self, origin: &CallOrigin) -> Option<(Agent, SessionId)> {
        let session = match origin {
            CallOrigin::Agent {
                session_id: Some(id),
                ..
            } => id.clone(),
            _ => return None,
        };
        let agent = self.store.agent_of_session(session.as_str())?;
        (agent.status == AgentStatus::Running).then_some((agent, session))
    }

    /// Paths a call would change, as the tool named them.
    fn changed_paths(call: &ToolCall) -> Vec<String> {
        WRITES
            .iter()
            .find(|(tool, _)| *tool == call.tool)
            .map(|(_, keys)| {
                keys.iter()
                    .filter_map(|key| call.args.get(*key).and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Takes what the call needs. `Err` is the refusal the agent reads.
    fn guard(&self, call: &ToolCall) -> Result<(), ToolError> {
        let paths = Self::changed_paths(call);
        if paths.is_empty() {
            return Ok(());
        }
        let Some((agent, _)) = self.agent_of(&call.origin) else {
            return Ok(());
        };
        let project_path = self
            .store
            .project(&agent.project_id)
            .map(|p| PathBuf::from(p.path))
            .unwrap_or_default();
        match self.locks.take(&agent, &project_path, &paths) {
            None => Ok(()),
            Some(held) => Err(ToolError::new(
                ToolErrorKind::Locked,
                format!(
                    "{} está com o agente \"{}\" enquanto ele trabalha. Trabalhe em outro \
                     arquivo ou espere ele terminar.",
                    held.path, held.agent_title
                ),
            )),
        }
    }

    fn run(&self, call: &ToolCall) -> Result<Value, ToolError> {
        let service = self
            .service
            .get()
            .ok_or_else(|| ToolError::internal("o gerenciador de agentes não está ligado"))?;
        let session =
            match &call.origin {
                CallOrigin::Agent {
                    session_id: Some(id),
                    ..
                } => id.clone(),
                _ => return Err(ToolError::new(
                    ToolErrorKind::InvalidArgs,
                    "esta ferramenta é dos agentes: ela só existe dentro de uma sessão conduzida \
                     por um agente",
                )),
            };
        let args = if call.args.is_null() {
            json!({})
        } else {
            call.args.clone()
        };
        match call.tool.as_str() {
            "agent.finish" => {
                let args: FinishArgs = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_args(e.to_string()))?;
                if args.result.trim().is_empty() {
                    return Err(ToolError::invalid_args(
                        "diga o que foi feito: o resultado vai para a task",
                    ));
                }
                let agent = service
                    .report_finish(&session, &args.result)
                    .map_err(|e| ToolError::invalid_args(e.message))?;
                Ok(json!({
                    "agentId": agent.id,
                    "task": agent.task,
                    "accepted": true,
                    "note": "resultado registrado; a task vai para revisão quando este turno \
                             terminar",
                }))
            }
            "agent.delegate" => {
                let args: DelegateArgs = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_args(e.to_string()))?;
                let (task, agent) = service
                    .delegate(
                        &session,
                        args.title.trim(),
                        args.description.as_deref().unwrap_or_default(),
                        args.files.unwrap_or_default(),
                    )
                    .map_err(|e| ToolError::invalid_args(e.message))?;
                Ok(json!({
                    "taskId": task.id,
                    "agentId": agent.id,
                    "status": agent.status,
                    "note": "subtask criada e subagente na fila; este agente não espera por ele",
                }))
            }
            other => Err(ToolError::new(
                ToolErrorKind::UnknownTool,
                format!("unknown tool {other}"),
            )),
        }
    }
}

fn result_of(
    call: ToolCall,
    outcome: Result<Value, ToolError>,
    started: chrono::DateTime<chrono::Utc>,
) -> ToolResult {
    let (ok, output, error) = match outcome {
        Ok(output) => (true, output, None),
        Err(error) => (false, Value::Null, Some(error)),
    };
    let finished_at = Utc::now();
    ToolResult {
        call_id: call.id,
        tool: call.tool,
        ok,
        output,
        error,
        started_at: started,
        finished_at,
        duration_ms: (finished_at - started).num_milliseconds().max(0) as u64,
    }
}

#[async_trait]
impl ToolExecutor for AgentTools {
    fn tools(&self) -> Vec<ToolDefinition> {
        let mut tools = self.inner.tools();
        tools.extend(definitions().iter().cloned());
        tools
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        let started = Utc::now();
        if is_agent_tool(&call.tool) {
            let outcome = self.run(&call);
            let result = result_of(call.clone(), outcome, started);
            self.record(&call, &result, false);
            return result;
        }
        if let Err(denied) = self.guard(&call) {
            let result = result_of(call.clone(), Err(denied), started);
            self.record(&call, &result, false);
            return result;
        }
        self.inner.execute(call).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_tools_are_portable_and_named_as_the_master_document_says() {
        let names: Vec<_> = definitions().iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["agent.finish", "agent.delegate"]);
        for def in definitions() {
            let text = def.parameters.to_string();
            assert!(!text.contains("$ref"), "{}: {text}", def.name);
            assert_eq!(def.parameters["type"], "object", "{}", def.name);
            assert!(!def.read_only, "{}", def.name);
        }
        assert_eq!(definitions()[0].parameters["required"], json!(["result"]));
        assert_eq!(definitions()[1].parameters["required"], json!(["title"]));
    }

    #[test]
    fn only_the_tools_that_change_files_are_guarded() {
        let call = |tool: &str, args: Value| ToolCall::new(tool, args, CallOrigin::User);
        assert_eq!(
            AgentTools::changed_paths(&call("filesystem.write", json!({"path": "a.ts"}))),
            ["a.ts"]
        );
        assert_eq!(
            AgentTools::changed_paths(&call(
                "filesystem.move",
                json!({"from": "a.ts", "to": "b.ts"})
            )),
            ["a.ts", "b.ts"]
        );
        assert_eq!(
            AgentTools::changed_paths(&call("filesystem.delete", json!({"path": "a.ts"}))),
            ["a.ts"]
        );
        // Reading, listing and running commands are not locked.
        assert!(
            AgentTools::changed_paths(&call("filesystem.read", json!({"path": "a.ts"}))).is_empty()
        );
        assert!(
            AgentTools::changed_paths(&call("shell.execute", json!({"command": "ls"}))).is_empty()
        );
    }
}
