// Several projects open side by side (ADR-0023): which tabs belong to which
// project, what each project has running, and which projects can be linked.
// Pure helpers (unit tested).

import type { Project, ProjectLink, SessionInfo } from "./types";

/** The same folder, whatever the trailing separator. */
export function samePath(a: string | null | undefined, b: string | null | undefined): boolean {
  if (!a || !b) return false;
  const trim = (path: string) => path.replace(/[\\/]+$/, "");
  return trim(a) === trim(b);
}

/** Tabs that belong to no project: they stay when the project changes. */
export const GLOBAL_TAB_KINDS: ReadonlySet<string> = new Set(["connection", "council", "settings", "discovery", "cost"]);

/** What a tab needs to be placed: its kind and the project it belongs to
 * (`null`: every project). */
export interface Scoped {
  id: string;
  kind: string;
  scope: string | null;
}

/** The tabs shown while `project` is open. */
export function visibleTabs<T extends Scoped>(tabs: readonly T[], project: string | null): T[] {
  return tabs.filter((tab) => tab.scope === null || samePath(tab.scope, project));
}

/** The tab to show after switching to `project`: the one it showed last,
 * else its first tab, else a tab of every project. */
export function tabAfterSwitch<T extends Scoped>(
  tabs: readonly T[],
  project: string | null,
  remembered: string | null | undefined,
): string | null {
  const visible = visibleTabs(tabs, project);
  if (remembered && visible.some((tab) => tab.id === remembered)) return remembered;
  return (visible.find((tab) => tab.scope !== null) ?? visible[0])?.id ?? null;
}

/** The project a tab of `kind` opened now belongs to. */
export function scopeOf(kind: string, project: string | null, sessionProject?: string | null): string | null {
  if (GLOBAL_TAB_KINDS.has(kind)) return null;
  if (kind === "session" && sessionProject) return sessionProject;
  return project;
}

/** Sessions with a turn running, per project folder. */
export function runningByProject(sessions: readonly SessionInfo[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const session of sessions) {
    if (session.status !== "running") continue;
    const key = session.projectPath.replace(/[\\/]+$/, "");
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return counts;
}

/** How many sessions are running in `path`. */
export function runningIn(counts: ReadonlyMap<string, number>, path: string): number {
  return counts.get(path.replace(/[\\/]+$/, "")) ?? 0;
}

/** Projects the open one can be linked to: the open ones and the recent
 * ones, once each, without itself and the ones already linked. */
export function linkCandidates(
  current: Project | null,
  open: readonly Project[],
  recent: readonly Project[],
  links: readonly ProjectLink[],
): Project[] {
  if (!current) return [];
  const seen = new Set<string>([current.id, ...links.map((link) => link.project.id)]);
  const out: Project[] = [];
  for (const project of [...open, ...recent]) {
    if (seen.has(project.id)) continue;
    seen.add(project.id);
    out.push(project);
  }
  return out;
}

/** A folder as a map key: without the trailing separator. */
export function pathKey(path: string): string {
  return path.replace(/[\\/]+$/, "");
}

/** Start of the message the Orchestrator sends to a related project's AI
 * (engine `projects::framed_question`). */
export const QUESTION_PREFIX = "[Pergunta da IA do projeto ";

/** The project whose AI asked, when `text` is a question from a related
 * project. */
export function askingProject(text: string): string | null {
  if (!text.startsWith(QUESTION_PREFIX)) return null;
  const rest = text.slice(QUESTION_PREFIX.length);
  const end = rest.indexOf(", em ");
  return end > 0 ? rest.slice(0, end) : null;
}
