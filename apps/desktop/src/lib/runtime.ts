// Typed client of the Orchestrator IPC surface (docs/ipc.md).
//
// Every operation goes through the single audited gateway `runtime_invoke`
// (ADR-0003). Only human keystrokes/resizes of an open terminal use the
// streaming commands `terminal_input` / `terminal_resize`. AI providers and
// sessions have their own commands (ADR-0009).

import { invoke, isTauri as detectTauri } from "@tauri-apps/api/core";
import type {
  BackupStatus,
  BudgetView,
  SecretsView,
  GuidanceSettings,
  CliKind,
  CliSettings,
  CliStatus,
  McpServerConfig,
  McpServerView,
  HfFile,
  LocalEngineInfo,
  LocalModel,
  LocalSettings,
  LocalView,
  GuidanceView,
  SkillDoc,
  SkillInfo,
  SpendReport,
  AppInfo,
  ContextOptions,
  ContextPack,
  ContextSettings,
  ContextSettingsView,
  Handoff,
  HandoffDraft,
  HandoffPacket,
  PreviewRequest,
  StartHandoff,
  StartedHandoff,
  AuditEvent,
  DeleteOutput,
  DiscoverOutput,
  DockerRuntime,
  GitBranch,
  GitChangedOutput,
  GitCommit,
  GitCommitResult,
  GitDiff,
  GitStashOutput,
  GitStatus,
  GitStatusWithRemotes,
  NodeRuntime,
  PackageOutput,
  ProjectProfile,
  ConnectionsView,
  ConnectionView,
  CouncilSettings,
  CouncilView,
  HistoryPage,
  HistoryQuery,
  MemoryEntry,
  MemoryInput,
  MemoryOverview,
  Project,
  ProjectDecision,
  ProjectDecisionInput,
  ProjectLink,
  SearchHit,
  DeliberateRequest,
  Deliberation,
  Recommendation,
  RouteRequest,
  RouteStart,
  RouteStarted,
  RunOutcome,
  ModelEntry,
  ProbeRequest,
  ProviderError,
  ProviderErrorKind,
  SaveConnectionRequest,
  TestReport,
  ProvidersView,
  ProviderStatus,
  SessionInfo,
  SessionSnapshot,
  StartRequest,
  PythonRuntime,
  Encoding,
  ExecuteArgs,
  ExecuteOutput,
  ListOutput,
  MoveOutput,
  ProcessInfo,
  ProcessRead,
  ReadOutput,
  ShellList,
  TerminalInfo,
  TerminalRead,
  ToolError,
  ToolErrorKind,
  ToolResult,
  ToolSpec,
  UpdateInfo,
  UpdateStatus,
  WriteOutput,
  StartedTask,
  StartTaskSession,
  Task,
  TaskInput,
  TaskStatus,
  TaskView,
  Agent,
  AgentSettings,
  AgentView,
  ApprovalAnswer,
  ApprovalView,
  GitHubComment,
  GitHubSetup,
  GitHubStatus,
  IssueSummary,
  MergeMethod,
  MergeResult,
  PullDetail,
  PullSummary,
  AutonomyMode,
  AutonomyOverview,
  PolicyRule,
  Trial,
  FileLock,
  StartAgent,
} from "./types";

/** True when running inside the Tauri desktop app (not a plain browser). */
export function isTauri(): boolean {
  try {
    return detectTauri();
  } catch {
    return false;
  }
}

/** A tool call that the runtime executed and reported as failed. */
export class ToolCallError extends Error {
  readonly tool: string;
  readonly kind: ToolErrorKind;

  constructor(tool: string, error: ToolError) {
    super(error.message);
    this.name = "ToolCallError";
    this.tool = tool;
    this.kind = error.kind;
  }
}

/** A provider/session command that failed (ADR-0009). */
export class ProviderCallError extends Error {
  readonly kind: ProviderErrorKind;

  constructor(error: ProviderError) {
    super(error.message);
    this.name = "ProviderCallError";
    this.kind = error.kind;
  }
}

