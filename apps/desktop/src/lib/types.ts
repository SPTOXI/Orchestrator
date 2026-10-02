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
  | "LOCKED"
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
  | "DECISION_SAVED"
  | "CONTEXT_BUILT"
  | "TASK_UPDATED"
  | "AUTONOMY_CHANGED"
  | "APPROVAL_REQUESTED"
  | "APPROVAL_DECIDED"
  | "EXECUTION_PAUSED"
  | "EXECUTION_RESUMED"
  | "GITHUB_PR_CREATED"
  | "GITHUB_PR_MERGED"
  | "GITHUB_ISSUE_CREATED"
  | "CONTEXT_COMPACTED"
  | "APP_UPDATED";

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
  /** The whole prompt, cache reads and writes included. */
  inputTokens: number;
  outputTokens: number;
  /** Read from the provider's prompt cache. */
  cachedInputTokens: number;
  /** Written to the prompt cache (Anthropic; ADR-0018). */
  cacheWriteTokens?: number;
  reasoningTokens: number;
  costUsd: number | null;
  /** What the cache saved (negative: a write never read back). */
  cacheSavedUsd?: number | null;
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
  | { type: "subagentSpawned"; childId: string; provider: string; title: string }
  | { type: "contextAttached"; turnId: string; summary: ContextSummary }
  | {
      type: "compacted";
      turnId: string;
      automatic: boolean;
      beforeTokens: number;
      afterTokens: number;
      messages: number;
      summary: string;
    }
  | { type: "handedOff"; handoffId: string; fromSession: string; toSession: string; provider: string };

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
  /** Compacts its conversation into a summary (ADR-0018). */
  compaction?: boolean;
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
  /** Project context of the session (ADR-0013). */
  context?: ContextOptions;
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
  /** USD per million input tokens read from the prompt cache (ADR-0018). */
  cachedInputPrice?: number | null;
  tags: string[];
  extraBody: unknown;
  enabled: boolean;
}

/** Lifetime of an Anthropic prompt cache entry. */
export type CacheTtl = "5m" | "1h";

export interface ProtocolOptions {
  streamUsage: boolean | null;
  eagerToolStreaming: boolean | null;
  refusalFallback: boolean | null;
  /** Prompt cache markers / key (default on). */
  promptCache?: boolean | null;
  cacheTtl?: CacheTtl | null;
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
  options: ProtocolOptions;
  generic: GenericProfile | null;
  enabled: boolean;
  notes: string | null;
  /** Seconds a streamed request waits for the server to start; 0 = no limit. Default 120. */
  firstResponseSecs?: number | null;
  /** Connection that answers when this one is overloaded or not responding. */
  fallback?: { connection: string; model: string | null } | null;
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
  kind: "memory" | "decision" | "message" | "event" | "handoff" | "task";
  refId: string;
  title: string;
  /** Matching terms between `[` and `]`. */
  snippet: string;
  at: string;
}

// ------------------------------------------ context and handoff (ADR-0013) ---

export interface ContextOptions {
  /** false: no project context; null/omitted: the app setting. */
  enabled?: boolean | null;
  budget?: number | null;
  handoffId?: string | null;
}

export interface ContextSectionSummary {
  kind: SectionKind;
  title: string;
  items: number;
  tokens: number;
}

/** What was attached to a session (the text stays with the session). */
export interface ContextSummary {
  tokens: number;
  budget: number;
  sections: ContextSectionSummary[];
  omitted: string[];
  handoffId: string | null;
}

export type SectionKind = "task" | "working" | "project" | "files" | "errors" | "history" | "git" | "handoff";

export interface ContextSection {
  kind: SectionKind;
  title: string;
  items: string[];
  /** Items found before the budget. */
  found: number;
  tokens: number;
}

export interface ContextPack {
  project: Project | null;
  sections: ContextSection[];
  tokens: number;
  budget: number;
  omitted: string[];
  notes: string[];
  text: string;
}

export interface PreviewRequest {
  projectPath?: string;
  task?: string;
  handoffId?: string;
  budget?: number;
  sessionId?: string;
}

/** When a session's conversation is compacted (ADR-0018). */
export interface CompactionPolicy {
  auto: boolean;
  thresholdTokens: number;
  thresholdPercent: number;
}

