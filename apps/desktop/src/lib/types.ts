// TypeScript mirror of the Rust contracts (packages/core, packages/runtime,
// packages/providers).
// Keep in sync with the serde definitions (camelCase JSON). See ADR-0001.

// ------------------------------------------------------------------ core ---

export type CallOrigin =
  | { type: "user" }
  /** Until Phase 8 each provider session is its own agent (ADR-0009). */
  | { type: "agent"; agentId: string; sessionId?: string; provider?: string }
  | { type: "system" }
  /** The model Council acting on its own in Full mode (ADR-0011). */
  | { type: "council"; deliberationId?: string };

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

export interface ToolDefinition extends ToolSpec {
  /** JSON Schema of the arguments. */
  parameters: Record<string, unknown>;
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
  | "TURN_COMPLETED"
  | "CONNECTION_SAVED"
  | "CONNECTION_REMOVED"
  | "COUNCIL_CONFIGURED"
  | "COUNCIL_DELIBERATED"
  | "ROUTE_DECIDED"
  | "MEMORY_SAVED"
  | "MEMORY_REMOVED"
  | "DECISION_SAVED";

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
  /** Database file (history, sessions, memory), or "(memória)". */
  database: string;
  databaseWarning: string | null;
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
  supportsTools: boolean | null;
  /** USD per million input tokens. */
  inputPrice: number | null;
  outputPrice: number | null;
  tags: string[];
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
  /** Answers one-off requests: can sit on the Council (ADR-0011). */
  completion: boolean;
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

// ------------------------------------------------------- API connections ---

export type ApiKind = "openai" | "anthropic" | "gemini" | "generic";
export type CredentialSource = "none" | "vault" | "env";
export type ToolMode = "native" | "prompt" | "none";
export type StreamFormat = "none" | "sse" | "ndjson";
export type MessageFormat = "chat" | "prompt";

export type GenericAuth =
  | { type: "bearer" }
  | { type: "header"; name: string; prefix?: string }
  | { type: "query"; param: string }
  | { type: "none" };

export interface GenericProfile {
  path: string;
  auth: GenericAuth;
  messageFormat: MessageFormat;
  roles: { system: string; user: string; assistant: string };
  body: unknown;
  stream: StreamFormat;
  textPath: string;
  donePath: string | null;
  doneMarker: string | null;
  inputTokensPath: string | null;
  outputTokensPath: string | null;
  errorPath: string | null;
  modelsPath: string | null;
  modelsListPath: string | null;
  modelIdField: string | null;
}

export interface ModelEntry {
  id: string;
  name: string | null;
  contextWindow: number | null;
  maxOutputTokens: number | null;
  supportsTools: boolean | null;
  supportsVision: boolean | null;
  inputPrice: number | null;
  outputPrice: number | null;
  tags: string[];
  extraBody: unknown;
  enabled: boolean;
}

export interface Connection {
  id: string;
  name: string;
  kind: ApiKind;
  baseUrl: string;
  credential: { source: CredentialSource; envVar: string | null };
  headers: Record<string, string>;
  extraBody: unknown;
  models: ModelEntry[];
  defaultModel: string | null;
  toolMode: ToolMode | null;
  maxToolRounds: number;
  maxOutputTokens: number | null;
  options: { streamUsage: boolean | null; eagerToolStreaming: boolean | null; refusalFallback: boolean | null };
  generic: GenericProfile | null;
  enabled: boolean;
  notes: string | null;
}

export interface KeyStatus {
  source: CredentialSource;
  present: boolean;
  detail: string | null;
}

export interface ConnectionView {
  connection: Connection;
  key: KeyStatus;
}

export interface Preset {
  key: string;
  label: string;
  hint: string;
  connection: Connection;
}

export interface ConnectionsView {
  connections: ConnectionView[];
  presets: Preset[];
  vault: string;
  warnings: string[];
}

export interface TestReport {
  ok: boolean;
  model: string | null;
  servedModel: string | null;
  latencyMs: number;
  reply: string | null;
  usage: TokenUsage | null;
  tools: "notTested" | "passed" | "noCall" | "failed";
  toolsDetail: string | null;
  error: string | null;
}

export interface SaveConnectionRequest {
  connection: Connection;
  apiKey?: string;
  clearKey?: boolean;
  previousId?: string;
}

export interface ProbeRequest {
  connection: Connection;
  apiKey?: string;
  model?: string;
}

// ---------------------------------------------------------------- router ---
// packages/router (ADR-0011).

export type Activity = "code" | "debug" | "review" | "tests" | "planning" | "docs" | "summary" | "general";

export type Preference = "quality" | "balanced" | "cost" | "speed";

export interface ActivityProfile {
  activity: Activity;
  label: string;
  tags: string[];
  needsTools: boolean;
  preference: Preference;
}

export interface RouteRequest {
  task: string;
  /** null = detected from the task. */
  activity?: Activity | null;
  preference?: Preference | null;
  needsTools?: boolean | null;
  minContext?: number | null;
}

export interface ModelRef {
  provider: string;
  model: string;
}

export interface Criteria {
  tags: number;
  quality: number;
  cost: number;
  speed: number;
  context: number;
  tools: number;
}

export interface Candidate extends ModelRef {
  providerName: string;
  modelName: string;
  /** 0–100. */
  score: number;
  criteria: Criteria;
  reasons: string[];
  contextWindow: number | null;
  inputPrice: number | null;
  outputPrice: number | null;
  supportsTools: boolean | null;
  tags: string[];
}

export interface Excluded extends ModelRef {
  providerName: string;
  reason: string;
}

export interface Recommendation {
  activity: Activity;
  detected: boolean;
  preference: Preference;
  needsTools: boolean;
  minContext: number | null;
  candidates: Candidate[];
  excluded: Excluded[];
}

export type CouncilMode = "off" | "suggest" | "full";

export interface CouncilMember {
  provider: string;
  /** null = the provider's default model. */
  model: string | null;
}

export interface CouncilSettings {
  mode: CouncilMode;
  members: CouncilMember[];
  shortlist: number;
  cacheMinutes: number;
  timeoutSecs: number;
  preference: Preference | null;
  sendTask: boolean;
}

export interface CouncilView {
  settings: CouncilSettings;
  activities: ActivityProfile[];
  maxMembers: number;
  warning: string | null;
}

export interface Vote {
  member: CouncilMember;
  providerName: string;
  model: string | null;
  choice: ModelRef | null;
  ranking: ModelRef[];
  confidence: number | null;
  reason: string | null;
  error: string | null;
  usage: TokenUsage;
  durationMs: number;
}

export type DecisionSource = "router" | "council";

export interface Decision extends ModelRef {
  providerName: string;
  modelName: string;
  source: DecisionSource;
  reason: string;
  agreement: number | null;
}

export interface Deliberation {
  id: string;
  createdAt: string;
  task: string;
  mode: CouncilMode;
  recommendation: Recommendation;
  shortlist: ModelRef[];
  votes: Vote[];
  decision: Decision | null;
  usage: TokenUsage;
  cached: boolean;
  cachedFrom: string | null;
  savedUsage: TokenUsage | null;
  notices: string[];
  autoApply: boolean;
  durationMs: number;
}

export interface DeliberateRequest extends RouteRequest {
  /** Ignore the cache. */
  force?: boolean;
}

export interface RouteStart {
  deliberationId?: string | null;
  provider: string;
  model?: string | null;
  title?: string | null;
  task?: string | null;
  sendTask?: boolean | null;
}

export interface RouteStarted {
  session: SessionInfo;
  turnId: string | null;
  sendError: string | null;
}

export interface RunOutcome {
  deliberation: Deliberation;
  started: RouteStarted | null;
}

// ---------------------------------------------------------------- memory ---
// packages/memory (ADR-0012).

export interface Project {
  id: string;
  path: string;
  name: string;
  createdAt: string;
  lastOpenedAt: string;
  stack: Record<string, unknown> | null;
}

export interface HistoryQuery {
  projectId?: string | null;
  kinds?: EventKind[];
  text?: string | null;
  hideReads?: boolean;
  /** Cursor from `HistoryPage.next`. */
  before?: string | null;
  limit?: number;
}

export interface HistoryPage {
  /** Oldest first. */
  events: AuditEvent[];
  /** Cursor of the previous (older) page. */
  next: string | null;
}

export type MemoryKind = "architecture" | "stack" | "convention" | "rule" | "note";
export type MemorySource = "user" | "agent" | "detector";

export interface MemoryEntry {
  id: string;
  projectId: string;
  kind: MemoryKind;
  title: string;
  content: string;
  tags: string[];
  pinned: boolean;
  source: MemorySource;
  createdAt: string;
  updatedAt: string;
}

export interface MemoryInput {
  id?: string | null;
  projectId: string;
  kind: MemoryKind;
  title: string;
  content: string;
  tags: string[];
  pinned: boolean;
}

export type DecisionStatus = "proposed" | "accepted" | "superseded" | "rejected";

export interface ProjectDecision {
  id: string;
  projectId: string;
  title: string;
  context: string;
  decision: string;
  consequences: string;
  status: DecisionStatus;
  source: MemorySource;
  createdAt: string;
  updatedAt: string;
}

export interface ProjectDecisionInput {
  id?: string | null;
  projectId: string;
  title: string;
  context: string;
  decision: string;
  consequences: string;
  status: DecisionStatus;
}

export interface WorkingSession {
  id: string;
  title: string;
  provider: string;
  model: string | null;
  status: SessionStatus;
  turns: number;
  updatedAt: string;
}

export interface WorkingFile {
  path: string;
  change: string;
  at: string;
  by: string;
}

export interface WorkingCommand {
  command: string;
  exitCode: number | null;
  background: boolean;
  at: string;
  by: string;
}

export interface WorkingError {
  kind: EventKind;
  summary: string;
  detail: string | null;
  at: string;
}

export interface WorkingMemory {
  sessions: WorkingSession[];
  files: WorkingFile[];
  commands: WorkingCommand[];
  errors: WorkingError[];
}

export interface MemoryOverview {
  project: Project;
  working: WorkingMemory;
  memoryEntries: number;
  decisions: number;
  sessions: number;
  events: number;
}

export interface SearchHit {
  kind: "memory" | "decision" | "message" | "event";
  refId: string;
  title: string;
  /** Matching terms between `[` and `]`. */
  snippet: string;
  at: string;
}
