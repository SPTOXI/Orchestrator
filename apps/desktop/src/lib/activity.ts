// What each AI session is doing right now, folded from the live session
// events. Shown while a turn runs — in the session, the sessions list, the
// tabs and the status bar — so the user can work elsewhere meanwhile and
// is told when the AI finishes.

import type { SessionEvent, TurnStatus } from "./types";

export type ActivityPhase = "waiting" | "thinking" | "writing" | "tool" | "retrying";

export interface SessionActivity {
  sessionId: string;
  turnId: string;
  /** When the turn started (ms). */
  since: number;
  /** Last sign of life from the turn (ms). */
  lastAt: number;
  phase: ActivityPhase;
  /** Tool being run (phase "tool") or the retry notice ("retrying"). */
  detail: string | null;
  /** Call id of the tool being run, to tell when it waits for the user. */
  callId: string | null;
}

export interface FinishedTurn {
  sessionId: string;
  turnId: string;
  status: TurnStatus;
  durationMs: number;
  error: string | null;
}

export interface ActivityState {
  /** Turns in progress, by session. */
  running: Record<string, SessionActivity>;
  /** The turn that ended last, if any (for the "terminou" notice). */
  finished: FinishedTurn | null;
}

export const emptyActivity: ActivityState = { running: {}, finished: null };

/** A silence this long, outside a tool, is worth pointing out. */
export const SILENCE_MS = 30_000;

export function applyActivity(state: ActivityState, sessionId: string, event: SessionEvent, now: number): ActivityState {
  const current = state.running[sessionId];
  const set = (activity: SessionActivity): ActivityState => ({
    ...state,
    running: { ...state.running, [sessionId]: activity },
  });
  // A live event for a turn whose start was missed still counts.
  const base = (turnId: string): SessionActivity =>
    current && current.turnId === turnId
      ? { ...current, lastAt: now }
      : { sessionId, turnId, since: now, lastAt: now, phase: "waiting", detail: null, callId: null };

  switch (event.type) {
    case "turnStarted":
      return set({ sessionId, turnId: event.turnId, since: now, lastAt: now, phase: "waiting", detail: null, callId: null });
    case "textDelta":
      return set({ ...base(event.turnId), phase: "writing", detail: null, callId: null });
    case "reasoningDelta":
      return set({ ...base(event.turnId), phase: "thinking", detail: null, callId: null });
    case "toolCallRequested":
      return set({ ...base(event.turnId), phase: "tool", detail: event.call.tool, callId: event.call.id });
    case "toolCallCompleted":
      return set({ ...base(event.turnId), phase: "waiting", detail: null, callId: null });
    case "usage":
      return current && current.turnId === event.turnId ? set({ ...current, lastAt: now }) : state;
    case "notice":
      if (!current || (event.turnId !== null && event.turnId !== current.turnId)) return state;
      if (event.level === "warning" && event.message.includes("tentando de novo")) {
        return set({ ...current, lastAt: now, phase: "retrying", detail: event.message, callId: null });
      }
      return set({ ...current, lastAt: now });
    case "turnCompleted": {
      const running = { ...state.running };
      delete running[sessionId];
      return {
        running,
        finished: {
          sessionId,
          turnId: event.turnId,
          status: event.status,
          durationMs: event.durationMs,
          error: event.error,
        },
      };
    }
    case "statusChanged":
      // Closed or idle without a turn end (e.g. the app lost the stream).
      if (event.status !== "running" && current) {
        const running = { ...state.running };
        delete running[sessionId];
        return { ...state, running };
      }
      return state;
    default:
      return state;
  }
}

/** What the AI is doing, in a few words. */
export function activityLabel(activity: SessionActivity, waitingForUser = false): string {
  switch (activity.phase) {
    case "tool":
      return waitingForUser
        ? `esperando sua autorização para ${activity.detail ?? "uma ferramenta"}`
        : `executando ${activity.detail ?? "uma ferramenta"}`;
    case "thinking":
      return "pensando";
    case "writing":
      return "escrevendo a resposta";
    case "retrying":
      return "a API recusou; tentando de novo";
    case "waiting":
      return "aguardando a resposta da IA";
  }
}

/** `8 s`, `1 min 05 s`, `1 h 02 min`: a running clock, whole seconds. */
export function formatElapsed(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min ${String(seconds % 60).padStart(2, "0")} s`;
  return `${Math.floor(minutes / 60)} h ${String(minutes % 60).padStart(2, "0")} min`;
}

/** How long the AI has been silent, when that is worth saying. */
export function silence(activity: SessionActivity, now: number): number | null {
  if (activity.phase === "tool") return null;
  const quiet = now - activity.lastAt;
  return quiet >= SILENCE_MS ? quiet : null;
}

/** Running turns, the oldest first. */
export function runningList(state: ActivityState): SessionActivity[] {
  return Object.values(state.running).sort((a, b) => a.since - b.since);
}