export interface ContextSettings {
  autoAttach: boolean;
  budgetTokens: number;
  compaction: CompactionPolicy;
}

export interface ContextSettingsView {
  settings: ContextSettings;
  minBudget: number;
  maxBudget: number;
  defaultBudget: number;
  warning: string | null;
}

export interface HandoffPacket {
  goal: string;
  status: string;
  completed: string[];
  remaining: string[];
  files: string[];
  commands: string[];
  errors: string[];
  decisions: string[];
  tests: string[];
  nextAction: string;
}

export interface HandoffEnd {
  sessionId: string;
  provider: string;
  model: string | null;
  title: string;
}

export type HandoffStatus = "created" | "accepted";

export interface Handoff {
  id: string;
  projectId: string | null;
  projectPath: string;
  from: HandoffEnd;
  to: HandoffEnd | null;
  packet: HandoffPacket;
  status: HandoffStatus;
  byAgent: boolean;
  createdAt: string;
  acceptedAt: string | null;
}

export interface HandoffDraft {
  from: HandoffEnd;
  projectPath: string;
  packet: HandoffPacket;
  byAgent: boolean;
  notes: string[];
  usage: TokenUsage | null;
}

export interface StartHandoff {
  handoffId: string;
  provider: string;
  model?: string | null;
  title?: string | null;
  budget?: number | null;
}

export interface StartedHandoff {
  handoff: Handoff;
  session: SessionInfo;
  turnId: string | null;
  sendError: string | null;
}

/** Tasks of the project (ADR-0014). */
export type TaskStatus = "TODO" | "IN_PROGRESS" | "BLOCKED" | "REVIEW" | "DONE" | "CANCELLED";

export type TaskPriority = "LOW" | "NORMAL" | "HIGH" | "URGENT";

export interface Task {
  id: string;
  projectId: string;
  title: string;
  description: string;
  status: TaskStatus;
  priority: TaskPriority;
  provider: string | null;
  model: string | null;
  /** Who executes it; only from Fase 8b on. */
  agent: string | null;
  parentTask: string | null;
  dependencies: string[];
  files: string[];
  sessions: string[];
  result: string;
  createdAt: string;
  updatedAt: string;
  startedAt: string | null;
  finishedAt: string | null;
}

export interface TaskRef {
  id: string;
  title: string;
  status: TaskStatus;
}

/** A task with what the panel needs but the task does not store. */
export interface TaskView extends Task {
  /** Dependencies not done yet: while there is one, the task cannot start. */
  waitingFor: TaskRef[];
  subtasks: { done: number; total: number };
  /** States this task may move to now, as the engine allows them. */
  can: TaskStatus[];
}

/** What `task_save` receives; only what changed needs to go. */
export interface TaskInput {
  id?: string;
  projectId?: string;
  title?: string;
  description?: string;
  priority?: TaskPriority;
  provider?: string;
  model?: string;
  parentTask?: string;
  dependencies?: string[];
  files?: string[];
  result?: string;
}

export interface StartTaskSession {
  taskId: string;
  provider?: string | null;
  model?: string | null;
  budget?: number | null;
}

export interface StartedTask {
  task: Task;
  session: SessionInfo;
  turnId: string | null;
  sendError: string | null;
}

/** Agents executing tasks (ADR-0015). */
export type AgentStatus = "QUEUED" | "RUNNING" | "DONE" | "FAILED" | "STOPPED";

export interface Agent {
  id: string;
  projectId: string;
  /** Task it executes. */
  task: string;
  title: string;
  provider: string;
  model: string | null;
  /** Session it works in; null while it is queued. */
  session: string | null;
  parentAgent: string | null;
  status: AgentStatus;
  /** Tools offered to it when it started. */
  tools: string[];
  /** Project context it received on its first turn. */
  context: ContextSummary | null;
  turns: number;
  maxTurns: number;
  /** Paths it holds while it works. */
  files: string[];
  result: string;
  error: string | null;
  /** Handoff created when it stopped before finishing. */
  handoff: string | null;
  /** Mode the user granted to this agent; null: the project's (ADR-0016). */
  autonomy: AutonomyMode | null;
  /** What it may spend (USD) before it stops; null: no ceiling (ADR-0018). */
  maxCostUsd?: number | null;
  createdAt: string;
  updatedAt: string;
  startedAt: string | null;
  finishedAt: string | null;
}

