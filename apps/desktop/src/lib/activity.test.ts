import { describe, expect, it } from "vitest";
import {
  activityLabel,
  applyActivity,
  emptyActivity,
  formatElapsed,
  runningList,
  silence,
  type ActivityState,
} from "./activity";
import type { CallOrigin, SessionEvent, TokenUsage } from "./types";

const origin: CallOrigin = { type: "agent", agentId: "s1" };

const usage: TokenUsage = {
  inputTokens: 1,
  outputTokens: 1,
  cachedInputTokens: 0,
  reasoningTokens: 0,
  costUsd: null,
  estimated: true,
};

function apply(state: ActivityState, sessionId: string, steps: Array<[number, SessionEvent]>): ActivityState {
  return steps.reduce((s, [now, event]) => applyActivity(s, sessionId, event, now), state);
}

describe("session activity", () => {
  it("follows a turn from start to end", () => {
    let state = apply(emptyActivity, "s1", [[1_000, { type: "turnStarted", turnId: "t1", input: "oi" }]]);
    let activity = state.running.s1!;
    expect(activity).toMatchObject({ since: 1_000, phase: "waiting" });
    expect(activityLabel(activity)).toBe("aguardando a resposta da IA");

    state = apply(state, "s1", [
      [2_000, { type: "reasoningDelta", turnId: "t1", text: "hmm" }],
      [3_000, { type: "toolCallRequested", turnId: "t1", call: { id: "c1", tool: "filesystem.list", args: {}, origin } }],
    ]);
    activity = state.running.s1!;
    expect(activity).toMatchObject({ since: 1_000, lastAt: 3_000, phase: "tool", detail: "filesystem.list", callId: "c1" });
    expect(activityLabel(activity)).toBe("executando filesystem.list");
    expect(activityLabel(activity, true)).toBe("esperando sua autorização para filesystem.list");

    state = apply(state, "s1", [
      [
        4_000,
        {
          type: "toolCallCompleted",
          turnId: "t1",
          result: {
            callId: "c1",
            tool: "filesystem.list",
            ok: true,
            output: {},
            error: null,
            startedAt: "",
            finishedAt: "",
            durationMs: 1,
          },
        },
      ],
      [5_000, { type: "textDelta", turnId: "t1", text: "Pronto" }],
    ]);
    expect(activityLabel(state.running.s1!)).toBe("escrevendo a resposta");

    state = apply(state, "s1", [
      [
        6_000,
        { type: "turnCompleted", turnId: "t1", status: "completed", error: null, usage, durationMs: 5_000, toolCalls: 1 },
      ],
    ]);
    expect(state.running.s1).toBeUndefined();
    expect(state.finished).toMatchObject({ sessionId: "s1", status: "completed", durationMs: 5_000 });
  });

  it("says when the API is being asked again and when the AI is silent", () => {
    let state = apply(emptyActivity, "s1", [[0, { type: "turnStarted", turnId: "t1", input: "oi" }]]);
    expect(silence(state.running.s1!, 10_000)).toBeNull();
    expect(silence(state.running.s1!, 45_000)).toBe(45_000);
    state = apply(state, "s1", [
      [
        50_000,
        { type: "notice", turnId: "t1", level: "warning", message: "rate limit (HTTP 429) — tentando de novo em 2 s (1 de 2)" },
      ],
    ]);
    expect(state.running.s1).toMatchObject({ phase: "retrying", lastAt: 50_000 });
    // A tool that runs long is not "silence".
    state = apply(state, "s1", [
      [51_000, { type: "toolCallRequested", turnId: "t1", call: { id: "c2", tool: "shell.execute", args: {}, origin } }],
    ]);
    expect(silence(state.running.s1!, 200_000)).toBeNull();
  });

  it("tracks sessions apart and recovers a missed start", () => {
    let state = apply(emptyActivity, "a", [[5_000, { type: "turnStarted", turnId: "ta", input: "x" }]]);
    state = apply(state, "b", [[1_000, { type: "textDelta", turnId: "tb", text: "y" }]]);
    expect(runningList(state).map((a) => a.sessionId)).toEqual(["b", "a"]);
    state = apply(state, "a", [[6_000, { type: "statusChanged", status: "closed" }]]);
    expect(Object.keys(state.running)).toEqual(["b"]);
    // Events of another turn do not move the clock of this one.
    const before = state.running.b!;
    state = apply(state, "b", [[7_000, { type: "usage", turnId: "other", usage }]]);
    expect(state.running.b).toBe(before);
  });

  it("formats a running clock", () => {
    expect(formatElapsed(800)).toBe("0 s");
    expect(formatElapsed(8_400)).toBe("8 s");
    expect(formatElapsed(65_000)).toBe("1 min 05 s");
    expect(formatElapsed(3_720_000)).toBe("1 h 02 min");
  });
});
