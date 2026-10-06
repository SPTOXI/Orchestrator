// Tasks of the open project (ADR-0014), refreshed when the history says a
// task changed — including when a session or an agent moves one.

import { useCallback, useEffect, useRef, useState } from "react";
import { auditEvents } from "./events";
import { errorMessage, taskApi } from "./runtime";
import type { TaskView } from "./types";

export interface Tasks {
  list: TaskView[];
  error: string | null;
  refresh: () => Promise<void>;
}

/** Events that change what the TASKS views show. */
const RELEVANT = new Set([
  "PROJECT_OPENED",
  "TASK_CREATED",
  "TASK_STARTED",
  "TASK_COMPLETED",
  "TASK_UPDATED",
]);

export function useTasks(enabled: boolean, projectId: string | null): Tasks {
  const [list, setList] = useState<TaskView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refresh = useCallback(async () => {
    if (!projectId) {
      setList([]);
      return;
    }
    try {
      setList(await taskApi.list(projectId));
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
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => void refresh(), 300);
    });
    return () => {
      unsubscribe();
      if (timer.current) clearTimeout(timer.current);
    };
  }, [enabled, refresh]);

  return { list, error, refresh };
}