function isProviderError(value: unknown): value is ProviderError {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as ProviderError).kind === "string" &&
    typeof (value as ProviderError).message === "string"
  );
}

/** Human readable message for any error thrown by this module. */
export function errorMessage(error: unknown): string {
  if (error instanceof ToolCallError || error instanceof ProviderCallError) return `${error.kind}: ${error.message}`;
  if (error instanceof Error) return error.message;
  return String(error);
}

type Args = Record<string, unknown>;

/** Executes one tool through the audited gateway; throws on failure. */
export async function callTool<T>(tool: string, args: Args = {}): Promise<T> {
  const result = await invoke<ToolResult<T>>("runtime_invoke", { tool, args });
  if (!result.ok) {
    throw new ToolCallError(tool, result.error ?? { kind: "INTERNAL", message: "falha sem detalhes" });
  }
  return result.output;
}

export const fsApi = {
  list: (path: string) => callTool<ListOutput>("filesystem.list", { path }),
  read: (path: string, options: { encoding?: Encoding; maxBytes?: number } = {}) =>
    callTool<ReadOutput>("filesystem.read", { path, ...options }),
  write: (path: string, content: string, options: { encoding?: Encoding; createDirs?: boolean; append?: boolean } = {}) =>
    callTool<WriteOutput>("filesystem.write", { path, content, ...options }),
  move: (from: string, to: string, overwrite = false) =>
    callTool<MoveOutput>("filesystem.move", { from, to, overwrite }),
  remove: (path: string, recursive = false) =>
    callTool<DeleteOutput>("filesystem.delete", { path, recursive }),
};

export const shellApi = {
  list: () => callTool<ShellList>("shell.list"),
  execute: (args: ExecuteArgs) => callTool<ExecuteOutput>("shell.execute", { ...args }),
};

export const terminalApi = {
  create: (options: { shell?: string; cwd?: string; cols?: number; rows?: number } = {}) =>
    callTool<TerminalInfo>("terminal.create", options),
  write: (id: string, data: string) =>
    callTool<{ id: string; bytesWritten: number }>("terminal.write", { id, data }),
  read: (id: string, since?: number, maxBytes?: number) =>
    callTool<TerminalRead>("terminal.read", { id, since, maxBytes }),
  close: (id: string) => callTool<TerminalInfo>("terminal.close", { id }),
  list: () => callTool<TerminalInfo[]>("terminal.list"),
  /** Human keystrokes (streaming channel, ADR-0003). */
  input: (id: string, data: string) => invoke<void>("terminal_input", { id, data }),
  /** PTY size (streaming channel, ADR-0003). */
  resize: (id: string, cols: number, rows: number) =>
    invoke<void>("terminal_resize", { id, cols, rows }),
};

export const processApi = {
  start: (options: { command: string; cwd?: string; shell?: string; name?: string }) =>
    callTool<ProcessInfo>("process.start", options),
  stop: (id: string, force = false) => callTool<ProcessInfo>("process.stop", { id, force }),
  list: () => callTool<ProcessInfo[]>("process.list"),
  read: (id: string, since?: number, maxBytes?: number) =>
    callTool<ProcessRead>("process.read", { id, since, maxBytes }),
};

export const projectApi = {
  discover: (options: { roots?: string[]; maxDepth?: number; maxDirs?: number } = {}) =>
    callTool<DiscoverOutput>("project.discover", options),
  /** Profile of `path`, or of the open project. */
  profile: (path?: string) => callTool<ProjectProfile>("project.profile", { path }),
  /** Opens a project: it becomes the runtime base directory. */
  open: (path: string) => callTool<ProjectProfile>("project.open", { path }),
};

type RepoArgs = { path?: string };

