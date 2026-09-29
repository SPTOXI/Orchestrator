// TypeScript mirror of the Rust contracts (packages/core, packages/runtime,
// packages/providers).
// Keep in sync with the serde definitions (camelCase JSON). See ADR-0001.

// ------------------------------------------------------------------ core ---

export type CallOrigin =
  | { type: "user" }
  /** Until Phase 8 each provider session is its own agent (ADR-0009). */
  | { type: "agent"; agentId: string; sessionId?: string; provider?: string }
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
  | "COMMAND_FAILED"
  | "CANCELLED"
  | "INTERNAL";

export interface ToolError {
  kind: ToolErrorKind;
  message: string;
}

export interface ToolCall {
  id: string;
  tool: string;
  args: unknown;
  origin: CallOrigin;
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
  readOnly: boolean;
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
  | "TERMINAL_EXITED"
  | "SESSION_STARTED"
  | "SESSION_RESUMED"
  | "SESSION_CLOSED"
  | "TURN_COMPLETED";

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
  | { type: "processExited"; processId: string; exitCode: number | null; stopped: boolean }
  | { type: "session"; sessionId: string; seq: number; event: SessionEvent };

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

// --------------------------------------------------------------- project ---

export interface ProjectCandidate {
  name: string;
  path: string;
  markers: string[];
  isGitRepo: boolean;
}

export interface DiscoverOutput {
  roots: string[];
  projects: ProjectCandidate[];
  scannedDirs: number;
  truncated: boolean;
}

export interface RuntimeRequirement {
  name: string;
  version: string | null;
}

export interface DockerInfo {
  dockerfiles: string[];
  composeFiles: string[];
  images: string[];
}

export interface GitRemote {
  name: string;
  url: string;
}

export interface GitSummary {
  root: string;
  branch: string | null;
  head: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  remotes: GitRemote[];
  staged: number;
  modified: number;
  deleted: number;
  untracked: number;
  conflicted: number;
  clean: boolean;
}

export interface ProjectProfile {
  name: string;
  path: string;
  git: GitSummary | null;
  languages: string[];
  frameworks: string[];
  packageManagers: string[];
  runtimes: RuntimeRequirement[];
  docker: DockerInfo;
  databases: string[];
  tools: string[];
  importantFiles: string[];
  scripts: Record<string, string>;
  monorepo: boolean;
  markers: string[];
  detectedAt: string;
}

// ------------------------------------------------------------------- git ---

export type ChangeKind =
  | "modified"
  | "added"
  | "deleted"
  | "renamed"
  | "copied"
  | "typeChanged"
  | "untracked"
  | "conflicted";

export interface FileChange {
  path: string;
  originalPath: string | null;
  staged: ChangeKind | null;
  unstaged: ChangeKind | null;
  conflicted: boolean;
}

export interface GitStatus {
  root: string;
  branch: string | null;
  head: string | null;
  detached: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
  files: FileChange[];
  clean: boolean;
}

export interface GitStatusWithRemotes extends GitStatus {
  remotes: GitRemote[];
}

export interface GitCommit {
  hash: string;
  shortHash: string;
  parents: string[];
  author: string;
  email: string;
  date: string;
  subject: string;
}

export interface GitBranch {
  name: string;
  remote: boolean;
  current: boolean;
  upstream: string | null;
  commit: string;
  date: string;
  subject: string;
}

export interface GitDiffFile {
  path: string;
  originalPath: string | null;
  additions: number | null;
  deletions: number | null;
  binary: boolean;
}

export interface GitDiff {
  patch: string;
  files: GitDiffFile[];
  truncated: boolean;
}

export interface GitCommandOutput {
  stdout: string;
  stderr: string;
}

export interface GitChangedOutput {
  output: GitCommandOutput;
  status: GitStatus;
}

export interface GitCommitResult {
  hash: string;
  shortHash: string;
  branch: string | null;
  subject: string;
  output: GitCommandOutput;
}

export interface GitStashEntry {
  reference: string;
  index: number;
  message: string;
  date: string;
}

export interface GitStashOutput {
  output: GitCommandOutput | null;
  stashes: GitStashEntry[];
}

// ------------------------------------------------------- package/runtime ---

export interface PackageOutput {
  manager: string;
  command: string;
  result?: ExecuteOutput;
  process?: ProcessInfo;
}

export interface NodeRuntime {
  available: boolean;
  version: string | null;
  managers: Record<string, string | null>;
}

export interface PythonRuntime {
  available: boolean;
  version: string | null;
  command: string | null;
  pip: string | null;
}

export interface DockerRuntime {
  available: boolean;
  version: string | null;
  daemonRunning: boolean;
  serverVersion: string | null;
  compose: string | null;
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

// ------------------------------------------------------------- providers ---

export interface TokenUsage {
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number;
  reasoningTokens: number;
  costUsd: number | null;
  estimated: boolean;
}

export type SessionStatus = "idle" | "running" | "closed";
export type TurnStatus = "completed" | "cancelled" | "failed";
export type NoticeLevel = "info" | "warning" | "error";

export interface SessionInfo {
  id: string;
  provider: string;
  title: string;
  model: string | null;
  projectPath: string;
  parentId: string | null;
  status: SessionStatus;
  nativeRef: string | null;
  createdAt: string;
  updatedAt: string;
  turns: number;
  usage: TokenUsage;
  lastError: string | null;
}

export type SessionEvent =
  | { type: "turnStarted"; turnId: string; input: string }
  | { type: "textDelta"; turnId: string; text: string }
  | { type: "reasoningDelta"; turnId: string; text: string }
  | { type: "toolCallRequested"; turnId: string; call: ToolCall }
  | { type: "toolCallCompleted"; turnId: string; result: ToolResult }
  | { type: "usage"; turnId: string; usage: TokenUsage }
  | { type: "notice"; turnId: string | null; level: NoticeLevel; message: string }
  | {
      type: "turnCompleted";
      turnId: string;
      status: TurnStatus;
      error: string | null;
      usage: TokenUsage;
      durationMs: number;
      toolCalls: number;
    }
  | { type: "statusChanged"; status: SessionStatus }
  | { type: "subagentSpawned"; childId: string; provider: string; title: string };

export interface SessionLogEntry {
  seq: number;
  at: string;
  event: SessionEvent;
}

export interface SessionSnapshot {
  info: SessionInfo;
  entries: SessionLogEntry[];
  lastSeq: number;
  truncated: boolean;
}

export interface ModelInfo {
  id: string;
  name: string;
  contextWindow: number | null;
}

export interface ProviderCapabilities {
  streaming: boolean;
  toolCalls: boolean;
  resume: boolean;
  cancel: boolean;
  nativeSubagents: boolean;
  reasoning: boolean;
  tokenUsage: boolean;
  cost: boolean;
  models: ModelInfo[];
  defaultModel: string | null;
}

export interface ProviderInfo {
  id: string;
  name: string;
  vendor: string;
  description: string;
  capabilities: ProviderCapabilities;
  active: boolean;
}

export interface ProvidersView {
  providers: ProviderInfo[];
  active: string | null;
}

export interface ProviderStatus {
  available: boolean;
  version: string | null;
  authenticated: boolean | null;
  detail: string | null;
  checkedAt: string;
}

export type ProviderErrorKind =
  | "NOT_FOUND"
  | "ALREADY_EXISTS"
  | "UNAVAILABLE"
  | "UNSUPPORTED"
  | "INVALID_REQUEST"
  | "BUSY"
  | "CLOSED"
  | "CANCELLED"
  | "FAILED"
  | "INTERNAL";

export interface ProviderError {
  kind: ProviderErrorKind;
  message: string;
}

export interface StartRequest {
  /** Provider id; omitted = the active one (or the parent's). */
  provider?: string;
  title?: string;
  model?: string;
  instructions?: string;
}
