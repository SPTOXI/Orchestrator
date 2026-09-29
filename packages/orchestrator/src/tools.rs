//! Memory tools for AI agents (ADR-0013): the project's memory through the
//! same `tool_call` path as every other tool, audited with `TOOL_CALLED`
//! and the memory events.
//!
//! What an AI writes has source `agent`. Entries written by the user or the
//! detector are not changed by an AI (it records another one), and nothing
//! is deleted by an AI.

use crate::text::clip;
use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, EventSink, ToolCall, ToolDefinition, ToolError, ToolResult,
};
use orchestrator_memory::{
    DecisionInput, DecisionStatus, MemoryInput, MemoryKind, MemoryStore, Source,
};
use orchestrator_providers::ToolExecutor;
use schemars::generate::SchemaSettings;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

const GROUP_MEMORY: &str = "memory";
const GROUP_DECISION: &str = "decision";
const SEARCH_DEFAULT: u32 = 8;
const SEARCH_MAX: u32 = 20;
/// Longest string kept in the audit record of the arguments.
const AUDIT_MAX_STRING: usize = 500;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    /// Words to look for; results match any of them, best first.
    query: String,
    /// Maximum results (default 8, at most 20).
    limit: Option<u32>,
}

#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum KindArg {
    Architecture,
    Stack,
    Convention,
    Rule,
    Note,
}

impl From<KindArg> for MemoryKind {
    fn from(kind: KindArg) -> Self {
        match kind {
            KindArg::Architecture => Self::Architecture,
            KindArg::Stack => Self::Stack,
            KindArg::Convention => Self::Convention,
            KindArg::Rule => Self::Rule,
            KindArg::Note => Self::Note,
        }
    }
}

#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum StatusArg {
    Proposed,
    Accepted,
    Superseded,
    Rejected,
}

