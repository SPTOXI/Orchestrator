// Agents of the open project and the files they hold (ADR-0015), refreshed
// when the history says an agent or a task moved. An agent works on its
// own, so the panel cannot wait for the user to ask.

import { useCallback, useEffect, useRef, useState } from "react";
import { auditEvents } from "./events";
import { agentApi, errorMessage } from "./runtime";
import type { AgentSettings, AgentView, BudgetView, FileLock } from "./types";

export interface Agents {
  list: AgentView[];
  locks: FileLock[];
  settings: AgentSettings | null;
  /** The project's spending today against the daily budget (ADR-0018). */
  budget: BudgetView | null;
  error: string | null;
  refresh: () => Promise<void>;
  saveSettings: (settings: AgentSettings) => Promise<void>;
}

/** Events that change what the AGENTS views show. */
const RELEVANT = new Set([
  "PROJECT_OPENED",
  "AGENT_STARTED",
  "AGENT_FINISHED",
  "TASK_CREATED",
  "TASK_STARTED",
  "TASK_COMPLETED",
  "TASK_UPDATED",
  // A lock is taken in the middle of a turn: a tool call is the only sign.
  "TOOL_CALLED",
  // Waiting for the user, or paused (ADR-0016).
  "APPROVAL_REQUESTED",
  "APPROVAL_DECIDED",
  "EXECUTION_PAUSED",
  "EXECUTION_RESUMED",
  "AUTONOMY_CHANGED",
  // What an agent spent, and the project's budget (ADR-0018).
  "TURN_COMPLETED",
]);

export function useAgents(enabled: boolean, projectId: string | null): Agents {
  const [list, setList] = useState<AgentView[]>([]);
  const [locks, setLocks] = useState<FileLock[]>([]);
  const [settings, setSettings] = useState<AgentSettings | null>(null);
  const [budget, setBudget] = useState<BudgetView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refresh = useCallback(async () => {
    if (!projectId) {
      setList([]);
      setLocks([]);
      setBudget(null);
      return;
    }
    try {
      const [agents, held, spent] = await Promise.all([
        agentApi.list(projectId),
        agentApi.locks(projectId),
        agentApi.budget(projectId),
      ]);
      setList(agents);
      setLocks(held);
      setBudget(spent);
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, [projectId]);

  const saveSettings = useCallback(
    async (next: AgentSettings) => {
      setSettings(await agentApi.saveSettings(next));
      // A new budget changes what is exhausted.
      void refresh();
    },
    [refresh],
  );

  useEffect(() => {
    if (!enabled) return;
    void refresh();
    agentApi
      .settings()
      .then(setSettings)
      .catch((err) => setError(errorMessage(err)));
    const unsubscribe = auditEvents.subscribe((event) => {
      if (!RELEVANT.has(event.kind)) return;
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => void refresh(), 300);
    });
    return () => {
      unsubscribe();
      if (timer.current) clearTimeout(timer.current);
    };
  }, [enabled, refresh]);

  return { list, locks, settings, budget, error, refresh, saveSettings };
}
