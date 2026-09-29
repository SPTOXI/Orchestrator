//! The Context Builder (ADR-0013): for each session, the part of the
//! project's memory that matters for its task, within a token budget.
//!
//! Sections, in the master document's order: task, working memory, project
//! memory, relevant files, recent errors, relevant history, Git state and
//! handoff. Chosen by rules and full-text search (no AI call), with paths
//! but never file contents, and never the whole history.

use crate::packet;
use crate::settings::{self, ContextSettings, MAX_BUDGET, MIN_BUDGET};
use crate::text::{estimate_tokens, is_inner_relative, line, relative, when};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use orchestrator_core::{ContextSectionSummary, ContextSummary, Handoff, SessionStatus};
use orchestrator_git::Git;
use orchestrator_memory::{MemoryStore, Project};
use orchestrator_providers::{AttachedContext, ContextRequest, ContextSource};
use parking_lot::RwLock;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Kinds of section, in the master document's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SectionKind {
    Task,
    Working,
    Project,
    Files,
    Errors,
    History,
    Git,
    Handoff,
}

impl SectionKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Task => "TASK",
            Self::Working => "WORKING MEMORY",
            Self::Project => "PROJECT MEMORY",
            Self::Files => "RELEVANT FILES",
            Self::Errors => "RECENT ERRORS",
            Self::History => "RELEVANT HISTORY",
            Self::Git => "GIT STATE",
            Self::Handoff => "HANDOFF",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Working => "working",
            Self::Project => "project",
            Self::Files => "files",
            Self::Errors => "errors",
            Self::History => "history",
            Self::Git => "git",
            Self::Handoff => "handoff",
        }
    }
}

/// Items that are never cut by the budget.
const KEEP: u8 = u8::MAX;

/// Trimming order (lowest first): history, files, errors, other L2, L1,
/// pinned L2, Git. Task and handoff are kept.
mod priority {
    pub const HISTORY: u8 = 1;
    pub const FILES: u8 = 2;
    pub const ERRORS: u8 = 3;
    pub const PROJECT: u8 = 4;
    pub const WORKING: u8 = 5;
    pub const PINNED: u8 = 6;
    pub const GIT: u8 = 7;
}

// Most items per section (before the budget).
const SESSIONS: usize = 4;
const COMMANDS: usize = 5;
const PINNED: usize = 6;
const RELATED_MEMORY: usize = 4;
const ACCEPTED_DECISIONS: usize = 3;
const RELATED_DECISIONS: usize = 3;
const MENTIONED_FILES: usize = 5;
const CHANGED_FILES: usize = 6;
const ERRORS: usize = 4;
const ERROR_DAYS: i64 = 7;
const HISTORY: usize = 4;
const GIT_FILES: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    text: String,
    priority: u8,
    /// Written as is (no `- ` bullet).
    plain: bool,
}

impl Item {
    fn bullet(text: impl Into<String>, priority: u8) -> Self {
        Self {
            text: text.into(),
            priority,
            plain: false,
        }
    }

    fn plain(text: impl Into<String>, priority: u8) -> Self {
        Self {
            text: text.into(),
            priority,
            plain: true,
        }
    }
}

/// One section of a pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSection {
    pub kind: SectionKind,
    pub title: String,
    /// Lines sent (after the budget).
    pub items: Vec<String>,
    /// Items found before the budget.
    pub found: usize,
    pub tokens: u32,
}

/// What would be (or was) sent.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPack {
    pub project: Option<Project>,
    pub sections: Vec<ContextSection>,
    /// Estimated tokens of `text`.
    pub tokens: u32,
    pub budget: u32,
    /// Left out, and why (e.g. "RELEVANT HISTORY: 2 itens (orçamento)").
    pub omitted: Vec<String>,
    /// Information about the pack (e.g. no Git repository).
    pub notes: Vec<String>,
    pub text: String,
}

