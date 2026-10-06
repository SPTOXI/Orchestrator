// The autonomy of the open project and the requests waiting for the user
// (ADR-0016), refreshed when the history says something changed. A request
// can arrive at any moment, from any screen, so this lives at the top.

import { useCallback, useEffect, useRef, useState } from "react";
import { auditEvents } from "./events";
import { autonomyApi, errorMessage } from "./runtime";
import type { ApprovalAnswer, ApprovalView, AutonomyMode, AutonomyOverview, PolicyRule } from "./types";

export interface Autonomy {
  overview: AutonomyOverview | null;
  pending: ApprovalView[];
  error: string | null;
  refresh: () => Promise<void>;
  setMode: (mode: AutonomyMode | null) => Promise<void>;
  setDefault: (mode: AutonomyMode) => Promise<void>;
  saveRules: (rules: PolicyRule[]) => Promise<void>;
  resetRules: () => Promise<void>;
  answer: (id: string, answer: ApprovalAnswer, note?: string | null) => Promise<void>;
  revoke: (grantId: string) => Promise<void>;
  pauseAll: () => Promise<void>;
  resumeAll: () => Promise<void>;
}

/** Events that change what the autonomy views show. */
const RELEVANT = new Set([
  "PROJECT_OPENED",
  "AUTONOMY_CHANGED",
  "APPROVAL_REQUESTED",
  "APPROVAL_DECIDED",
  "EXECUTION_PAUSED",
  "EXECUTION_RESUMED",
  "AGENT_FINISHED",
]);

export function useAutonomy(enabled: boolean, projectId: string | null): Autonomy {
  const [overview, setOverview] = useState<AutonomyOverview | null>(null);
  const [pending, setPending] = useState<ApprovalView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [next, waiting] = await Promise.all([autonomyApi.get(projectId), autonomyApi.pending()]);
      setOverview(next);
      setPending(waiting);
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, [projectId]);

  // Every change goes through the engine and comes back as the new state.
  const act = useCallback(
    async (work: () => Promise<unknown>) => {
      try {
        const result = await work();
        if (result && typeof result === "object" && "rules" in result) {
          setOverview(result as AutonomyOverview);
        }
        setError(null);
      } catch (err) {
        setError(errorMessage(err));
        throw err;
      } finally {
        void refresh();
      }
    },
    [refresh],
  );

  useEffect(() => {
    if (!enabled) return;
    void refresh();
    const unsubscribe = auditEvents.subscribe((event) => {
      if (!RELEVANT.has(event.kind)) return;
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => void refresh(), 80);
    });
    return () => {
      unsubscribe();
      if (timer.current) clearTimeout(timer.current);
    };
  }, [enabled, refresh]);

  return {
    overview,
    pending,
    error,
    refresh,
    setMode: (mode) => act(() => autonomyApi.setMode(projectId, mode)),
    setDefault: (mode) => act(() => autonomyApi.setDefault(projectId, mode)),
    saveRules: (rules) => act(() => autonomyApi.saveRules(projectId, rules)),
    resetRules: () => act(() => autonomyApi.resetRules(projectId)),
    answer: (id, answer, note) => act(() => autonomyApi.answer(id, answer, note)),
    revoke: (grantId) => act(() => autonomyApi.revoke(grantId)),
    pauseAll: () => act(() => autonomyApi.pauseAll()),
    resumeAll: () => act(() => autonomyApi.resumeAll()),
  };
}