/** An agent with what the panel and the board show. */
export interface AgentView extends Agent {
  /** Why a queued agent has not started yet. */
  waiting: string | null;
  taskTitle: string;
  taskStatus: TaskStatus;
  /** Held by a pause (its own, or of every AI). */
  paused: boolean;
  /** What it waits for the user to authorize. */
  approval: string | null;
  /** Mode its calls are judged by now. */
  mode: AutonomyMode;
  /** What its session cost so far (null: no price, or not started). */
  costUsd: number | null;
  /** 1 = next to start, while queued. */
  queuePosition: number | null;
}

/** What the project's AIs spent today against its daily budget. */
export interface BudgetView {
  spentTodayUsd: number;
  budgetUsd: number | null;
  /** Calls with tokens but no price: the real spending is higher. */
  unpriced: number;
  exhausted: boolean;
}

/** Spending of one provider and model (ADR-0018). */
export interface SpendRow {
  provider: string;
  model: string | null;
  calls: number;
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number;
  cacheWriteTokens: number;
  costUsd: number;
  cacheSavedUsd: number;
  unpriced: number;
}

export interface SpendReport {
  since: string;
  costUsd: number;
  cacheSavedUsd: number;
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number;
  cacheWriteTokens: number;
  calls: number;
  unpriced: number;
  compactions: number;
  rows: SpendRow[];
}

/** A file held by an agent while it works. */
export interface FileLock {
  projectId: string;
  path: string;
  agentId: string;
  agentTitle: string;
  task: string;
  at: string;
}

export interface StartAgent {
  taskId: string;
  provider?: string | null;
  model?: string | null;
  maxTurns?: number | null;
  /** Mode granted to this agent and its subagents; null: the project's. */
  autonomy?: AutonomyMode | null;
  /** Cost ceiling of this agent (USD); null: the setting. */
  maxCostUsd?: number | null;
}

export interface AgentSettings {
  /** Agents running at the same time (1–8). */
  maxParallel: number;
  /** Turns an agent may spend before it stops on its own (1–50). */
  maxTurns: number;
  /** Subagents one agent may create (0–10; 0 = no delegation). */
  maxSubagents: number;
  /** Agents of one provider running at the same time. */
  providerLimits: Record<string, number>;
  /** What one agent may spend (USD); null = no ceiling. */
  maxCostUsd: number | null;
  /** What a project's AIs may spend per day (USD); null = no budget. */
  dailyBudgetUsd: number | null;
}

/** Autonomy (ADR-0016). */
export type AutonomyMode = "assisted" | "autonomous" | "unrestricted";
/** What a rule decides (not the router's `Decision`). */
export type RuleDecision = "allow" | "ask" | "deny";
export type RuleAccess = "read" | "write";
export type RuleWhere = "inside" | "outside";

/** One rule; empty fields match everything; the first that matches decides. */
export interface PolicyRule {
  tools: string[];
  access?: RuleAccess | null;
  where?: RuleWhere | null;
  command?: string | null;
  path?: string | null;
  decision: RuleDecision;
  note?: string | null;
}

export interface ApprovalRequest {
  id: string;
  callId: string;
  tool: string;
  summary: string;
  detail: string | null;
  reason: string;
  mode: AutonomyMode;
  rule: number | null;
  command: string | null;
  sessionId: string;
  agentId: string | null;
  agentTitle: string | null;
  taskId: string | null;
  projectId: string | null;
  provider: string | null;
  requestedAt: string;
}

export interface ApprovalView extends ApprovalRequest {
  projectName: string | null;
}

export type ApprovalAnswer = "approve" | "approveSession" | "deny";

export interface SessionGrant {
  id: string;
  sessionId: string;
  tool: string;
  mode: AutonomyMode;
  rule: number | null;
  command: string | null;
  agentTitle: string | null;
  grantedAt: string;
}

export interface AutonomyOverview {
  projectId: string | null;
  mode: AutonomyMode;
  projectMode: AutonomyMode | null;
  defaultMode: AutonomyMode;
  rules: PolicyRule[];
  assistedRules: PolicyRule[];
  defaultRules: PolicyRule[];
  pausedAll: boolean;
  pausedAgents: string[];
  pending: number;
  grants: SessionGrant[];
  warning: string | null;
}