export const gitApi = {
  status: (path?: string) => callTool<GitStatusWithRemotes>("git.status", { path }),
  diff: (args: RepoArgs & { staged?: boolean; target?: string; files?: string[]; maxBytes?: number }) =>
    callTool<GitDiff>("git.diff", args),
  log: (args: RepoArgs & { limit?: number; ref?: string; file?: string } = {}) =>
    callTool<GitCommit[]>("git.log", args),
  branch: (args: RepoArgs & { create?: string; startPoint?: string; delete?: string; force?: boolean } = {}) =>
    callTool<GitBranch[]>("git.branch", args),
  checkout: (args: RepoArgs & { target: string; create?: boolean; startPoint?: string }) =>
    callTool<GitChangedOutput>("git.checkout", args),
  add: (args: RepoArgs & { files?: string[]; all?: boolean }) => callTool<GitStatus>("git.add", args),
  commit: (args: RepoArgs & { message: string; all?: boolean; amend?: boolean }) =>
    callTool<GitCommitResult>("git.commit", args),
  pull: (args: RepoArgs & { remote?: string; branch?: string; mode?: "merge" | "rebase" | "ffOnly" } = {}) =>
    callTool<GitChangedOutput>("git.pull", args),
  fetch: (args: RepoArgs & { remote?: string; prune?: boolean } = {}) =>
    callTool<GitChangedOutput>("git.fetch", { prune: true, ...args }),
  push: (args: RepoArgs & { remote?: string; branch?: string; setUpstream?: boolean; force?: boolean } = {}) =>
    callTool<GitChangedOutput>("git.push", args),
  stash: (
    args: RepoArgs & {
      action: "push" | "pop" | "apply" | "drop" | "list";
      message?: string;
      includeUntracked?: boolean;
      index?: number;
    },
  ) => callTool<GitStashOutput>("git.stash", args),
  reset: (args: RepoArgs & { mode?: "soft" | "mixed" | "hard"; target?: string; files?: string[] }) =>
    callTool<GitChangedOutput>("git.reset", args),
};

export const packageApi = {
  install: (args: { path?: string; packages?: string[]; dev?: boolean; manager?: string } = {}) =>
    callTool<PackageOutput>("package.install", args),
  run: (args: { script: string; args?: string[]; path?: string; manager?: string; background?: boolean }) =>
    callTool<PackageOutput>("package.run", args),
};

export const runtimeInfoApi = {
  node: () => callTool<NodeRuntime>("runtime.node"),
  python: () => callTool<PythonRuntime>("runtime.python"),
  docker: () => callTool<DockerRuntime>("runtime.docker"),
};

export const appApi = {
  info: () => invoke<AppInfo>("app_info"),
  history: (limit = 500) => invoke<AuditEvent[]>("history_recent", { limit }),
  tools: () => invoke<ToolSpec[]>("runtime_tools"),
  /** Native folder picker (UI only; open the result with projectApi.open). */
  pickFolder: () => invoke<string | null>("pick_folder"),
};

/** Calls a provider/session command; structured errors become ProviderCallError. */
async function callProvider<T>(command: string, args: Args = {}): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw isProviderError(error) ? new ProviderCallError(error) : error;
  }
}

export const providerApi = {
  list: () => callProvider<ProvidersView>("providers_list"),
  inspect: (id: string) => callProvider<ProviderStatus>("provider_inspect", { id }),
  /** Makes `id` the active provider (PROVIDER_SWITCHED). */
  select: (id: string) => callProvider<ProvidersView>("provider_select", { id }),
};

