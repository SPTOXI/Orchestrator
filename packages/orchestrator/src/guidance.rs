//! Development rules and skills (ADR-0021): what the user wants every AI to
//! follow, and instructions for kinds of work that an AI loads when a task
//! calls for them.
//!
//! - **Rules**: the user's own (`rules.md` in the app's data folder, for
//!   every project) plus the project's instruction files (`AGENTS.md`,
//!   `CLAUDE.md`, `.orchestrator/rules.md`) when the setting allows. They go
//!   whole into the session's system instructions, with the project context.
//! - **Skills**: folders with a `SKILL.md` (front matter `name` and
//!   `description`, then the instructions), as in Claude Code. The
//!   Orchestrator's own (`<data>/skills`), the project's
//!   (`.orchestrator/skills`, `.claude/skills`) and the user's Claude Code
//!   skills (`~/.claude/skills`). Only names and descriptions go into the
//!   instructions; an AI reads one with `skill.read` when a task matches.
//!
//! Nothing is written inside a project: the Orchestrator edits only its own
//! rules and skills.

use crate::text::clip;
use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::{
    AuditEvent, CallOrigin, ContextSectionSummary, ContextSummary, EventKind, EventSink, ToolCall,
    ToolDefinition, ToolError, ToolResult,
};
use orchestrator_memory::MemoryStore;
use orchestrator_providers::{AttachedContext, ContextRequest, ContextSource, ToolExecutor};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Longest project instruction file sent whole (characters).
const MAX_RULE_FILE: usize = 20_000;
/// Longest user rules (characters).
pub const MAX_USER_RULES: usize = 40_000;
/// Longest skill body `skill.read` returns (characters).
const MAX_SKILL_BODY: usize = 60_000;
/// Files listed with a skill.
const MAX_SKILL_FILES: usize = 50;
/// Project files with instructions for AIs, in this order.
pub const PROJECT_RULE_FILES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", ".orchestrator/rules.md"];

/// What the user chose (`guidance.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GuidanceSettings {
    /// Send the rules to the AIs.
    pub rules_enabled: bool,
    /// Also the project's `AGENTS.md`, `CLAUDE.md`, `.orchestrator/rules.md`.
    pub project_rule_files: bool,
    /// Offer skills to the AIs.
    pub skills_enabled: bool,
    /// Also the project's skills (`.orchestrator/skills`, `.claude/skills`).
    pub project_skills: bool,
    /// Also the user's Claude Code skills (`~/.claude/skills`).
    pub claude_user_skills: bool,
    /// Skills turned off, by name.
    pub disabled_skills: Vec<String>,
}

impl Default for GuidanceSettings {
    fn default() -> Self {
        Self {
            rules_enabled: true,
            project_rule_files: true,
            skills_enabled: true,
            project_skills: true,
            claude_user_skills: true,
            disabled_skills: Vec::new(),
        }
    }
}

/// Where a skill comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillSource {
    /// The Orchestrator's own: editable here.
    Orchestrator,
    /// The open project's folder.
    Project,
    /// `~/.claude/skills`.
    ClaudeUser,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    /// The `SKILL.md`.
    pub path: String,
    pub enabled: bool,
    /// Another skill with the same name comes first.
    pub shadowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDoc {
    #[serde(flatten)]
    pub info: SkillInfo,
    /// The instructions, without the front matter.
    pub body: String,
    /// Other files in the skill's folder, relative to it.
    pub files: Vec<String>,
}

/// A project instruction file found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleFile {
    pub name: String,
    pub path: String,
    pub chars: usize,
    /// Longer than what is sent.
    pub truncated: bool,
}

/// Everything the settings page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuidanceView {
    pub settings: GuidanceSettings,
    pub user_rules: String,
    /// Where the user's rules and skills are kept.
    pub rules_path: String,
    pub skills_dir: String,
    /// The open project's instruction files.
    pub project_files: Vec<RuleFile>,
    pub skills: Vec<SkillInfo>,
    pub project_path: Option<String>,
}

/// A skill the user writes or edits here.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInput {
    pub name: String,
    pub description: String,
    pub body: String,
    /// The name it had, when renaming.
    #[serde(default)]
    pub previous_name: Option<String>,
}

pub struct GuidanceService {
    data_dir: PathBuf,
    home: Option<PathBuf>,
    settings: RwLock<GuidanceSettings>,
}

