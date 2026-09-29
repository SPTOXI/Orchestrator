// Memory of the open project (ADR-0012): L1 overview, L2 entries and
// decisions, refreshed when the history reports changes.

import { useCallback, useEffect, useRef, useState } from "react";
import { auditEvents } from "./events";
import { errorMessage, memoryApi } from "./runtime";
import type { MemoryEntry, MemoryOverview, ProjectDecision } from "./types";

export interface Memory {
  overview: MemoryOverview | null;
  entries: MemoryEntry[];
  decisions: ProjectDecision[];
  error: string | null;
  refresh: () => Promise<void>;
}

/** Events that change what the MEMORY views show. */
const RELEVANT = new Set([
  "PROJECT_OPENED",
  "MEMORY_SAVED",
  "MEMORY_REMOVED",
  "DECISION_SAVED",
  "FILE_CHANGED",
  "COMMAND_EXECUTED",
  "TOOL_CALLED",
  "TURN_COMPLETED",
  "SESSION_STARTED",
  "SESSION_CLOSED",
  "SESSION_RESUMED",
  "PROCESS_EXITED",
]);

export function useMemory(enabled: boolean, projectId: string | null): Memory {
  const [overview, setOverview] = useState<MemoryOverview | null>(null);
  const [entries, setEntries] = useState<MemoryEntry[]>([]);
  const [decisions, setDecisions] = useState<ProjectDecision[]>([]);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refresh = useCallback(async () => {
    if (!projectId) {
      setOverview(null);
      setEntries([]);
      setDecisions([]);
      return;
    }
    try {
      const [o, e, d] = await Promise.all([
        memoryApi.overview(projectId),
        memoryApi.list(projectId),
        memoryApi.decisions(projectId),
      ]);
      setOverview(o);
      setEntries(e);
      setDecisions(d);
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, [projectId]);

  useEffect(() => {
    if (!enabled) return;
    void refresh();
    const unsubscribe = auditEvents.subscribe((event) => {
      if (!RELEVANT.has(event.kind)) return;
      // Bursts (a turn with many tool calls) refresh once.
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => void refresh(), 400);
    });
    return () => {
      unsubscribe();
      if (timer.current) clearTimeout(timer.current);
    };
  }, [enabled, refresh]);

  return { overview, entries, decisions, error, refresh };
}