export const sessionApi = {
  list: () => callProvider<SessionInfo[]>("sessions_list"),
  /** Opens a session on the open project. */
  start: (request: StartRequest = {}) => callProvider<SessionInfo>("session_start", { request }),
  get: (id: string) => callProvider<SessionSnapshot>("session_get", { id }),
  /** Starts a turn; progress arrives as `session` stream events. Returns the turn id. */
  send: (id: string, input: string) => callProvider<string>("session_send", { id, input }),
  cancel: (id: string) => callProvider<SessionInfo>("session_cancel", { id }),
  close: (id: string) => callProvider<SessionInfo>("session_close", { id }),
  resume: (id: string) => callProvider<SessionInfo>("session_resume", { id }),
  /** Subagent session (spawnAgent). */
  spawn: (parentId: string, request: StartRequest = {}) =>
    callProvider<SessionInfo>("session_spawn", { parentId, request }),
  /** "Compactar" (ADR-0018): the AI summarizes the conversation; waits for it. */
  compact: (id: string) => callProvider<unknown>("session_compact", { id }),
  /** Project context options; changeable before the first turn (ADR-0013). */
  context: (id: string) => callProvider<ContextOptions>("session_context_get", { id }),
  setContext: (id: string, options: ContextOptions) =>
    callProvider<ContextOptions>("session_context_set", { id, options }),
};

/** User-registered AI APIs (ADR-0010). Keys go to the OS vault, never back. */
export const connectionApi = {
  list: () => callProvider<ConnectionsView>("connections_list"),
  save: (request: SaveConnectionRequest) => callProvider<ConnectionView>("connection_save", { request }),
  remove: (id: string) => callProvider<void>("connection_delete", { id }),
  test: (request: ProbeRequest) => callProvider<TestReport>("connection_test", { request }),
  models: (request: ProbeRequest) => callProvider<ModelEntry[]>("connection_models", { request }),
};

/** Model router and Council (ADR-0011). */
export const councilApi = {
  get: () => callProvider<CouncilView>("council_get"),
  save: (settings: CouncilSettings) => callProvider<CouncilSettings>("council_save", { settings }),
  /** Router ranking only (no tokens). */
  recommend: (request: RouteRequest) => callProvider<Recommendation>("router_recommend", { request }),
  /** The members analyze the demand and write the plan; in Full mode the
   * first one also carries it out (ADR-0024). Council off: the router. */
  run: (request: DeliberateRequest) => callProvider<RunOutcome>("council_run", { request }),
  /** Carries out an approved plan: the first member able opens the session,
   * the others are its reserves. */
  execute: (deliberationId: string) => callProvider<RouteStarted>("council_execute", { deliberationId }),
  history: () => callProvider<Deliberation[]>("council_history"),
  /** Opens a session with a model the user picked (ROUTE_DECIDED). */
  startSession: (request: RouteStart) => callProvider<RouteStarted>("route_start_session", { request }),
};

/** Context Builder and handoff between AIs (ADR-0013). */
export const contextApi = {
  /** What a session would receive for a task or handoff (nothing is sent). */
  preview: (request: PreviewRequest = {}) => callProvider<ContextPack>("context_preview", { request }),
  settings: () => callProvider<ContextSettingsView>("context_settings_get"),
  saveSettings: (settings: ContextSettings) =>
    callProvider<ContextSettings>("context_settings_save", { settings }),
};

export const handoffApi = {
  /** Draft from a session; `askAgent` asks its AI for the narrative (one turn). */
  prepare: (sessionId: string, askAgent: boolean) =>
    callProvider<HandoffDraft>("handoff_prepare", { request: { sessionId, askAgent } }),
  create: (sessionId: string, packet: HandoffPacket, byAgent: boolean) =>
    callProvider<Handoff>("handoff_create", { request: { sessionId, packet, byAgent } }),
  start: (request: StartHandoff) => callProvider<StartedHandoff>("handoff_start", { request }),
  list: (projectId?: string) => callProvider<Handoff[]>("handoffs_list", { projectId }),
  get: (id: string) => callProvider<Handoff | null>("handoff_get", { id }),
};

/** Tasks of the project (ADR-0014). */
export const taskApi = {
  /** Tasks of a project (the open one by default), in panel order. */
  list: (projectId?: string) => callProvider<TaskView[]>("tasks_list", { projectId }),
  get: (id: string) => callProvider<TaskView | null>("task_get", { id }),
  /** Creates a task (no `id`) or edits one. */
  save: (input: TaskInput) => callProvider<Task>("task_save", { input }),
  status: (id: string, status: TaskStatus) => callProvider<Task>("task_status", { id, status }),
  startSession: (request: StartTaskSession) =>
    callProvider<StartedTask>("task_start_session", { request }),
  /** What a session opened for this task would receive. */
  context: (id: string) => callProvider<ContextPack>("task_context", { id }),
};

