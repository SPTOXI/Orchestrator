// Recently opened projects as the UI shows them. Before Phase 6 the list
// lived in localStorage; it is now imported into the database (ADR-0012).

export interface RecentProject {
  path: string;
  name: string;
  openedAt: string;
}

const KEY = "orchestrator.recentProjects";
export const MAX_RECENT = 8;

/** Puts `entry` first, removing an older entry for the same path. */
export function addRecent(list: RecentProject[], entry: RecentProject, max = MAX_RECENT): RecentProject[] {
  return [entry, ...list.filter((p) => p.path !== entry.path)].slice(0, max);
}

export function removeRecent(list: RecentProject[], path: string): RecentProject[] {
  return list.filter((p) => p.path !== path);
}

export function loadRecent(): RecentProject[] {
  try {
    const raw = localStorage.getItem(KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed)
      ? parsed.filter((p): p is RecentProject => typeof p?.path === "string" && typeof p?.name === "string")
      : [];
  } catch {
    return [];
  }
}

export function saveRecent(list: RecentProject[]): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(list));
  } catch {
    // Storage unavailable: recents are just not remembered.
  }
}

/** Forgets the old list once it moved to the database (Phase 6). */
export function clearRecent(): void {
  try {
    localStorage.removeItem(KEY);
  } catch {
    // Storage unavailable: nothing to clear.
  }
}
