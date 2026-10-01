// Session transcript view: folds numbered session events (snapshot from
// `session_get` + live `session` stream events) into display items.
//
// Every event carries `seq`. Events with `seq <= lastSeq` are already part of
// the view and are ignored, so a snapshot and the live stream can be merged
// without gaps or duplicates (same idea as OutputSync, ADR-0009).

import type {
  ContextSummary,
  NoticeLevel,
  SessionEvent,
  SessionSnapshot,
  SessionStatus,
  TokenUsage,
  ToolResult,
  TurnStatus,
} from "./types";

export type TranscriptItem =
  | { kind: "user"; key: string; turnId: string; text: string; at: string }
  | { kind: "assistant"; key: string; turnId: string; text: string }
  | { kind: "reasoning"; key: string; turnId: string; text: string }
  | { kind: "tool"; key: string; turnId: string; callId: string; tool: string; args: unknown; result: ToolResult | null }
  | { kind: "notice"; key: string; level: NoticeLevel; message: string }
  | {
      kind: "turnEnd";
      key: string;
      turnId: string;
      status: TurnStatus;
      error: string | null;
      usage: TokenUsage;
      durationMs: number;
      toolCalls: number;
    }
  | { kind: "subagent"; key: string; childId: string; provider: string; title: string }
  | { kind: "context"; key: string; turnId: string; summary: ContextSummary }
  | {
      kind: "compacted";
      key: string;
      turnId: string;
      automatic: boolean;
      beforeTokens: number;
      afterTokens: number;
      messages: number;
      summary: string;
    }
  | {
      kind: "handoff";
      key: string;
      handoffId: string;
      fromSession: string;
      toSession: string;
      provider: string;
    };

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;

function isTool(item: TranscriptItem): item is ToolItem {
  return item.kind === "tool";
}

/** Index of the newest tool item for `callId`, or -1. */
function lastToolIndex(items: TranscriptItem[], callId: string): number {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item && isTool(item) && item.callId === callId) return index;
  }
  return -1;
}

export interface Transcript {
  items: TranscriptItem[];
  /** Highest `seq` applied. */
  lastSeq: number;
  /** Latest status seen in the events (null before any). */
  status: SessionStatus | null;
  /** Turn in progress, if any. */
  runningTurn: string | null;
}

export const emptyTranscript: Transcript = { items: [], lastSeq: 0, status: null, runningTurn: null };

/** Appends `text` to the last item when it is the same kind and turn. */
function appendText(
  items: TranscriptItem[],
  kind: "assistant" | "reasoning",
  key: string,
  turnId: string,
  text: string,
): TranscriptItem[] {
  const last = items[items.length - 1];
  if (last && last.kind === kind && last.turnId === turnId) {
    return [...items.slice(0, -1), { ...last, text: last.text + text }];
  }
  return [...items, { kind, key, turnId, text }];
}