export interface TrialTarget {
  path: string;
  inside: boolean;
  command: string | null;
  decision: RuleDecision;
  rule: number | null;
}

export interface Trial {
  mode: AutonomyMode;
  decision: RuleDecision;
  rule: number | null;
  reason: string;
  targets: TrialTarget[];
  opaque: boolean;
  unknownTool: boolean;
}

/** GitHub (ADR-0017). */
export interface GitHubAccount {
  login: string;
  name: string | null;
  url: string;
  scopes: string[];
}

export interface GitHubRepoRef {
  host: string;
  owner: string;
  name: string;
}

export interface GitHubRepo {
  owner: string;
  name: string;
  fullName: string;
  url: string;
  defaultBranch: string;
  private: boolean;
  description: string | null;
}

export type CheckState = "success" | "failure" | "pending" | "none";

export interface CheckItem {
  name: string;
  state: "success" | "failure" | "pending" | "skipped";
  kind: "check" | "status";
  url: string | null;
  description: string | null;
}

export interface Checks {
  state: CheckState;
  total: number;
  passed: number;
  failed: number;
  pending: number;
  items: CheckItem[];
}

export interface PullSummary {
  number: number;
  title: string;
  state: "open" | "closed" | "merged";
  draft: boolean;
  author: string;
  head: string;
  headSha: string;
  base: string;
  url: string;
  createdAt: string;
  updatedAt: string;
}

export interface PullReview {
  author: string;
  state: "approved" | "changes_requested" | "commented" | "dismissed";
  body: string;
  submittedAt: string | null;
}

export interface GitHubComment {
  author: string;
  body: string;
  url: string;
  createdAt: string;
}

export interface PullDetail extends PullSummary {
  body: string;
  merged: boolean;
  mergedAt: string | null;
  mergeable: boolean | null;
  mergeableState: string | null;
  commits: number;
  additions: number;
  deletions: number;
  changedFiles: number;
  checks: Checks;
  reviews: PullReview[];
  comments: GitHubComment[];
}

export interface MergeResult {
  number: number;
  title: string;
  url: string;
  merged: boolean;
  sha: string;
  message: string;
  branchDeleted: boolean;
  branchError: string | null;
}

export interface IssueSummary {
  number: number;
  title: string;
  state: "open" | "closed";
  author: string;
  labels: string[];
  commentCount: number;
  url: string;
  createdAt: string;
  updatedAt: string;
}

export interface GitHubStatus {
  host: string;
  apiUrl: string;
  authenticated: boolean;
  tokenSource: string | null;
  account: GitHubAccount | null;
  accountError: string | null;
  repo: GitHubRepo | null;
  repoRef: GitHubRepoRef | null;
  remote: string | null;
  repoError: string | null;
  branch: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  pull: PullSummary | null;
  checks: Checks | null;
}

export type MergeMethod = "merge" | "squash" | "rebase";

/** What the GitHub tab shows about the connection (never the token). */
export interface GitHubSetup {
  host: string;
  apiUrl: string | null;
  apiBase: string;
  vaultToken: boolean;
  vault: string;
  envToken: string | null;
  ghInstalled: boolean;
  warning: string | null;
}

// --------------------------------------------------------------- secrets ---

/** A secret the AIs use by name (ADR-0020); never its value. */
export interface SecretItem {
  name: string;
  updatedAt: string;
  /** Its value was read from the vault. */
  loaded: boolean;
  /** What the AIs write: {{secret:NAME}}. */
  placeholder: string;
}

export interface SecretsView {
  secrets: SecretItem[];
  vault: string;
  warning: string | null;
}

// --------------------------------------------------------------- updates ---

/** A newer version the update endpoint offers (ADR-0019). */
export interface UpdateInfo {
  version: string;
  currentVersion: string;
  /** Release date (RFC 3339), when the manifest says. */
  date: string | null;
  notes: string | null;
}

export type UpdatePhase = "idle" | "checking" | "downloading" | "installed";