impl GuidanceService {
    /// Loads `guidance.json`; a broken file falls back to the defaults
    /// with a warning.
    pub fn new(data_dir: &Path) -> (Self, Option<String>) {
        let path = data_dir.join("guidance.json");
        let (settings, warning) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(settings) => (settings, None),
                Err(err) => (
                    GuidanceSettings::default(),
                    Some(format!(
                        "{} inválido ({err}); usando o padrão",
                        path.display()
                    )),
                ),
            },
            Err(_) => (GuidanceSettings::default(), None),
        };
        let service = Self {
            data_dir: data_dir.to_path_buf(),
            home: home_dir(),
            settings: RwLock::new(settings),
        };
        (service, warning)
    }

    /// Where `~/.claude/skills` is looked for (tests).
    pub fn with_home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }

    pub fn settings(&self) -> GuidanceSettings {
        self.settings.read().clone()
    }

    pub fn save_settings(&self, settings: GuidanceSettings) -> Result<GuidanceSettings, String> {
        write_json(&self.data_dir.join("guidance.json"), &settings)?;
        *self.settings.write() = settings.clone();
        Ok(settings)
    }

    fn rules_path(&self) -> PathBuf {
        self.data_dir.join("rules.md")
    }

    fn skills_dir(&self) -> PathBuf {
        self.data_dir.join("skills")
    }

    pub fn user_rules(&self) -> String {
        std::fs::read_to_string(self.rules_path()).unwrap_or_default()
    }

    pub fn save_user_rules(&self, text: &str) -> Result<(), String> {
        if text.chars().count() > MAX_USER_RULES {
            return Err(format!("as regras passam de {MAX_USER_RULES} caracteres"));
        }
        std::fs::create_dir_all(&self.data_dir).map_err(|e| e.to_string())?;
        std::fs::write(self.rules_path(), text).map_err(|e| e.to_string())
    }

    pub fn view(&self, project: Option<&Path>) -> GuidanceView {
        GuidanceView {
            settings: self.settings(),
            user_rules: self.user_rules(),
            rules_path: self.rules_path().display().to_string(),
            skills_dir: self.skills_dir().display().to_string(),
            project_files: project.map(project_rule_files).unwrap_or_default(),
            skills: self.skills(project),
            project_path: project.map(|p| p.display().to_string()),
        }
    }

    /// Every skill found, the Orchestrator's first; a later one with the
    /// same name is `shadowed`.
    pub fn skills(&self, project: Option<&Path>) -> Vec<SkillInfo> {
        let settings = self.settings();
        let mut roots = vec![(self.skills_dir(), SkillSource::Orchestrator)];
        if let Some(project) = project {
            roots.push((project.join(".orchestrator/skills"), SkillSource::Project));
            roots.push((project.join(".claude/skills"), SkillSource::Project));
        }
        if let Some(home) = &self.home {
            roots.push((home.join(".claude/skills"), SkillSource::ClaudeUser));
        }
        let mut found: Vec<SkillInfo> = Vec::new();
        for (root, source) in roots {
            for path in skill_files(&root) {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let folder = path
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let (meta, _) = split_front_matter(&text);
                let name = meta.name.unwrap_or(folder);
                let shadowed = found.iter().any(|s| s.name == name);
                let source_on = match source {
                    SkillSource::Orchestrator => true,
                    SkillSource::Project => settings.project_skills,
                    SkillSource::ClaudeUser => settings.claude_user_skills,
                };
                found.push(SkillInfo {
                    enabled: source_on && !settings.disabled_skills.contains(&name),
                    description: meta.description.unwrap_or_default(),
                    name,
                    source,
                    path: path.display().to_string(),
                    shadowed,
                });
            }
        }
        found
    }

    /// The skills an AI is offered: enabled, not shadowed.
    fn offered(&self, project: Option<&Path>) -> Vec<SkillInfo> {
        if !self.settings().skills_enabled {
            return Vec::new();
        }
        self.skills(project)
            .into_iter()
            .filter(|s| s.enabled && !s.shadowed)
            .collect()
    }

    /// One skill by name (the first with it), whole.
    pub fn skill(&self, name: &str, project: Option<&Path>) -> Option<SkillDoc> {
        let info = self
            .skills(project)
            .into_iter()
            .find(|s| s.name == name && !s.shadowed)?;
        let path = PathBuf::from(&info.path);
        let text = std::fs::read_to_string(&path).ok()?;
        let (_, body) = split_front_matter(&text);
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        Some(SkillDoc {
            files: folder_files(&folder),
            body: body.to_owned(),
            info,
        })
    }

    /// Writes one of the Orchestrator's skills (`<data>/skills/<name>`).
    pub fn save_skill(&self, input: &SkillInput) -> Result<SkillInfo, String> {
        let name = input.name.trim();
        validate_skill_name(name)?;
        let description = one_line(&input.description);
        if description.is_empty() {
            return Err("a descrição diz às IAs quando usar a skill: preencha".into());
        }
        let dir = self.skills_dir().join(name);
        if let Some(previous) = input.previous_name.as_deref().filter(|p| *p != name) {
            validate_skill_name(previous)?;
            if dir.exists() {
                return Err(format!("já existe uma skill {name}"));
            }
            let old = self.skills_dir().join(previous);
            if old.exists() {
                std::fs::rename(&old, &dir).map_err(|e| e.to_string())?;
            }
        }
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let text = format!(
            "---\nname: {name}\ndescription: {description}\n---\n\n{}\n",
            input.body.trim_end()
        );
        let path = dir.join("SKILL.md");
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        let settings = self.settings();
        Ok(SkillInfo {
            name: name.to_owned(),
            description,
            source: SkillSource::Orchestrator,
            path: path.display().to_string(),
            enabled: !settings.disabled_skills.iter().any(|d| d == name),
            shadowed: false,
        })
    }

    /// Removes one of the Orchestrator's skills (folder and all).
    pub fn delete_skill(&self, name: &str) -> Result<(), String> {
        validate_skill_name(name)?;
        let dir = self.skills_dir().join(name);
        if !dir.join("SKILL.md").is_file() {
            return Err(format!("a skill {name} não é do Orchestrator"));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())
    }

    pub fn set_skill_enabled(&self, name: &str, enabled: bool) -> Result<GuidanceSettings, String> {
        let mut settings = self.settings();
        settings.disabled_skills.retain(|d| d != name);
        if !enabled {
            settings.disabled_skills.push(name.to_owned());
            settings.disabled_skills.sort();
        }
        self.save_settings(settings)
    }

    /// The text that goes into a session's instructions, with what it
    /// carries. `tools`: the session can call `skill.read`.
    pub fn instructions(
        &self,
        project: Option<&Path>,
        tools: bool,
    ) -> Option<(String, Vec<ContextSectionSummary>)> {
        let settings = self.settings();
        let mut text = String::new();
        let mut sections = Vec::new();

        if settings.rules_enabled {
            let mut parts = Vec::new();
            let user = self.user_rules();
            if !user.trim().is_empty() {
                parts.push(format!(
                    "### The user's rules (every project)\n{}",
                    user.trim()
                ));
            }
            if settings.project_rule_files {
                if let Some(project) = project {
                    for file in project_rule_files(project) {
                        let Ok(content) = std::fs::read_to_string(&file.path) else {
                            continue;
                        };
                        if content.trim().is_empty() {
                            continue;
                        }
                        let mut body = clip(content.trim(), MAX_RULE_FILE);
                        if file.truncated {
                            body.push_str("\n[… cut: read the file for the rest]");
                        }
                        parts.push(format!("### {} (project)\n{body}", file.name));
                    }
                }
            }
            if !parts.is_empty() {
                let body = format!(
                    "## DEVELOPMENT RULES\nRules the user set for this work. Follow them in everything you do; \
                     when one conflicts with a request, say so.\n\n{}",
                    parts.join("\n\n")
                );
                sections.push(ContextSectionSummary {
                    kind: "rules".into(),
                    title: "Regras de desenvolvimento".into(),
                    items: parts.len() as u32,
                    tokens: estimate(&body),
                });
                text.push_str(&body);
            }
        }

        let skills = if tools {
            self.offered(project)
        } else {
            Vec::new()
        };
        if !skills.is_empty() {
            let list: Vec<String> = skills
                .iter()
                .map(|s| format!("- {}: {}", s.name, one_line(&s.description)))
                .collect();
            let body = format!(
                "## SKILLS\nSkills are instructions for specific kinds of work. When the task matches a skill's \
                 description, call skill.read with its name before starting, and follow it.\n{}",
                list.join("\n")
            );
            sections.push(ContextSectionSummary {
                kind: "skills".into(),
                title: "Skills".into(),
                items: skills.len() as u32,
                tokens: estimate(&body),
            });
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&body);
        }
        (!text.is_empty()).then_some((text, sections))
    }
}