impl From<StatusArg> for DecisionStatus {
    fn from(status: StatusArg) -> Self {
        match status {
            StatusArg::Proposed => Self::Proposed,
            StatusArg::Accepted => Self::Accepted,
            StatusArg::Superseded => Self::Superseded,
            StatusArg::Rejected => Self::Rejected,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// Only entries of this kind.
    kind: Option<KindArg>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveMemoryArgs {
    /// Id of an entry created by an AI, to change it; omit to create one.
    id: Option<String>,
    kind: KindArg,
    title: String,
    content: String,
    tags: Option<Vec<String>>,
    /// Keep it at the top of the project memory.
    pinned: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DecisionListArgs {
    /// Only decisions with this status.
    status: Option<StatusArg>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveDecisionArgs {
    /// Id of a decision to change (e.g. its status); omit to record one.
    id: Option<String>,
    title: String,
    /// Why a decision is needed.
    context: Option<String>,
    /// What was decided.
    decision: String,
    /// What changes, costs and risks.
    consequences: Option<String>,
    /// Default: `proposed` for a new decision, unchanged for an existing one.
    status: Option<StatusArg>,
}

/// Draft-7 schema with every subschema inlined, like the runtime's tools
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

/// The memory tools offered to AI agents.
pub fn definitions() -> &'static [ToolDefinition] {
    static DEFINITIONS: OnceLock<Vec<ToolDefinition>> = OnceLock::new();
    DEFINITIONS.get_or_init(|| {
        let def = |name: &str, group: &str, read_only: bool, description: &str, parameters| {
            ToolDefinition {
                name: name.into(),
                group: group.into(),
                description: description.into(),
                read_only,
                parameters,
            }
        };
        vec![
            def(
                "memory.working",
                GROUP_MEMORY,
                true,
                "Working memory (L1) of the project: recent sessions, changed files, commands \
                 with their exit codes and recent errors.",
                schema::<Empty>(),
            ),
            def(
                "memory.search",
                GROUP_MEMORY,
                true,
                "Searches the project's memory: entries, decisions, messages of earlier \
                 sessions, handoffs and notable events (commits, commands, failures). Results \
                 match any of the words, best first.",
                schema::<SearchArgs>(),
            ),
            def(
                "memory.list",
                GROUP_MEMORY,
                true,
                "Project memory entries (L2): architecture, stack, conventions, rules and \
                 notes, pinned first.",
                schema::<ListArgs>(),
            ),
            def(
                "memory.save",
                GROUP_MEMORY,
                false,
                "Records knowledge in the project memory (L2) so that other agents and later \
                 sessions know it. Changes only entries created by AI agents; to correct one \
                 written by the user, record another.",
                schema::<SaveMemoryArgs>(),
            ),
            def(
                "decision.list",
                GROUP_DECISION,
                true,
                "Decisions of the project with their status (proposed, accepted, superseded, \
                 rejected), newest first.",
                schema::<DecisionListArgs>(),
            ),
            def(
                "decision.save",
                GROUP_DECISION,
                false,
                "Records a project decision (status proposed by default) or changes one, e.g. \
                 its status. Decisions are never deleted.",
                schema::<SaveDecisionArgs>(),
            ),
        ]
    })
}

pub fn is_memory_tool(name: &str) -> bool {
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

/// The project of the session that called, or the open project.
fn project_of(store: &MemoryStore, origin: &CallOrigin) -> Result<String, ToolError> {
    let from_session = match origin {
        CallOrigin::Agent {
            session_id: Some(id),
            ..
        } => store.session_project_id(id.as_str()),
        _ => None,
    };
    from_session
        .or_else(|| store.current_project().map(|p| p.id))
        .ok_or_else(|| ToolError::not_found("no project is open"))
}

fn json_of<T: serde::Serialize>(value: &T) -> Result<Value, ToolError> {
    serde_json::to_value(value).map_err(|e| ToolError::internal(e.to_string()))
}

type Outcome = Result<(Value, Vec<AuditEvent>), ToolError>;

fn run(store: &MemoryStore, call: &ToolCall) -> Outcome {
    let project = project_of(store, &call.origin)?;
    let failed = |message: String| ToolError::invalid_args(message);
    match call.tool.as_str() {
        "memory.working" => {
            parse::<Empty>(&call.args)?;
            let working = store
                .working_memory(&project)
                .map_err(ToolError::internal)?;
            Ok((json_of(&working)?, Vec::new()))
        }
        "memory.search" => {
            let args: SearchArgs = parse(&call.args)?;
            let limit = args.limit.unwrap_or(SEARCH_DEFAULT).clamp(1, SEARCH_MAX);
            let hits = store
                .search_related(&project, &args.query, &[], limit as usize)
                .map_err(ToolError::internal)?;
            Ok((json_of(&hits)?, Vec::new()))
        }
        "memory.list" => {
            let args: ListArgs = parse(&call.args)?;
            let kind = args.kind.map(MemoryKind::from);
            let entries: Vec<_> = store
                .memory_list(&project)
                .map_err(ToolError::internal)?
                .into_iter()
                .filter(|e| kind.is_none_or(|k| e.kind == k))
                .collect();
            Ok((json_of(&entries)?, Vec::new()))
        }
        "memory.save" => {
            let args: SaveMemoryArgs = parse(&call.args)?;
            let existing =
                match &args.id {
                    Some(id) => {
                        let entry = store
                            .memory_list(&project)
                            .map_err(ToolError::internal)?
                            .into_iter()
                            .find(|e| &e.id == id)
                            .ok_or_else(|| {
                                ToolError::not_found(format!("memory entry {id} not found"))
                            })?;
                        if entry.source != Source::Agent {
                            return Err(failed(format!(
                            "\"{}\" was written by the {}; AI agents do not change it — record \
                             another entry instead",
                            entry.title,
                            if entry.source == Source::User { "user" } else { "stack detector" }
                        )));
                        }
                        Some(entry)
                    }
                    None => None,
                };
            let (entry, event) = store
                .memory_save(
                    MemoryInput {
                        id: args.id,
                        project_id: project,
                        kind: args.kind.into(),
                        title: args.title,
                        content: args.content,
                        tags: args
                            .tags
                            .or_else(|| existing.as_ref().map(|e| e.tags.clone()))
                            .unwrap_or_default(),
                        pinned: args
                            .pinned
                            .or_else(|| existing.as_ref().map(|e| e.pinned))
                            .unwrap_or(false),
                    },
                    &call.origin,
                )
                .map_err(failed)?;
            Ok((json_of(&entry)?, vec![event]))
        }
        "decision.list" => {
            let args: DecisionListArgs = parse(&call.args)?;
            let status = args.status.map(DecisionStatus::from);
            let decisions: Vec<_> = store
                .decisions_list(&project)
                .map_err(ToolError::internal)?
                .into_iter()
                .filter(|d| status.is_none_or(|s| d.status == s))
                .collect();
            Ok((json_of(&decisions)?, Vec::new()))
        }
        "decision.save" => {
            let args: SaveDecisionArgs = parse(&call.args)?;
            let existing = match &args.id {
                Some(id) => Some(
                    store
                        .decisions_list(&project)
                        .map_err(ToolError::internal)?
                        .into_iter()
                        .find(|d| &d.id == id)
                        .ok_or_else(|| ToolError::not_found(format!("decision {id} not found")))?,
                ),
                None => None,
            };
            let (decision, event) = store
                .decision_save(
                    DecisionInput {
                        id: args.id,
                        project_id: project,
                        title: args.title,
                        context: args
                            .context
                            .or_else(|| existing.as_ref().map(|d| d.context.clone()))
                            .unwrap_or_default(),
                        decision: args.decision,
                        consequences: args
                            .consequences
                            .or_else(|| existing.as_ref().map(|d| d.consequences.clone()))
                            .unwrap_or_default(),
                        status: args
                            .status
                            .map(DecisionStatus::from)
                            .or_else(|| existing.as_ref().map(|d| d.status))
                            .unwrap_or(DecisionStatus::Proposed),
                    },
                    &call.origin,
                )
                .map_err(failed)?;
            Ok((json_of(&decision)?, vec![event]))
        }
        other => Err(ToolError::new(
            orchestrator_core::ToolErrorKind::UnknownTool,
            format!("unknown tool {other}"),
        )),
    }
}

/// Copy of the arguments with long strings cut, as the runtime records them.
fn audit_args(value: &Value) -> Value {
    match value {
        Value::String(s) if s.chars().count() > AUDIT_MAX_STRING => {
            Value::String(clip(s, AUDIT_MAX_STRING))
        }
        Value::Array(items) => Value::Array(items.iter().map(audit_args).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), audit_args(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn summary(call: &ToolCall, error: Option<&ToolError>) -> String {
    let target = ["query", "title", "kind", "status"]
        .iter()
        .find_map(|key| call.args.get(key).and_then(Value::as_str))
        .map(|value| format!(" {}", clip(value, 120)))
        .unwrap_or_default();
    match error {
        None => format!("{}{target}", call.tool),
        Some(err) => format!("{}{target} failed: {}", call.tool, clip(&err.message, 200)),
    }
}

/// The app's tool executor plus the memory tools.
pub struct EngineTools {
    inner: Arc<dyn ToolExecutor>,
    store: Arc<MemoryStore>,
    sink: Arc<dyn EventSink>,
}

impl EngineTools {
    pub fn new(
        inner: Arc<dyn ToolExecutor>,
        store: Arc<MemoryStore>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self { inner, store, sink }
    }
}

#[async_trait]
impl ToolExecutor for EngineTools {
    fn tools(&self) -> Vec<ToolDefinition> {
        let mut tools = self.inner.tools();
        tools.extend(definitions().iter().cloned());
        tools
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        if !is_memory_tool(&call.tool) {
            return self.inner.execute(call).await;
        }
        let started_at = Utc::now();
        let clock = Instant::now();
        let store = self.store.clone();
        let task_call = call.clone();
        let outcome: Outcome = tokio::task::spawn_blocking(move || run(&store, &task_call))
            .await
            .unwrap_or_else(|e| Err(ToolError::internal(format!("memory tool failed: {e}"))));
        let duration_ms = clock.elapsed().as_millis() as u64;
        let read_only = definitions()
            .iter()
            .find(|d| d.name == call.tool)
            .map(|d| d.read_only);
        let (ok, output, error, events) = match outcome {
            Ok((output, events)) => (true, output, None, events),
            Err(error) => (false, Value::Null, Some(error), Vec::new()),
        };
        self.sink.audit(
            AuditEvent::new(
                EventKind::ToolCalled,
                call.origin.clone(),
                summary(&call, error.as_ref()),
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
            call_id: call.id,
            tool: call.tool,
            ok,
            output,
            error,
            started_at,
            finished_at: Utc::now(),
            duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_are_portable() {
        let defs = definitions();
        let names: Vec<_> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "memory.working",
                "memory.search",
                "memory.list",
                "memory.save",
                "decision.list",
                "decision.save"
            ]
        );
        for def in defs {
            let text = def.parameters.to_string();
            assert!(!text.contains("$ref"), "{}: {text}", def.name);
            assert_eq!(def.parameters["type"], "object", "{}", def.name);
        }
        let save = &defs[3].parameters;
        assert_eq!(
            save["required"],
            json!(["kind", "title", "content"]),
            "{save}"
        );
        assert_eq!(
            save["properties"]["kind"]["enum"],
            json!(["architecture", "stack", "convention", "rule", "note"])
        );
        assert_eq!(
            defs.iter().filter(|d| d.read_only).count(),
            4,
            "only the save tools write"
        );
    }
}
