import { describe, expect, it } from "vitest";
import { applyEvent, cacheShare, emptyTranscript, formatUsage, fromSnapshot, type Transcript } from "./transcript";
import type { SessionEvent, SessionInfo, TokenUsage, ToolResult } from "./types";

const AT = "2026-09-28T12:00:00Z";
const usage: TokenUsage = {
  inputTokens: 3,
  outputTokens: 4,
  cachedInputTokens: 0,
  reasoningTokens: 0,
  costUsd: null,
  estimated: true,
};

function apply(events: SessionEvent[], start: Transcript = emptyTranscript, firstSeq = 1): Transcript {
  return events.reduce((t, event, i) => applyEvent(t, firstSeq + i, AT, event), start);
}

const result: ToolResult = {
  callId: "c1",
  tool: "filesystem.read",
  ok: true,
  output: { content: "x" },
  error: null,
  startedAt: AT,
  finishedAt: AT,
  durationMs: 2,
};

const turn: SessionEvent[] = [
  { type: "turnStarted", turnId: "t1", input: "oi" },
  { type: "statusChanged", status: "running" },
  { type: "textDelta", turnId: "t1", text: "Eco: " },
  { type: "textDelta", turnId: "t1", text: "oi" },
  {
    type: "toolCallRequested",
    turnId: "t1",
    call: { id: "c1", tool: "filesystem.read", args: { path: "a" }, origin: { type: "agent", agentId: "s1" } },
  },
  { type: "toolCallCompleted", turnId: "t1", result },
  { type: "textDelta", turnId: "t1", text: "fim" },
  { type: "usage", turnId: "t1", usage },
  { type: "turnCompleted", turnId: "t1", status: "completed", error: null, usage, durationMs: 12, toolCalls: 1 },
  { type: "statusChanged", status: "idle" },
];

describe("transcript", () => {
  it("folds a turn into items", () => {
    const t = apply(turn);
    expect(t.items.map((i) => i.kind)).toEqual(["user", "assistant", "tool", "assistant", "turnEnd"]);
    expect(t.items[1]).toMatchObject({ kind: "assistant", text: "Eco: oi" });
    expect(t.items[2]).toMatchObject({ kind: "tool", tool: "filesystem.read", result: { ok: true } });
    expect(t.items[3]).toMatchObject({ kind: "assistant", text: "fim" });
    expect(t.status).toBe("idle");
    expect(t.runningTurn).toBeNull();
    expect(t.lastSeq).toBe(turn.length);
  });

  it("tracks the running turn", () => {
    const t = apply(turn.slice(0, 3));
    expect(t.status).toBe("running");
    expect(t.runningTurn).toBe("t1");
  });

  it("ignores events already applied (snapshot + live merge)", () => {
    const partial = apply(turn.slice(0, 4));
    // Live events 3..6 arrive again after the snapshot: 3 and 4 are skipped.
    const merged = turn.slice(2, 6).reduce((t, event, i) => applyEvent(t, 3 + i, AT, event), partial);
    expect(merged.items.map((i) => i.kind)).toEqual(["user", "assistant", "tool"]);
    expect(merged.items[1]).toMatchObject({ text: "Eco: oi" });
    expect(merged.items[2]).toMatchObject({ result: { callId: "c1" } });
  });

  it("builds from a snapshot with merged fragments", () => {
    const info = { status: "idle" } as SessionInfo;
    const t = fromSnapshot({
      info,
      entries: [
        { seq: 1, at: AT, event: { type: "turnStarted", turnId: "t1", input: "oi" } },
        // Fragments 2..4 merged by the backend into one entry with seq 4.
        { seq: 4, at: AT, event: { type: "textDelta", turnId: "t1", text: "Eco: oi" } },
      ],
      lastSeq: 4,
      truncated: false,
    });
    expect(t.lastSeq).toBe(4);
    expect(t.status).toBe("idle");
    const live = applyEvent(t, 4, AT, { type: "textDelta", turnId: "t1", text: "oi" });
    expect(live).toBe(t);
    const next = applyEvent(t, 5, AT, { type: "textDelta", turnId: "t1", text: "!" });
    expect(next.items[1]).toMatchObject({ text: "Eco: oi!" });
  });

  it("keeps tool results whose request was truncated away", () => {
    const t = apply([{ type: "toolCallCompleted", turnId: "t1", result }]);
    expect(t.items[0]).toMatchObject({ kind: "tool", callId: "c1", args: null, result: { ok: true } });
  });

  it("records subagents and notices", () => {
    const t = apply([
      { type: "subagentSpawned", childId: "s2", provider: "echo", title: "sub" },
      { type: "notice", turnId: null, level: "warning", message: "cuidado" },
    ]);
    expect(t.items.map((i) => i.kind)).toEqual(["subagent", "notice"]);
  });

  it("records the attached context and handoffs", () => {
    const summary = { tokens: 420, budget: 1500, sections: [], omitted: [], handoffId: "h1" };
    const t = apply([
      { type: "handedOff", handoffId: "h1", fromSession: "s1", toSession: "s2", provider: "echo" },
      { type: "turnStarted", turnId: "t1", input: "continue" },
      { type: "contextAttached", turnId: "t1", summary },
    ]);
    expect(t.items.map((i) => i.kind)).toEqual(["handoff", "user", "context"]);
    expect(t.items[0]).toMatchObject({ fromSession: "s1", toSession: "s2" });
    expect(t.items[2]).toMatchObject({ kind: "context", summary: { tokens: 420 } });
  });

  it("formats usage", () => {
    expect(formatUsage(usage)).toBe("7 tokens (3 in / 4 out) · estimado");
    expect(formatUsage({ ...usage, inputTokens: 1, outputTokens: 0, estimated: false, costUsd: 0.0123 })).toBe(
      "1 token (1 in / 0 out) · US$ 0.0123",
    );
    const cached = { ...usage, inputTokens: 1000, outputTokens: 10, cachedInputTokens: 800, estimated: false };
    expect(formatUsage({ ...cached, costUsd: 0.002, cacheSavedUsd: 0.0029 })).toBe(
      "1.010 tokens (1.000 in / 10 out · 800 do cache) · US$ 0.0020 · cache economizou US$ 0.0029",
    );
    expect(formatUsage({ ...cached, costUsd: 0.01, cacheSavedUsd: -0.001 })).toContain("cache custou US$ 0.0010");
    expect(cacheShare(cached)).toBe(80);
    expect(cacheShare(usage)).toBe(0);
  });

  it("shows a compaction and hides the empty input of a compaction turn", () => {
    const t = apply([
      { type: "turnStarted", turnId: "t9", input: "" },
      {
        type: "compacted",
        turnId: "t9",
        automatic: false,
        beforeTokens: 152000,
        afterTokens: 2100,
        messages: 40,
        summary: "o que foi feito",
      },
    ]);
    expect(t.items.map((i) => i.kind)).toEqual(["compacted"]);
    expect(t.items[0]).toMatchObject({ automatic: false, messages: 40, summary: "o que foi feito" });
    expect(t.runningTurn).toBe("t9");
  });
});