// ------------------------------------------------------------- context ---

/// The project context plus the rules and skills: what a session gets on
/// its first turn. Rules and skills come even with the project context
/// turned off.
pub struct GuidedContext {
    inner: Arc<dyn ContextSource>,
    guidance: Arc<GuidanceService>,
}

impl GuidedContext {
    pub fn new(inner: Arc<dyn ContextSource>, guidance: Arc<GuidanceService>) -> Self {
        Self { inner, guidance }
    }
}

#[async_trait]
impl ContextSource for GuidedContext {
    async fn build(&self, request: ContextRequest) -> Result<Option<AttachedContext>, String> {
        let project = request.session.project_path.clone();
        let tools = request.tools;
        let guidance = self.guidance.clone();
        let extra =
            tokio::task::spawn_blocking(move || guidance.instructions(Some(&project), tools))
                .await
                .map_err(|e| format!("rules task failed: {e}"))?;
        let context = self.inner.build(request).await?;
        let Some((text, sections)) = extra else {
            return Ok(context);
        };
        let tokens: u32 = sections.iter().map(|s| s.tokens).sum();
        Ok(Some(match context {
            Some(mut context) => {
                context.text = format!("{text}\n\n{}", context.text);
                context.summary.tokens += tokens;
                let mut all = sections;
                all.append(&mut context.summary.sections);
                context.summary.sections = all;
                context
            }
            None => AttachedContext {
                text,
                summary: ContextSummary {
                    tokens,
                    budget: tokens,
                    sections,
                    ..Default::default()
                },
            },
        }))
    }
}