impl ContextPack {
    pub fn summary(&self) -> ContextSummary {
        ContextSummary {
            tokens: self.tokens,
            budget: self.budget,
            sections: self
                .sections
                .iter()
                .map(|s| ContextSectionSummary {
                    kind: s.kind.id().into(),
                    title: s.title.clone(),
                    items: s.items.len() as u32,
                    tokens: s.tokens,
                })
                .collect(),
            omitted: self.omitted.clone(),
            handoff_id: None,
        }
    }
}

/// What to build a pack for.
#[derive(Debug, Clone, Default)]
pub struct BuildRequest {
    pub project_path: PathBuf,
    /// Text the relevance comes from (the first message, or the handoff's
    /// goal and next action). The message itself is not repeated.
    pub task: String,
    pub handoff: Option<Handoff>,
    /// Estimated tokens; `None` = the setting.
    pub budget: Option<u32>,
    /// The session receiving the context (left out of WORKING MEMORY and
    /// RELEVANT HISTORY).
    pub session_id: Option<String>,
    /// The session cannot ask for tools: the text does not point to them.
    pub no_tools: bool,
}

struct Draft {
    kind: SectionKind,
    items: Vec<Item>,
    found: usize,
    removed: usize,
}

pub struct ContextBuilder {
    store: Arc<MemoryStore>,
    git: Option<Git>,
    settings: RwLock<ContextSettings>,
    settings_path: Option<PathBuf>,
}

impl ContextBuilder {
    /// A builder with the settings at `settings_path` (`None`: defaults, not
    /// saved) and the system `git`, if installed. Returns a warning when the
    /// settings file was ignored.
    pub fn new(store: Arc<MemoryStore>, settings_path: Option<PathBuf>) -> (Self, Option<String>) {
        let (settings, warning) = match &settings_path {
            Some(path) => settings::load(path),
            None => (ContextSettings::default(), None),
        };
        (
            Self {
                store,
                git: Git::detect().ok(),
                settings: RwLock::new(settings),
                settings_path,
            },
            warning,
        )
    }

    /// Without Git (tests, or when it should not be consulted).
    pub fn without_git(mut self) -> Self {
        self.git = None;
        self
    }

    pub fn store(&self) -> &Arc<MemoryStore> {
        &self.store
    }

    pub fn settings(&self) -> ContextSettings {
        *self.settings.read()
    }

    /// Validates, saves (when there is a file) and applies the settings.
    pub fn save_settings(&self, settings: ContextSettings) -> Result<ContextSettings, String> {
        settings.validate()?;
        if let Some(path) = &self.settings_path {
            settings::save(path, &settings)?;
        }
        *self.settings.write() = settings;
        Ok(settings)
    }

