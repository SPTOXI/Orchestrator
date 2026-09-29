// Typed client of the Orchestrator IPC surface (docs/ipc.md).
//
// Every operation goes through the single audited gateway `runtime_invoke`
// (ADR-0003). Only human keystrokes/resizes of an open terminal use the
// streaming commands `terminal_input` / `terminal_resize`. AI providers and
// sessions have their own commands (ADR-0009).

import { invoke, isTauri as detectTauri } from "@tauri-apps/api/core";
import type {
  AppInfo,
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
  WriteOutput,
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
  /** Deliberates; in Full mode the Council also opens the session. */
  run: (request: DeliberateRequest) => callProvider<RunOutcome>("council_run", { request }),
  history: () => callProvider<Deliberation[]>("council_history"),
  /** Opens a session with the approved or picked model (ROUTE_DECIDED). */
  startSession: (request: RouteStart) => callProvider<RouteStarted>("route_start_session", { request }),
};

/** History, projects and project memory in the local database (ADR-0012). */
export const memoryApi = {
  history: (query: HistoryQuery = {}) => callProvider<HistoryPage>("history_query", { query }),
  projectsRecent: (limit = 8) => callProvider<Project[]>("projects_recent", { limit }),
  projectCurrent: () => callProvider<Project | null>("project_current"),
  projectForget: (id: string) => callProvider<void>("project_forget", { id }),
  importRecent: (list: Array<{ path: string; name: string; openedAt: string }>) =>
    callProvider<number>("projects_import_recent", { list }),
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