// --------------------------------------------------------------- tools ---

const SKILL_LIST: &str = "skill.list";
const SKILL_READ: &str = "skill.read";

fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: SKILL_LIST.into(),
            group: "skill".into(),
            description: "Lists the skills available (name, description, where it comes from). A skill holds \
                          instructions for a kind of work; read one with skill.read when the task matches."
                .into(),
            read_only: true,
            parameters: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        },
        ToolDefinition {
            name: SKILL_READ.into(),
            group: "skill".into(),
            description: "Reads a skill: its instructions and the other files in its folder (read those with \
                          filesystem.read at the folder shown). Follow the instructions for the task."
                .into(),
            read_only: true,
            parameters: json!({
                "type": "object",
                "properties": {"name": {"type": "string", "description": "Skill name, as listed."}},
                "required": ["name"],
                "additionalProperties": false
            }),
        },
    ]
}

/// The app's tools plus `skill.list` and `skill.read`.
pub struct SkillTools {
    inner: Arc<dyn ToolExecutor>,
    guidance: Arc<GuidanceService>,
    store: Arc<MemoryStore>,
    sink: Arc<dyn EventSink>,
}

impl SkillTools {
    pub fn new(
        inner: Arc<dyn ToolExecutor>,
        guidance: Arc<GuidanceService>,
        store: Arc<MemoryStore>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            inner,
            guidance,
            store,
            sink,
        }
    }

    /// The project of the session that called, or the open one.
    fn project(&self, origin: &CallOrigin) -> Option<PathBuf> {
        let id = match origin {
            CallOrigin::Agent {
                session_id: Some(id),
                ..
            } => self.store.session_project_id(id.as_str()),
            _ => None,
        };
        id.and_then(|id| self.store.project(&id))
            .or_else(|| self.store.current_project())
            .map(|p| PathBuf::from(p.path))
    }

    fn run(&self, call: &ToolCall) -> Result<Value, ToolError> {
        let project = self.project(&call.origin);
        let offered = |name: &str| {
            self.guidance
                .offered(project.as_deref())
                .into_iter()
                .any(|s| s.name == name)
        };
        match call.tool.as_str() {
            SKILL_LIST => Ok(json!({
                "skills": self.guidance.offered(project.as_deref()).iter().map(|s| json!({
                    "name": s.name,
                    "description": s.description,
                    "source": s.source,
                })).collect::<Vec<_>>()
            })),
            _ => {
                let name = call
                    .args
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .ok_or_else(|| ToolError::invalid_args("name is required"))?;
                let doc = self
                    .guidance
                    .skill(name, project.as_deref())
                    .filter(|_| offered(name))
                    .ok_or_else(|| {
                        ToolError::not_found(format!("no skill named {name} (see skill.list)"))
                    })?;
                let folder = Path::new(&doc.info.path)
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let mut body = clip(&doc.body, MAX_SKILL_BODY);
                if doc.body.chars().count() > MAX_SKILL_BODY {
                    body.push_str("\n[… cut: read SKILL.md for the rest]");
                }
                Ok(json!({
                    "name": doc.info.name,
                    "description": doc.info.description,
                    "folder": folder,
                    "instructions": body,
                    "files": doc.files,
                }))
            }
        }
    }
}

