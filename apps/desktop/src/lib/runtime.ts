// Typed client of the Orchestrator IPC surface (docs/ipc.md).
//
// Every operation goes through the single audited gateway `runtime_invoke`
// (ADR-0003). Only human keystrokes/resizes of an open terminal use the
// streaming commands `terminal_input` / `terminal_resize`.

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

/** Human readable message for any error thrown by this module. */
export function errorMessage(error: unknown): string {
  if (error instanceof ToolCallError) return `${error.kind}: ${error.message}`;
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