/** Agents, their file locks and the limits around them (ADR-0015). */
export const agentApi = {
  /** Agents of a project (the open one by default), in board order. */
  list: (projectId?: string) => callProvider<AgentView[]>("agents_list", { projectId }),
  get: (id: string) => callProvider<AgentView | null>("agent_get", { id }),
  /** Queues an agent for a task. */
  start: (request: StartAgent) => callProvider<Agent>("agent_start", { request }),
  stop: (id: string) => callProvider<Agent>("agent_stop", { id }),
  /** Stop All Agents; returns how many were asked to stop. */
  stopAll: (projectId?: string) => callProvider<number>("agents_stop_all", { projectId }),
  /** Files held by agents right now. */
  locks: (projectId?: string) => callProvider<FileLock[]>("agent_locks", { projectId }),
  settings: () => callProvider<AgentSettings>("agent_settings_get"),
  saveSettings: (settings: AgentSettings) =>
    callProvider<AgentSettings>("agent_settings_save", { settings }),
  /** Pause: the agent stops at its next tool call or turn (ADR-0016). */
  pause: (id: string) => callProvider<AgentView>("agent_pause", { id }),
  resume: (id: string) => callProvider<AgentView>("agent_resume", { id }),
  /** The open project's spending today against the daily budget (ADR-0018). */
  budget: (projectId?: string) => callProvider<BudgetView>("agents_budget", { projectId }),
};

/** What the AIs spent, from the history (ADR-0018). */
export const costApi = {
  /** Last `days` days (1 = today) of the open project, or of every project. */
  report: (days: number, allProjects = false) =>
    callProvider<SpendReport>("spend_report", { days, allProjects }),
};

/** Installers and updates (ADR-0019): the updater is on the Rust side; the
 * webview only asks. Installing is always the user's call. */
export const updateApi = {
  status: () => invoke<UpdateStatus>("update_status"),
  /** The newer version, or null when this one is the latest. */
  check: () => invoke<UpdateInfo | null>("update_check"),
  /** Stops running agents (with handoff), downloads, verifies and installs. */
  install: () => invoke<void>("update_install"),
  /** Opens the installed version. */
  restart: () => invoke<void>("update_restart"),
  saveSettings: (autoCheck: boolean) => invoke<UpdateStatus>("update_settings_save", { autoCheck }),
};

/** GitHub (ADR-0017): tools through the runtime; settings and token
 * through their own commands (the token never comes back). */
/** Secrets the AIs use by name (ADR-0020). Values go in, never out. */
export const secretsApi = {
  list: () => invoke<SecretsView>("secrets_list"),
  save: (name: string, value: string) => invoke<SecretsView>("secret_save", { name, value }),
  remove: (name: string) => invoke<SecretsView>("secret_delete", { name }),
};

/** Local models with the Orchestrator's own engine (ADR-0025). */
export const localApi = {
  status: () => invoke<LocalView>("local_status"),
  installEngine: () => invoke<LocalEngineInfo>("local_engine_install"),
  latestEngine: () => invoke<string>("local_engine_latest"),
  removeEngine: () => invoke<void>("local_engine_remove"),
  /** Cancels a download: "engine", a catalog id or "repo/file". */
  cancel: (key: string) => invoke<void>("local_cancel", { key }),
  downloadCatalog: (entry: string) => invoke<LocalModel>("local_download_catalog", { entry }),
  hfFiles: (repo: string) => invoke<HfFile[]>("local_hf_files", { repo }),
  downloadHf: (repo: string, file: string) => invoke<LocalModel>("local_download_hf", { repo, file }),
  importOllama: (name: string) => invoke<LocalModel>("local_import_ollama", { name }),
  addFile: (path: string) => invoke<LocalModel>("local_add_file", { path }),
  pickFile: () => invoke<string | null>("pick_model_file"),
  removeModel: (id: string) => invoke<void>("local_remove_model", { id }),
  setContext: (id: string, context: number) => invoke<LocalModel>("local_set_context", { id, context }),
  saveSettings: (settings: LocalSettings) => invoke<LocalSettings>("local_settings_save", { settings }),
  stop: () => invoke<void>("local_stop"),
  log: () => invoke<string[]>("local_log"),
  /** Removes the connection the app made for Ollama before ADR-0025. */
  removeLegacy: () => invoke<void>("local_remove_legacy"),
  activateConnection: () => invoke<void>("local_connection_activate"),
};