#[async_trait]
impl ToolExecutor for SkillTools {
    fn tools(&self) -> Vec<ToolDefinition> {
        let mut tools = self.inner.tools();
        tools.extend(definitions());
        tools
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        self.execute_with(call, Default::default()).await
    }

    async fn execute_with(
        &self,
        call: ToolCall,
        cancel: tokio_util::sync::CancellationToken,
    ) -> ToolResult {
        if call.tool != SKILL_LIST && call.tool != SKILL_READ {
            return self.inner.execute_with(call, cancel).await;
        }
        let started_at = Utc::now();
        let clock = Instant::now();
        let outcome = self.run(&call);
        let duration_ms = clock.elapsed().as_millis() as u64;
        let (ok, output, error) = match outcome {
            Ok(output) => (true, output, None),
            Err(error) => (false, Value::Null, Some(error)),
        };
        let target = call
            .args
            .get("name")
            .and_then(Value::as_str)
            .map(|n| format!(" {}", clip(n, 80)))
            .unwrap_or_default();
        self.sink.audit(
            AuditEvent::new(
                EventKind::ToolCalled,
                call.origin.clone(),
                match &error {
                    None => format!("{}{target}", call.tool),
                    Some(err) => {
                        format!("{}{target} failed: {}", call.tool, clip(&err.message, 200))
                    }
                },
                json!({
                    "tool": call.tool,
                    "readOnly": true,
                    "args": call.args,
                    "ok": ok,
                    "error": error,
                    "durationMs": duration_ms,
                }),
            )
            .with_call(call.id.clone()),
        );
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

// ------------------------------------------------------------- helpers ---

#[derive(Default)]
struct FrontMatter {
    name: Option<String>,
    description: Option<String>,
}

/// `---\nkey: value\n---\nbody`: the keys this needs, and the body.
/// Values may be quoted; `>`/`|` blocks take the indented lines below.
fn split_front_matter(text: &str) -> (FrontMatter, &str) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut meta = FrontMatter::default();
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (meta, text);
    };
    let Some(end) = rest.find("\n---") else {
        return (meta, text);
    };
    let header = &rest[..end];
    let after = &rest[end + 4..];
    let body = after
        .split_once('\n')
        .map_or("", |(_, body)| body)
        .trim_start_matches(['\r', '\n']);
    let lines: Vec<&str> = header.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let mut value = value.trim().to_owned();
        if value == ">" || value == "|" || value == ">-" || value == "|-" {
            let mut block = Vec::new();
            while i < lines.len()
                && (lines[i].starts_with([' ', '\t']) || lines[i].trim().is_empty())
            {
                block.push(lines[i].trim());
                i += 1;
            }
            value = block.join(" ").trim().to_owned();
        }
        let value = unquote(&value);
        match key.trim() {
            "name" => meta.name = Some(value).filter(|v| !v.is_empty()),
            "description" => meta.description = Some(value).filter(|v| !v.is_empty()),
            _ => {}
        }
    }
    (meta, body)
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return v[1..v.len() - 1].to_owned();
        }
    }
    v.to_owned()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn estimate(text: &str) -> u32 {
    (text.chars().count() / 4) as u32 + 1
}

fn validate_skill_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && !name.starts_with(['-', '_']);
    if ok {
        Ok(())
    } else {
        Err("nome da skill: letras minúsculas, números, - e _ (até 64)".into())
    }
}

/// `<root>/<folder>/SKILL.md`, sorted by folder.
fn skill_files(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path().join("SKILL.md"))
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    files
}

/// Files under a skill's folder besides SKILL.md, relative, sorted.
fn folder_files(folder: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(folder) {
                let relative = relative.to_string_lossy().replace('\\', "/");
                if relative != "SKILL.md" {
                    out.push(relative);
                }
            }
            if out.len() >= MAX_SKILL_FILES {
                break;
            }
        }
    }
    out.sort();
    out
}