export function applyEvent(transcript: Transcript, seq: number, at: string, event: SessionEvent): Transcript {
  if (seq <= transcript.lastSeq) return transcript;
  const key = `e${seq}`;
  let { items, status, runningTurn } = transcript;
  switch (event.type) {
    case "turnStarted":
      // A turn without input only compacts the conversation ("Compactar").
      if (event.input) items = [...items, { kind: "user", key, turnId: event.turnId, text: event.input, at }];
      runningTurn = event.turnId;
      break;
    case "textDelta":
      items = appendText(items, "assistant", key, event.turnId, event.text);
      break;
    case "reasoningDelta":
      items = appendText(items, "reasoning", key, event.turnId, event.text);
      break;
    case "toolCallRequested":
      items = [
        ...items,
        {
          kind: "tool",
          key,
          turnId: event.turnId,
          callId: event.call.id,
          tool: event.call.tool,
          args: event.call.args,
          result: null,
        },
      ];
      break;
    case "toolCallCompleted": {
      const index = lastToolIndex(items, event.result.callId);
      if (index >= 0) {
        const item = items[index] as ToolItem;
        items = [...items.slice(0, index), { ...item, result: event.result }, ...items.slice(index + 1)];
      } else {
        // The request fell out of a truncated transcript.
        items = [
          ...items,
          {
            kind: "tool",
            key,
            turnId: event.turnId,
            callId: event.result.callId,
            tool: event.result.tool,
            args: null,
            result: event.result,
          },
        ];
      }
      break;
    }
    case "usage":
      // Included in the turn total (turnCompleted).
      break;
    case "notice":
      items = [...items, { kind: "notice", key, level: event.level, message: event.message }];
      break;
    case "turnCompleted":
      items = [
        ...items,
        {
          kind: "turnEnd",
          key,
          turnId: event.turnId,
          status: event.status,
          error: event.error,
          usage: event.usage,
          durationMs: event.durationMs,
          toolCalls: event.toolCalls,
        },
      ];
      if (runningTurn === event.turnId) runningTurn = null;
      break;
    case "statusChanged":
      status = event.status;
      if (event.status !== "running") runningTurn = null;
      break;
    case "subagentSpawned":
      items = [...items, { kind: "subagent", key, childId: event.childId, provider: event.provider, title: event.title }];
      break;
    case "contextAttached":
      items = [...items, { kind: "context", key, turnId: event.turnId, summary: event.summary }];
      break;
    case "compacted":
      items = [
        ...items,
        {
          kind: "compacted",
          key,
          turnId: event.turnId,
          automatic: event.automatic,
          beforeTokens: event.beforeTokens,
          afterTokens: event.afterTokens,
          messages: event.messages,
          summary: event.summary,
        },
      ];
      break;
    case "handedOff":
      items = [
        ...items,
        {
          kind: "handoff",
          key,
          handoffId: event.handoffId,
          fromSession: event.fromSession,
          toSession: event.toSession,
          provider: event.provider,
        },
      ];
      break;
  }
  return { items, lastSeq: seq, status, runningTurn };
}

/** Transcript of a `session_get` snapshot. */
export function fromSnapshot(snapshot: SessionSnapshot): Transcript {
  let transcript = emptyTranscript;
  for (const entry of snapshot.entries) transcript = applyEvent(transcript, entry.seq, entry.at, entry.event);
  return {
    ...transcript,
    lastSeq: Math.max(transcript.lastSeq, snapshot.lastSeq),
    status: snapshot.info.status,
    runningTurn: snapshot.info.status === "running" ? transcript.runningTurn : null,
  };
}

/** "1.234 tokens" */
export function formatTokens(usage: TokenUsage): string {
  const total = usage.inputTokens + usage.outputTokens;
  return `${total.toLocaleString("pt-BR")} ${total === 1 ? "token" : "tokens"}`;
}

/** "7 tokens (3 in / 4 out · 2 do cache) · estimado · US$ 0.0012 · cache economizou US$ 0.0004" */
export function formatUsage(usage: TokenUsage): string {
  const tokens = formatTokens(usage);
  const cached = usage.cachedInputTokens
    ? ` · ${usage.cachedInputTokens.toLocaleString("pt-BR")} do cache`
    : "";
  const detail = `${usage.inputTokens.toLocaleString("pt-BR")} in / ${usage.outputTokens.toLocaleString("pt-BR")} out${cached}`;
  const cost = usage.costUsd !== null ? ` · US$ ${usage.costUsd.toFixed(4)}` : "";
  const saved =
    usage.cacheSavedUsd !== null && usage.cacheSavedUsd !== undefined && Math.abs(usage.cacheSavedUsd) >= 0.00005
      ? ` · cache ${usage.cacheSavedUsd >= 0 ? "economizou" : "custou"} US$ ${Math.abs(usage.cacheSavedUsd).toFixed(4)}`
      : "";
  return `${tokens} (${detail})${usage.estimated ? " · estimado" : ""}${cost}${saved}`;
}

/** Share of the prompt read from the cache, 0–100 (null with no prompt). */
export function cacheShare(usage: TokenUsage): number | null {
  if (!usage.inputTokens) return null;
  return Math.round((usage.cachedInputTokens / usage.inputTokens) * 100);
}