    /// Builds the pack. Never fails: what cannot be read becomes a note.
    pub fn build(&self, request: &BuildRequest) -> ContextPack {
        let budget = request
            .budget
            .unwrap_or_else(|| self.settings().budget_tokens)
            .clamp(MIN_BUDGET, MAX_BUDGET);
        let root = request.project_path.as_path();
        let project = self
            .store
            .project_by_path(&root.to_string_lossy())
            .or_else(|| {
                // Sessions record the path as opened; accept a trailing
                // separator difference.
                let trimmed = root
                    .to_string_lossy()
                    .trim_end_matches(['/', '\\'])
                    .to_owned();
                self.store.project_by_path(&trimmed)
            });
        let mut notes = Vec::new();
        let mut drafts = Vec::new();

        drafts.push(Draft::with(
            SectionKind::Task,
            vec![Item::plain(
                match &request.handoff {
                    Some(_) => {
                        "Take over the work described in HANDOFF below; start with its NEXT ACTION."
                    }
                    None => "The task is the user's first message in this conversation.",
                },
                KEEP,
            )],
        ));

        let git_paths = match &self.git {
            Some(git) => match git.status(root) {
                Ok(status) => {
                    let paths: HashSet<String> =
                        status.files.iter().map(|f| f.path.clone()).collect();
                    drafts.push(git_section(&status));
                    paths
                }
                Err(_) => {
                    notes.push("GIT STATE: a pasta não é um repositório Git".into());
                    HashSet::new()
                }
            },
            None => {
                notes.push("GIT STATE: Git indisponível".into());
                HashSet::new()
            }
        };

        match &project {
            Some(project) => {
                let task = task_text(request);
                drafts.push(self.working_section(&project.id, request.session_id.as_deref()));
                drafts.push(self.project_section(&project.id, &task));
                drafts.push(self.files_section(&project.id, root, &task, &git_paths));
                drafts.push(self.errors_section(&project.id));
                drafts.push(self.history_section(&project.id, &task, request));
            }
            None => notes.push(
                "Projeto sem memória registrada: só tarefa, Git e handoff entram no contexto"
                    .into(),
            ),
        }

        if let Some(handoff) = &request.handoff {
            drafts.push(Draft::with(
                SectionKind::Handoff,
                vec![Item::plain(packet::render(handoff), KEEP)],
            ));
        }

        drafts.sort_by_key(|d| order(d.kind));
        drafts.retain(|d| !d.items.is_empty());
        let header = header(project.as_ref(), root, !request.no_tools);
        trim(&mut drafts, &header, budget);

        let omitted = drafts
            .iter()
            .filter(|d| d.removed > 0)
            .map(|d| {
                format!(
                    "{}: {} {} (orçamento)",
                    d.kind.title(),
                    d.removed,
                    if d.removed == 1 { "item" } else { "itens" }
                )
            })
            .collect();
        drafts.retain(|d| !d.items.is_empty());
        let text = render(&header, &drafts);
        let sections = drafts
            .iter()
            .map(|d| ContextSection {
                kind: d.kind,
                title: d.kind.title().into(),
                items: d.items.iter().map(|i| i.text.clone()).collect(),
                found: d.found,
                tokens: estimate_tokens(&render_section(d)),
            })
            .collect();
        ContextPack {
            project,
            sections,
            tokens: estimate_tokens(&text),
            budget,
            omitted,
            notes,
            text,
        }
    }

    fn working_section(&self, project_id: &str, session_id: Option<&str>) -> Draft {
        let mut items = Vec::new();
        if let Ok(working) = self.store.working_memory(project_id) {
            for session in working
                .sessions
                .iter()
                .filter(|s| Some(s.id.as_str()) != session_id)
                .take(SESSIONS)
            {
                let status = match session.status {
                    SessionStatus::Running => "running",
                    SessionStatus::Idle => "open",
                    SessionStatus::Closed => "closed",
                };
                items.push(Item::bullet(
                    format!(
                        "Session \"{}\" ({}{}): {status}, {} turn{}, last activity {}",
                        line(&session.title, 80),
                        session.provider,
                        session
                            .model
                            .as_deref()
                            .map(|m| format!("/{m}"))
                            .unwrap_or_default(),
                        session.turns,
                        if session.turns == 1 { "" } else { "s" },
                        when(&session.updated_at)
                    ),
                    priority::WORKING,
                ));
            }
            for command in working.commands.iter().take(COMMANDS) {
                let outcome = if command.background {
                    "background".to_owned()
                } else {
                    match command.exit_code {
                        Some(code) => format!("exit {code}"),
                        None => "no exit code".to_owned(),
                    }
                };
                items.push(Item::bullet(
                    format!(
                        "$ {} → {outcome} ({}, {})",
                        line(&command.command, 160),
                        by(&command.by),
                        when(&command.at)
                    ),
                    priority::WORKING,
                ));
            }
        }
        Draft::with(SectionKind::Working, items)
    }

