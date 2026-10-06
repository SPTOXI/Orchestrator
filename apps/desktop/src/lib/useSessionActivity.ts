// Live activity of every AI session (lib/activity.ts), for the whole app.

import { useEffect, useRef, useState } from "react";
import { applyActivity, emptyActivity, type ActivityState } from "./activity";
import { streamEvents } from "./events";

/** Streamed text moves the "last sign of life" this often, at most. */
const LIVENESS_STEP_MS = 1_000;

/** Whether `next` is worth a render: more than the clock of a delta. */
function changed(prev: ActivityState, next: ActivityState, sessionId: string): boolean {
  if (next === prev) return false;
  if (next.finished !== prev.finished) return true;
  const a = prev.running[sessionId];
  const b = next.running[sessionId];
  if (!a || !b) return a !== b;
  return (
    a.turnId !== b.turnId ||
    a.phase !== b.phase ||
    a.detail !== b.detail ||
    a.callId !== b.callId ||
    b.lastAt - a.lastAt >= LIVENESS_STEP_MS
  );
}

export function useSessionActivity(enabled: boolean): ActivityState {
  const [state, setState] = useState<ActivityState>(emptyActivity);
  const current = useRef(state);

  useEffect(() => {
    if (!enabled) return;
    return streamEvents.subscribe((event) => {
      if (event.type !== "session") return;
      const next = applyActivity(current.current, event.sessionId, event.event, Date.now());
      if (!changed(current.current, next, event.sessionId)) return;
      current.current = next;
      setState(next);
    });
  }, [enabled]);

  return state;
}

/** The current time, ticking every `ms` while `active`. */
export function useNow(active: boolean, ms = 1_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), ms);
    return () => window.clearInterval(timer);
  }, [active, ms]);
  return now;
}