export interface UpdateStatus {
  version: string;
  commit: string | null;
  os: string;
  arch: string;
  /** How the app was installed (deb, rpm, appimage, msi, nsis, app); null when it runs from a build. */
  bundle: string | null;
  /** This build looks for updates (release builds carry the key). */
  configured: boolean;
  notConfigured: string | null;
  endpoint: string | null;
  autoCheck: boolean;
  lastCheck: string | null;
  lastError: string | null;
  phase: UpdatePhase;
  available: UpdateInfo | null;
  warning: string | null;
}

/** Pushed on `runtime://update`. */
export type UpdateEvent =
  | { kind: "checked" }
  | { kind: "available"; info: UpdateInfo }
  | { kind: "progress"; downloaded: number; total: number | null }
  | { kind: "installed"; version: string }
  | { kind: "failed"; message: string };

// ---- Development rules and skills (ADR-0021) ----------------------------

export interface GuidanceSettings {
  rulesEnabled: boolean;
  projectRuleFiles: boolean;
  skillsEnabled: boolean;
  projectSkills: boolean;
  claudeUserSkills: boolean;
  disabledSkills: string[];
}

export type SkillSource = "orchestrator" | "project" | "claudeUser";

export interface SkillInfo {
  name: string;
  description: string;
  source: SkillSource;
  path: string;
  enabled: boolean;
  shadowed: boolean;
}

export interface SkillDoc extends SkillInfo {
  body: string;
  files: string[];
}

export interface RuleFile {
  name: string;
  path: string;
  chars: number;
  truncated: boolean;
}

export interface GuidanceView {
  settings: GuidanceSettings;
  userRules: string;
  rulesPath: string;
  skillsDir: string;
  projectFiles: RuleFile[];
  skills: SkillInfo[];
  projectPath: string | null;
}

// ---- Offline models (ADR-0021) ------------------------------------------

export interface OllamaStatus {
  url: string;
  running: boolean;
  version: string | null;
  program: string | null;
  error: string | null;
}

export interface LocalModel {
  name: string;
  sizeBytes: number;
  family: string | null;
  parameterSize: string | null;
  quantization: string | null;
  modifiedAt: string | null;
  tools: boolean | null;
  vision: boolean | null;
  contextWindow: number | null;
}

export interface CatalogModel {
  name: string;
  label: string;
  sizeGb: number;
  memoryGb: number;
  tools: boolean;
  note: string;
}

export interface OfflineView {
  status: OllamaStatus;
  models: LocalModel[];
  catalog: CatalogModel[];
  connection: string | null;
  downloading: string[];
  installCommand: string | null;
  downloadPage: string;
  modelsError: string | null;
}

export interface PullProgress {
  model: string;
  status: string;
  completed: number | null;
  total: number | null;
}

/** Pushed on `runtime://offline`. */
export type OfflineEvent =
  | ({ type: "progress" } & PullProgress)
  | { type: "done"; model: string }
  | { type: "failed"; model: string; error: string };

// ---- MCP servers (ADR-0021) ---------------------------------------------

export type McpTransport = "stdio" | "http";

export interface McpServerConfig {
  id: string;
  name: string;
  transport: McpTransport;
  command: string;
  args: string[];
  env: Record<string, string>;
  cwd: string | null;
  url: string;
  headers: Record<string, string>;
  enabled: boolean;
  timeoutSecs: number | null;
  disabledTools: string[];
}

export type McpStatus = "disabled" | "starting" | "ready" | "failed";

export interface McpToolView {
  name: string;
  remote: string;
  description: string;
  readOnly: boolean;
  enabled: boolean;
}

export interface McpServerView {
  config: McpServerConfig;
  status: McpStatus;
  error: string | null;
  serverName: string | null;
  serverVersion: string | null;
  tools: McpToolView[];
  log: string[];
}

// ---- Subscriptions through CLIs (ADR-0021) ------------------------------

export type CliKind = "claudeCode" | "codex" | "gemini";

export interface CliSettings {
  enabled: boolean;
  program: string | null;
  models: string[];
  defaultModel: string | null;
  ownTools: boolean;
  extraArgs: string[];
}

export interface CliStatus {
  kind: CliKind;
  id: string;
  name: string;
  subscription: string;
  program: string | null;
  version: string | null;
  loggedIn: boolean | null;
  detail: string | null;
  installCommand: string;
  loginCommand: string;
  loginHint: string;
  suggestedModels: string[];
  settings: CliSettings;
}