    fn project_section(&self, project_id: &str, task: &str) -> Draft {
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        let entries = self.store.memory_list(project_id).unwrap_or_default();
        for entry in entries.iter().filter(|e| e.pinned).take(PINNED) {
            seen.insert(entry.id.clone());
            items.push(Item::bullet(
                format!(
                    "[{}, pinned] {}: {}",
                    kind_label(entry.kind),
                    entry.title,
                    line(&entry.content.replace('\n', "; "), 400)
                ),
                priority::PINNED,
            ));
        }
        let related = self
            .store
            .search_related(project_id, task, &["memory"], RELATED_MEMORY * 2)
            .unwrap_or_default();
        for hit in related {
            if items.len() >= PINNED + RELATED_MEMORY || !seen.insert(hit.ref_id.clone()) {
                continue;
            }
            if let Some(entry) = entries.iter().find(|e| e.id == hit.ref_id) {
                items.push(Item::bullet(
                    format!(
                        "[{}] {}: {}",
                        kind_label(entry.kind),
                        entry.title,
                        line(&entry.content.replace('\n', "; "), 400)
                    ),
                    priority::PROJECT,
                ));
            }
        }

        let decisions = self.store.decisions_list(project_id).unwrap_or_default();
        let mut chosen: Vec<&orchestrator_memory::Decision> = Vec::new();
        for hit in self
            .store
            .search_related(project_id, task, &["decision"], RELATED_DECISIONS)
            .unwrap_or_default()
        {
            if let Some(decision) = decisions.iter().find(|d| d.id == hit.ref_id) {
                chosen.push(decision);
            }
        }
        for decision in decisions
            .iter()
            .filter(|d| d.status == orchestrator_memory::DecisionStatus::Accepted)
            .take(ACCEPTED_DECISIONS)
        {
            if !chosen.iter().any(|c| c.id == decision.id) {
                chosen.push(decision);
            }
        }
        for decision in chosen {
            items.push(Item::bullet(
                format!(
                    "[decision, {}] {}: {}",
                    decision_status_en(decision.status),
                    decision.title,
                    line(&decision.decision, 300)
                ),
                priority::PROJECT,
            ));
        }
        Draft::with(SectionKind::Project, items)
    }

    fn files_section(
        &self,
        project_id: &str,
        root: &Path,
        task: &str,
        git_paths: &HashSet<String>,
    ) -> Draft {
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        for path in mentioned_paths(task, root)
            .into_iter()
            .take(MENTIONED_FILES)
        {
            seen.insert(path.clone());
            items.push(Item::bullet(
                format!("{path} (mentioned in the task)"),
                priority::FILES,
            ));
        }
        if let Ok(working) = self.store.working_memory(project_id) {
            for file in working.files.iter().take(CHANGED_FILES * 2) {
                let path = relative(&file.path, root);
                if git_paths.contains(&path) || !seen.insert(path.clone()) {
                    continue;
                }
                if items.len() >= MENTIONED_FILES + CHANGED_FILES {
                    break;
                }
                items.push(Item::bullet(
                    format!(
                        "{path} ({} by {}, {})",
                        file.change,
                        by(&file.by),
                        when(&file.at)
                    ),
                    priority::FILES,
                ));
            }
        }
        Draft::with(SectionKind::Files, items)
    }