export const mcpApi = {
  list: () => invoke<McpServerView[]>("mcp_list"),
  save: (server: McpServerConfig, previousId: string | null) =>
    invoke<McpServerView[]>("mcp_save", { server, previousId }),
  importJson: (text: string) => invoke<string[]>("mcp_import", { text }),
  restart: (id: string) => invoke<void>("mcp_restart", { id }),
  remove: (id: string) => invoke<McpServerView[]>("mcp_delete", { id }),
  setTool: (id: string, tool: string, enabled: boolean) =>
    invoke<McpServerView[]>("mcp_set_tool", { id, tool, enabled }),
};

export const backupApi = {
  status: () => invoke<BackupStatus>("backup_status"),
  create: (label: string | null) => invoke<BackupStatus>("backup_create", { label }),
  remove: (id: string) => invoke<BackupStatus>("backup_delete", { id }),
  /** The app restarts to put the files back. */
  restore: (id: string) => invoke<void>("backup_restore", { id }),
};

export const cliApi = {
  list: () => invoke<CliStatus[]>("clis_list"),
  save: (kind: CliKind, settings: CliSettings) => invoke<CliStatus[]>("cli_save", { kind, settings }),
};

export const guidanceApi = {
  get: () => invoke<GuidanceView>("guidance_get"),
  saveSettings: (settings: GuidanceSettings) => invoke<GuidanceSettings>("guidance_settings_save", { settings }),
  saveRules: (text: string) => invoke<void>("rules_save", { text }),
  skill: (name: string) => invoke<SkillDoc>("skill_get", { name }),
  saveSkill: (skill: { name: string; description: string; body: string; previousName: string | null }) =>
    invoke<SkillInfo>("skill_save", { skill }),
  deleteSkill: (name: string) => invoke<void>("skill_delete", { name }),
  setSkillEnabled: (name: string, enabled: boolean) =>
    invoke<GuidanceSettings>("skill_set_enabled", { name, enabled }),
};

export const githubApi = {
  status: (path?: string) => callTool<GitHubStatus>("github.status", { path }),
  pulls: (args: { path?: string; state?: "open" | "closed" | "all"; head?: string; limit?: number } = {}) =>
    callTool<{ repo: string; pulls: PullSummary[] }>("github.pr.list", args),
  pull: (number: number, path?: string) => callTool<PullDetail>("github.pr.get", { number, path }),
  createPull: (args: { path?: string; title: string; body?: string; base?: string; draft?: boolean }) =>
    callTool<PullSummary>("github.pr.create", args),
  comment: (number: number, body: string, path?: string) =>
    callTool<GitHubComment>("github.pr.comment", { number, body, path }),
  merge: (args: { path?: string; number: number; method: MergeMethod; deleteBranch: boolean }) =>
    callTool<MergeResult>("github.pr.merge", args),
  issues: (args: { path?: string; limit?: number } = {}) =>
    callTool<{ repo: string; issues: IssueSummary[] }>("github.issue.list", args),
  setup: () => invoke<GitHubSetup>("github_settings_get"),
  saveSettings: (settings: { host: string; apiUrl: string | null }) =>
    invoke<GitHubSetup>("github_settings_save", { settings }),
  saveToken: (token: string) => invoke<GitHubSetup>("github_token_save", { token }),
  clearToken: () => invoke<GitHubSetup>("github_token_clear"),
  /** Opens a page in the system browser. */
  openUrl: (url: string) => invoke<void>("open_url", { url }),
};