/// The project's instruction files that exist.
pub fn project_rule_files(project: &Path) -> Vec<RuleFile> {
    PROJECT_RULE_FILES
        .iter()
        .filter_map(|name| {
            let path = project.join(name);
            let chars = std::fs::read_to_string(&path).ok()?.chars().count();
            Some(RuleFile {
                name: (*name).to_owned(),
                path: path.display().to_string(),
                chars,
                truncated: chars > MAX_RULE_FILE,
            })
        })
        .collect()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn front_matter_in_its_common_shapes() {
        let (meta, body) =
            split_front_matter("---\nname: pdf\ndescription: \"Lê PDFs\"\n---\n\n# Corpo\n");
        assert_eq!(meta.name.as_deref(), Some("pdf"));
        assert_eq!(meta.description.as_deref(), Some("Lê PDFs"));
        assert_eq!(body, "# Corpo\n");
        let (meta, _) = split_front_matter(
            "---\r\nname: x\r\ndescription: >\r\n  linha um\r\n  linha dois\r\nlicense: MIT\r\n---\r\nb",
        );
        assert_eq!(meta.description.as_deref(), Some("linha um linha dois"));
        let (meta, body) = split_front_matter("# sem cabeçalho");
        assert!(meta.name.is_none());
        assert_eq!(body, "# sem cabeçalho");
    }

    #[test]
    fn rules_and_skills_reach_the_instructions() {
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let (service, warning) = GuidanceService::new(data.path());
        assert!(warning.is_none());
        let service = service.with_home(Some(home.path().to_path_buf()));
        assert!(service.instructions(Some(project.path()), true).is_none());

        service.save_user_rules("Sempre escreva testes.").unwrap();
        write(&project.path().join("AGENTS.md"), "Use pnpm, nunca npm.");
        service
            .save_skill(&SkillInput {
                name: "revisao".into(),
                description: "Revisar código antes do commit".into(),
                body: "1. Rode os testes".into(),
                previous_name: None,
            })
            .unwrap();
        write(
            &project.path().join(".claude/skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Publicar o app\n---\nPassos",
        );
        write(
            &project.path().join(".orchestrator/skills/revisao/SKILL.md"),
            "---\nname: revisao\ndescription: outra\n---\n",
        );
        write(
            &home.path().join(".claude/skills/notas/SKILL.md"),
            "---\nname: notas\ndescription: Notas pessoais\n---\n",
        );

        let (text, sections) = service.instructions(Some(project.path()), true).unwrap();
        assert!(text.contains("Sempre escreva testes."));
        assert!(text.contains("### AGENTS.md (project)\nUse pnpm, nunca npm."));
        assert!(text.contains("- revisao: Revisar código antes do commit"));
        assert!(text.contains("- deploy: Publicar o app"));
        assert!(text.contains("- notas: Notas pessoais"));
        assert!(!text.contains("outra"), "the project's revisao is shadowed");
        assert_eq!(sections.len(), 2);
        // Without tools, no skills (they could not be read).
        let (text, _) = service.instructions(Some(project.path()), false).unwrap();
        assert!(!text.contains("SKILLS"));

        // Turned off: one skill, the project's files, the Claude Code ones.
        service.set_skill_enabled("deploy", false).unwrap();
        let mut settings = service.settings();
        settings.project_rule_files = false;
        settings.claude_user_skills = false;
        service.save_settings(settings).unwrap();
        let (text, _) = service.instructions(Some(project.path()), true).unwrap();
        assert!(!text.contains("deploy") && !text.contains("pnpm") && !text.contains("notas"));
        assert!(text.contains("revisao"));

        // Settings survive a restart.
        let (again, _) = GuidanceService::new(data.path());
        assert_eq!(again.settings().disabled_skills, vec!["deploy".to_owned()]);

        // Renaming and deleting the Orchestrator's own.
        service
            .save_skill(&SkillInput {
                name: "review".into(),
                description: "Revisar".into(),
                body: "corpo".into(),
                previous_name: Some("revisao".into()),
            })
            .unwrap();
        let doc = service.skill("review", Some(project.path())).unwrap();
        assert_eq!(doc.body, "corpo\n");
        assert!(
            service.delete_skill("deploy").is_err(),
            "not the Orchestrator's"
        );
        service.delete_skill("review").unwrap();
        assert!(service
            .save_skill(&SkillInput {
                name: "../fora".into(),
                description: "x".into(),
                body: String::new(),
                previous_name: None,
            })
            .is_err());
    }
}