    fn errors_section(&self, project_id: &str) -> Draft {
        let since = Utc::now() - Duration::days(ERROR_DAYS);
        let items = self
            .store
            .working_memory(project_id)
            .map(|w| w.errors)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.at >= since)
            .take(ERRORS)
            .map(|e| {
                let detail = e
                    .detail
                    .as_deref()
                    .map(|d| format!(": {}", line(d, 200)))
                    .unwrap_or_default();
                Item::bullet(
                    format!("{}{detail} ({})", line(&e.summary, 160), when(&e.at)),
                    priority::ERRORS,
                )
            })
            .collect();
        Draft::with(SectionKind::Errors, items)
    }

    fn history_section(&self, project_id: &str, task: &str, request: &BuildRequest) -> Draft {
        // In a handoff, the packet stands for the source session: none of
        // its messages, nor the handoff itself, come back as history.
        let skip_handoff = request.handoff.as_ref().map(|h| h.id.to_string());
        let skip_source = request
            .handoff
            .as_ref()
            .map(|h| h.from.session_id.to_string());
        // Failures already in RECENT ERRORS are not repeated as history.
        let recent_errors: HashSet<String> = self
            .store
            .working_memory(project_id)
            .map(|w| w.errors.into_iter().map(|e| e.summary).collect())
            .unwrap_or_default();
        let items = self
            .store
            .search_related(
                project_id,
                task,
                &["message", "event", "handoff"],
                HISTORY * 3,
            )
            .unwrap_or_default()
            .into_iter()
            .filter(|hit| {
                Some(&hit.ref_id) != skip_handoff.as_ref()
                    && Some(&hit.ref_id) != skip_source.as_ref()
                    && Some(hit.ref_id.as_str()) != request.session_id.as_deref()
                    && !(hit.kind == "event" && recent_errors.contains(&hit.title))
            })
            .take(HISTORY)
            .map(|hit| {
                let snippet = hit.snippet.replace(['[', ']'], "");
                Item::bullet(
                    format!(
                        "[{}: {}] {} ({})",
                        hit.kind,
                        line(&hit.title, 60),
                        line(&snippet, 240),
                        when(&hit.at)
                    ),
                    priority::HISTORY,
                )
            })
            .collect();
        Draft::with(SectionKind::History, items)
    }
}

impl Draft {
    fn with(kind: SectionKind, items: Vec<Item>) -> Self {
        Self {
            kind,
            found: items.len(),
            items,
            removed: 0,
        }
    }
}

fn order(kind: SectionKind) -> u8 {
    kind as u8
}

fn task_text(request: &BuildRequest) -> String {
    match &request.handoff {
        Some(handoff) => format!(
            "{} {} {}",
            handoff.packet.goal,
            handoff.packet.next_action,
            handoff.packet.remaining.join(" ")
        ),
        None => request.task.clone(),
    }
}

fn by(origin: &str) -> &str {
    match origin {
        "agent" => "AI",
        "user" => "user",
        other => other,
    }
}

fn kind_label(kind: orchestrator_memory::MemoryKind) -> &'static str {
    use orchestrator_memory::MemoryKind;
    match kind {
        MemoryKind::Architecture => "architecture",
        MemoryKind::Stack => "stack",
        MemoryKind::Convention => "convention",
        MemoryKind::Rule => "rule",
        MemoryKind::Note => "note",
    }
}

fn decision_status_en(status: orchestrator_memory::DecisionStatus) -> &'static str {
    use orchestrator_memory::DecisionStatus;
    match status {
        DecisionStatus::Proposed => "proposed",
        DecisionStatus::Accepted => "accepted",
        DecisionStatus::Superseded => "superseded",
        DecisionStatus::Rejected => "rejected",
    }
}

fn git_section(status: &orchestrator_git::Status) -> Draft {
    let mut items = Vec::new();
    let branch = match (&status.branch, status.detached) {
        (Some(branch), _) => branch.clone(),
        (None, true) => "detached HEAD".into(),
        (None, false) => "(no branch)".into(),
    };
    let upstream = match &status.upstream {
        Some(up) => format!(" → {up} (ahead {}, behind {})", status.ahead, status.behind),
        None => " (no upstream)".into(),
    };
    items.push(Item::plain(
        format!("Branch {branch}{upstream}"),
        priority::GIT,
    ));
    if status.clean {
        items.push(Item::plain("Working tree clean.", priority::GIT));
    } else {
        let staged = status.count(|f| f.staged.is_some());
        let unstaged = status.count(|f| {
            f.unstaged.is_some() && f.unstaged != Some(orchestrator_git::ChangeKind::Untracked)
        });
        let untracked =
            status.count(|f| f.unstaged == Some(orchestrator_git::ChangeKind::Untracked));
        items.push(Item::plain(
            format!(
                "{} changed file{} ({staged} staged, {unstaged} not staged, {untracked} untracked), paths from the repository root:",
                status.files.len(),
                if status.files.len() == 1 { "" } else { "s" }
            ),
            priority::GIT,
        ));
        for file in status.files.iter().take(GIT_FILES) {
            items.push(Item::bullet(
                format!("{}: {}", file.path, change_words(file)),
                priority::GIT,
            ));
        }
        if status.files.len() > GIT_FILES {
            items.push(Item::bullet(
                format!("… and {} more", status.files.len() - GIT_FILES),
                priority::GIT,
            ));
        }
    }
    Draft::with(SectionKind::Git, items)
}

