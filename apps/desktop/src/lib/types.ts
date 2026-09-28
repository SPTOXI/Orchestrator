// TypeScript mirror of the Rust contracts (packages/core, packages/runtime).
// Keep in sync with the serde definitions (camelCase JSON). See ADR-0001.

// ------------------------------------------------------------------ core ---

export type CallOrigin =
  | { type: "user" }
  | { type: "agent"; agentId: string }
  | { type: "system" };

export type ToolErrorKind =
  | "UNKNOWN_TOOL"
  | "INVALID_ARGS"
  | "NOT_FOUND"
  | "ALREADY_EXISTS"
  | "PERMISSION_DENIED"
  | "IO"
  | "SPAWN"
  | "NOT_RUNNING"
  | "INTERNAL";

export interface ToolError {
  kind: ToolErrorKind;
  message: string;
}

export interface ToolResult<T = unknown> {
  callId: string;
  tool: string;
  ok: boolean;
  output: T;
  error: ToolError | null;
  startedAt: string;
  finishedAt: string;
  durationMs: number;
}

export interface ToolSpec {
  name: string;
  group: string;
  description: string;
}

export type EventKind =
  | "PROJECT_CREATED"
  | "PROJECT_OPENED"
  | "TASK_CREATED"
  | "TASK_STARTED"
  | "TASK_COMPLETED"
  | "AGENT_STARTED"
  | "AGENT_FINISHED"
  | "PROVIDER_SWITCHED"
  | "TOOL_CALLED"
  | "FILE_CHANGED"
  | "COMMAND_EXECUTED"
  | "GIT_COMMIT"
  | "GIT_PUSH"
  | "HANDOFF_CREATED"
  | "HANDOFF_ACCEPTED"
  | "PROCESS_EXITED"
  | "TERMINAL_EXITED";

export interface AuditEvent {
  id: string;
  at: string;
  kind: EventKind;
  origin: CallOrigin;
  callId: string | null;
  summary: string;
  data: Record<string, unknown>;
}

export type OutputStream = "stdout" | "stderr";

export type StreamEvent =
  | { type: "terminalOutput"; terminalId: string; offset: number; data: string }
  | { type: "terminalExited"; terminalId: string; exitCode: number | null }
  | {
      type: "processOutput";
      processId: string;
      stream: OutputStream;
      offset: number;
      data: string;
    }
  | { type: "processExited"; processId: string; exitCode: number | null; stopped: boolean };

// ------------------------------------------------------------ filesystem ---

export type EntryKind = "file" | "directory" | "other";
export type Encoding = "utf8" | "base64";

export interface DirEntryInfo {
  name: string;
  path: string;
  kind: EntryKind;
  isSymlink: boolean;
  size: number;
  modifiedAt: string | null;
}

export interface ListOutput {
  path: string;
  entries: DirEntryInfo[];
}

export interface ReadOutput {
  path: string;
  content: string;
  encoding: Encoding;
  size: number;
  truncated: boolean;
}

export interface WriteOutput {
  path: string;
  bytesWritten: number;
  created: boolean;
}

export interface MoveOutput {
  from: string;
  to: string;
  replaced: boolean;
}

export interface DeleteOutput {
  path: string;
  kind: EntryKind;
}

// ----------------------------------------------------------------- shell ---

export type ShellKind = "bash" | "zsh" | "fish" | "sh" | "pwsh" | "powerShell" | "cmd" | "wsl";

export interface ShellInfo {
  id: string;
  kind: ShellKind;
  name: string;
  path: string;
}

export interface ShellList {
  default: string;
  shells: ShellInfo[];
}

export interface ExecuteArgs {
  command: string;
  cwd?: string;
  shell?: string;
  env?: Record<string, string>;
  timeoutMs?: number;
  stdin?: string;
  maxOutputBytes?: number;
}

export interface ExecuteOutput {
  command: string;
  shell: string;
  cwd: string;
  exitCode: number | null;
  stdout: string;
  stderr: string;
  stdoutTruncated: boolean;
  stderrTruncated: boolean;
  timedOut: boolean;
  durationMs: number;
}

// -------------------------------------------------------------- terminal ---

export interface OutputChunk {
  data: string;
  from: number;
  next: number;
  truncated: boolean;
  hasMore: boolean;
}

export interface TerminalInfo {
  id: string;
  shell: ShellInfo;
  cwd: string;
  pid: number | null;
  alive: boolean;
  exitCode: number | null;
  cols: number;
  rows: number;
  createdAt: string;
}

export interface TerminalRead extends OutputChunk {
  id: string;
  alive: boolean;
  exitCode: number | null;
}

// --------------------------------------------------------------- process ---

export type ProcessStatus = "running" | "exited" | "stopped";

export interface ProcessInfo {
  id: string;
  name: string;
  command: string;
  cwd: string;
  shell: string;
  pid: number | null;
  status: ProcessStatus;
  exitCode: number | null;
  startedAt: string;
  endedAt: string | null;
}

export interface ProcessRead extends OutputChunk {
  id: string;
  status: ProcessStatus;
  exitCode: number | null;
}

// ------------------------------------------------------------------- app ---

export interface AppInfo {
  version: string;
  os: string;
  arch: string;
  baseDir: string;
  dataDir: string;
  auditLog: string;
  defaultShell: string;
}