/** Autonomy: modes, rules, requests for authorization and pause (ADR-0016). */
export const autonomyApi = {
  /** The autonomy of a project (the open one by default). */
  get: (projectId?: string | null) => callProvider<AutonomyOverview>("autonomy_get", { projectId }),
  /** The project's mode; null goes back to the default. */
  setMode: (projectId: string | null, mode: AutonomyMode | null) =>
    callProvider<AutonomyOverview>("autonomy_set_mode", { projectId, mode }),
  setDefault: (projectId: string | null, mode: AutonomyMode) =>
    callProvider<AutonomyOverview>("autonomy_set_default", { projectId, mode }),
  saveRules: (projectId: string | null, rules: PolicyRule[]) =>
    callProvider<AutonomyOverview>("autonomy_save_rules", { projectId, rules }),
  resetRules: (projectId: string | null) =>
    callProvider<AutonomyOverview>("autonomy_reset_rules", { projectId }),
  /** Which rule decides a call (rules may be a draft). */
  tryCall: (request: {
    projectId: string | null;
    mode: AutonomyMode | null;
    rules: PolicyRule[] | null;
    tool: string;
    args: unknown;
  }) => callProvider<Trial>("autonomy_try", request),
  pending: () => callProvider<ApprovalView[]>("approvals_pending"),
  answer: (id: string, answer: ApprovalAnswer, note?: string | null) =>
    callProvider<void>("approval_answer", { id, answer, note: note ?? null }),
  revoke: (grantId: string) => callProvider<boolean>("autonomy_revoke", { grantId }),
  /** Pause every AI (section 11). */
  pauseAll: () => callProvider<boolean>("execution_pause"),
  resumeAll: () => callProvider<boolean>("execution_resume"),
};

/** History, projects and project memory in the local database (ADR-0012). */
export const memoryApi = {
  history: (query: HistoryQuery = {}) => callProvider<HistoryPage>("history_query", { query }),
  projectsRecent: (limit = 8) => callProvider<Project[]>("projects_recent", { limit }),
  projectCurrent: () => callProvider<Project | null>("project_current"),
  projectForget: (id: string) => callProvider<void>("project_forget", { id }),
  importRecent: (list: Array<{ path: string; name: string; openedAt: string }>) =>
    callProvider<number>("projects_import_recent", { list }),
  /** Projects open side by side, in sidebar order (ADR-0023). */
  projectsOpen: () => callProvider<Project[]>("projects_open"),
  /** Closes a project in the app; returns the one to show next. */
  projectClose: (id: string) => callProvider<Project | null>("project_close", { id }),
  projectsReorder: (ids: string[]) => callProvider<void>("projects_reorder", { ids }),
  links: (id: string) => callProvider<ProjectLink[]>("project_links", { id }),
  link: (id: string, other: string, note: string) =>
    callProvider<ProjectLink[]>("project_link", { id, other, note }),
  unlink: (id: string, other: string) => callProvider<ProjectLink[]>("project_unlink", { id, other }),
  /** L1 and totals; default: the open project. */
  overview: (projectId?: string) => callProvider<MemoryOverview | null>("memory_overview", { projectId }),
  list: (projectId: string) => callProvider<MemoryEntry[]>("memory_list", { projectId }),
  save: (input: MemoryInput) => callProvider<MemoryEntry>("memory_save", { input }),
  remove: (id: string) => callProvider<void>("memory_delete", { id }),
  search: (projectId: string, text: string, limit = 30) =>
    callProvider<SearchHit[]>("memory_search", { projectId, text, limit }),
  decisions: (projectId: string) => callProvider<ProjectDecision[]>("decisions_list", { projectId }),
  saveDecision: (input: ProjectDecisionInput) => callProvider<ProjectDecision>("decision_save", { input }),
};