/// "modified (not staged)", "added (staged)", "untracked", "conflicted".
fn change_words(file: &orchestrator_git::FileChange) -> String {
    use orchestrator_git::ChangeKind;
    let word = |kind: ChangeKind| match kind {
        ChangeKind::Modified => "modified",
        ChangeKind::Added => "added",
        ChangeKind::Deleted => "deleted",
        ChangeKind::Renamed => "renamed",
        ChangeKind::Copied => "copied",
        ChangeKind::TypeChanged => "type changed",
        ChangeKind::Untracked => "untracked",
        ChangeKind::Conflicted => "conflicted",
    };
    if file.conflicted {
        return "conflicted".into();
    }
    match (file.staged, file.unstaged) {
        (_, Some(ChangeKind::Untracked)) => "untracked".into(),
        (Some(staged), None) => format!("{} (staged)", word(staged)),
        (None, Some(unstaged)) => format!("{} (not staged)", word(unstaged)),
        (Some(staged), Some(unstaged)) => {
            format!(
                "{} (staged), then {} (not staged)",
                word(staged),
                word(unstaged)
            )
        }
        (None, None) => "changed".into(),
    }
}

/// Paths written in the task that exist inside the project.
fn mentioned_paths(task: &str, root: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in task.split(|c: char| c.is_whitespace() || "`'\"()[]{}<>,;".contains(c)) {
        let word = word.trim_end_matches(['.', ':', '!', '?']);
        if word.len() < 3 || !(word.contains('/') || word.contains('.')) || word.contains("://") {
            continue;
        }
        let candidate = Path::new(word.trim_start_matches("./"));
        if !is_inner_relative(candidate) {
            continue;
        }
        if root.join(candidate).exists() {
            let path = candidate.to_string_lossy().replace('\\', "/");
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

fn header(project: Option<&Project>, root: &Path, tools: bool) -> String {
    let name = project.map(|p| p.name.as_str()).unwrap_or("project");
    let more = if tools {
        "More is available through the memory.search, memory.list, memory.working and \
         decision.list tools, and files through filesystem.read."
    } else {
        "Tools are not available in this session: ask the user for any file or detail you \
         need."
    };
    format!(
        "# PROJECT CONTEXT\nProject {name} at {}. Selected by the Orchestrator for this session: \
         the relevant part of the project's memory, not all of it. {more}",
        root.display()
    )
}

fn render_section(draft: &Draft) -> String {
    let mut out = format!("## {}\n", draft.kind.title());
    for item in &draft.items {
        if item.plain {
            out.push_str(&item.text);
        } else {
            out.push_str("- ");
            out.push_str(&item.text);
        }
        out.push('\n');
    }
    out
}

fn render(header: &str, drafts: &[Draft]) -> String {
    let mut out = String::from(header);
    for draft in drafts.iter().filter(|d| !d.items.is_empty()) {
        out.push_str("\n\n");
        out.push_str(render_section(draft).trim_end());
    }
    out
}

/// Cuts items, lowest priority first (the last of a section first), until
/// the text fits the budget or only kept items remain.
fn trim(drafts: &mut [Draft], header: &str, budget: u32) {
    loop {
        if estimate_tokens(&render(header, drafts)) <= budget {
            return;
        }
        let victim = drafts
            .iter()
            .enumerate()
            .filter_map(|(i, d)| {
                d.items
                    .iter()
                    .map(|item| item.priority)
                    .filter(|p| *p != KEEP)
                    .min()
                    .map(|p| (p, std::cmp::Reverse(i)))
            })
            .min()
            .map(|(p, std::cmp::Reverse(i))| (i, p));
        let Some((index, lowest)) = victim else {
            return;
        };
        let draft = &mut drafts[index];
        if let Some(position) = draft.items.iter().rposition(|item| item.priority == lowest) {
            draft.items.remove(position);
            draft.removed += 1;
        }
    }
}

#[async_trait]
impl ContextSource for ContextBuilder {
    async fn build(&self, request: ContextRequest) -> Result<Option<AttachedContext>, String> {
        let handoff = match &request.options.handoff_id {
            Some(id) => Some(
                self.store
                    .handoff(id.as_str())
                    .ok_or_else(|| format!("handoff {id} não encontrado"))?,
            ),
            None => None,
        };
        let enabled = request
            .options
            .enabled
            .unwrap_or_else(|| self.settings().auto_attach);
        // A session that takes over a handoff always gets its packet.
        if !enabled && handoff.is_none() {
            return Ok(None);
        }
        let build = BuildRequest {
            project_path: request.session.project_path.clone(),
            task: request.task.clone(),
            handoff,
            budget: request.options.budget,
            session_id: Some(request.session.id.to_string()),
            no_tools: !request.tools,
        };
        let this = BuilderRef {
            store: self.store.clone(),
            git: self.git.clone(),
            settings: self.settings(),
        };
        // SQLite and `git status` block: off the async threads.
        let pack = tokio::task::spawn_blocking(move || this.builder().build(&build))
            .await
            .map_err(|e| format!("context task failed: {e}"))?;
        let mut summary = pack.summary();
        summary.handoff_id = request.options.handoff_id.clone();
        Ok(Some(AttachedContext {
            text: pack.text,
            summary,
        }))
    }
}

/// What a blocking build needs, owned.
struct BuilderRef {
    store: Arc<MemoryStore>,
    git: Option<Git>,
    settings: ContextSettings,
}

impl BuilderRef {
    fn builder(self) -> ContextBuilder {
        ContextBuilder {
            store: self.store,
            git: self.git,
            settings: RwLock::new(self.settings),
            settings_path: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(kind: SectionKind, items: &[(usize, u8)]) -> Draft {
        Draft::with(
            kind,
            items
                .iter()
                .map(|(len, p)| Item::bullet("x".repeat(*len), *p))
                .collect(),
        )
    }

    #[test]
    fn trimming_goes_by_priority_and_keeps_what_must_stay() {
        let mut drafts = vec![
            Draft::with(SectionKind::Task, vec![Item::plain("t".repeat(40), KEEP)]),
            draft(
                SectionKind::Project,
                &[(200, priority::PINNED), (200, priority::PROJECT)],
            ),
            draft(
                SectionKind::History,
                &[(200, priority::HISTORY), (200, priority::HISTORY)],
            ),
        ];
        let header = "# H";
        let full = estimate_tokens(&render(header, &drafts));
        // Room for all but ~2 items: history goes first.
        trim(&mut drafts, header, full - 60);
        assert_eq!((drafts[1].items.len(), drafts[2].items.len()), (2, 0));
        assert_eq!(drafts[2].removed, 2);
        // A tiny budget leaves only what is kept.
        trim(&mut drafts, header, 1);
        assert_eq!(drafts[0].items.len(), 1);
        assert!(drafts[1].items.is_empty());
        assert_eq!(drafts[1].removed, 2);
    }

    #[test]
    fn finds_paths_mentioned_in_the_task() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/api")).unwrap();
        std::fs::write(dir.path().join("src/api/webhooks.ts"), "").unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();
        let found = mentioned_paths(
            "Corrija `src/api/webhooks.ts` e o README.md. Veja https://x.io/a.b e ../fora.txt, não.existe",
            dir.path(),
        );
        assert_eq!(found, ["src/api/webhooks.ts", "README.md"]);
    }
}
