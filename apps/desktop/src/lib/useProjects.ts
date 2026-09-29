// Recent projects and the open project, from the database (ADR-0012). The
// list the UI kept in localStorage before Phase 6 is imported once.

import { useCallback, useEffect, useState } from "react";
import { auditEvents } from "./events";
import { clearRecent, loadRecent, type RecentProject } from "./recent";
import { errorMessage, memoryApi } from "./runtime";
import type { Project } from "./types";

export interface Projects {
  recent: RecentProject[];
  /** The open project as registered in the database. */
  current: Project | null;
  error: string | null;
  forget: (path: string) => Promise<void>;
  refresh: () => Promise<void>;
}

export function useProjects(enabled: boolean): Projects {
  const [list, setList] = useState<Project[]>([]);
  const [current, setCurrent] = useState<Project | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [recent, open] = await Promise.all([memoryApi.projectsRecent(8), memoryApi.projectCurrent()]);
      setList(recent);
      setCurrent(open);
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (!enabled) return;
    const legacy = loadRecent();
    const imported = legacy.length
      ? memoryApi
          .importRecent(legacy)
          .then(() => clearRecent())
          .catch(() => undefined)
      : Promise.resolve();
    void imported.then(refresh);
    return auditEvents.subscribe((event) => {
      if (event.kind === "PROJECT_OPENED" || event.kind === "PROJECT_CREATED") void refresh();
    });
  }, [enabled, refresh]);

  const forget = useCallback(
    async (path: string) => {
      const project = list.find((p) => p.path === path);
      if (!project) return;
      try {
        await memoryApi.projectForget(project.id);
        await refresh();
      } catch (e) {
        setError(errorMessage(e));
      }
    },
    [list, refresh],
  );

  return {
    recent: list.map((p) => ({ path: p.path, name: p.name, openedAt: p.lastOpenedAt })),
    current,
    error,
    forget,
    refresh,
  };
}
