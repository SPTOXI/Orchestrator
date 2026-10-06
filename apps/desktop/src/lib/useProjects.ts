// Recent projects, the projects open side by side and the open one, from
// the database (ADR-0012, ADR-0023). The list the UI kept in localStorage
// before Phase 6 is imported once.

import { useCallback, useEffect, useState } from "react";
import { auditEvents } from "./events";
import { clearRecent, loadRecent, type RecentProject } from "./recent";
import { errorMessage, memoryApi } from "./runtime";
import type { Project, ProjectLink } from "./types";

export interface Projects {
  recent: RecentProject[];
  /** Recent projects as registered (to link them). */
  recentProjects: Project[];
  /** Projects open side by side, in sidebar order (ADR-0023). */
  open: Project[];
  /** The open project as registered in the database. */
  current: Project | null;
  /** Projects that work with the current one (ADR-0023). */
  links: ProjectLink[];
  error: string | null;
  forget: (path: string) => Promise<void>;
  /** Closes a project in the app; returns the one to show next. */
  close: (id: string) => Promise<Project | null>;
  reorder: (ids: string[]) => Promise<void>;
  link: (other: string, note: string) => Promise<boolean>;
  unlink: (other: string) => Promise<void>;
  refresh: () => Promise<void>;
}

export function useProjects(enabled: boolean): Projects {
  const [list, setList] = useState<Project[]>([]);
  const [open, setOpen] = useState<Project[]>([]);
  const [current, setCurrent] = useState<Project | null>(null);
  const [links, setLinks] = useState<ProjectLink[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [recent, opened, now] = await Promise.all([
        memoryApi.projectsRecent(12),
        memoryApi.projectsOpen(),
        memoryApi.projectCurrent(),
      ]);
      setList(recent);
      setOpen(opened);
      setCurrent(now);
      setLinks(now ? await memoryApi.links(now.id) : []);
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
      if (event.kind === "PROJECT_OPENED" || event.kind === "PROJECT_CREATED" || event.kind === "PROJECT_LINKED")
        void refresh();
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

  const close = useCallback(
    async (id: string) => {
      try {
        const next = await memoryApi.projectClose(id);
        await refresh();
        return next;
      } catch (e) {
        setError(errorMessage(e));
        return null;
      }
    },
    [refresh],
  );

  const reorder = useCallback(
    async (ids: string[]) => {
      try {
        await memoryApi.projectsReorder(ids);
        await refresh();
      } catch (e) {
        setError(errorMessage(e));
      }
    },
    [refresh],
  );

  const link = useCallback(
    async (other: string, note: string) => {
      if (!current) return false;
      try {
        setLinks(await memoryApi.link(current.id, other, note));
        setError(null);
        return true;
      } catch (e) {
        setError(errorMessage(e));
        return false;
      }
    },
    [current],
  );

  const unlink = useCallback(
    async (other: string) => {
      if (!current) return;
      try {
        setLinks(await memoryApi.unlink(current.id, other));
        setError(null);
      } catch (e) {
        setError(errorMessage(e));
      }
    },
    [current],
  );

  return {
    recent: list.slice(0, 8).map((p) => ({ path: p.path, name: p.name, openedAt: p.lastOpenedAt })),
    recentProjects: list,
    open,
    current,
    links,
    error,
    forget,
    close,
    reorder,
    link,
    unlink,
    refresh,
  };
}
