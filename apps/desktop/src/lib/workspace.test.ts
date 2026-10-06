import { describe, expect, it } from "vitest";
import type { Project, ProjectLink, SessionInfo } from "./types";
import {
  askingProject,
  linkCandidates,
  runningByProject,
  runningIn,
  samePath,
  scopeOf,
  tabAfterSwitch,
  visibleTabs,
} from "./workspace";

const project = (id: string, path = `/p/${id}`): Project => ({
  id,
  path,
  name: id,
  createdAt: "2026-10-01T00:00:00Z",
  lastOpenedAt: "2026-10-01T00:00:00Z",
  stack: null,
  openRank: null,
});

const session = (projectPath: string, status: SessionInfo["status"]): SessionInfo =>
  ({ id: `${projectPath}:${status}:${Math.random()}`, projectPath, status }) as SessionInfo;

describe("workspace", () => {
  it("compares folders whatever the trailing separator", () => {
    expect(samePath("/p/api/", "/p/api")).toBe(true);
    expect(samePath("C:\\p\\api\\", "C:\\p\\api")).toBe(true);
    expect(samePath("/p/api", "/p/app")).toBe(false);
    expect(samePath(null, "/p/api")).toBe(false);
  });

  it("keeps each project's tabs apart and the global ones everywhere", () => {
    const tabs = [
      { id: "file:/p/api/a.rs", kind: "file", scope: "/p/api" },
      { id: "settings", kind: "settings", scope: null },
      { id: "session:2", kind: "session", scope: "/p/app/" },
    ];
    expect(visibleTabs(tabs, "/p/api").map((t) => t.id)).toEqual(["file:/p/api/a.rs", "settings"]);
    expect(visibleTabs(tabs, "/p/app").map((t) => t.id)).toEqual(["settings", "session:2"]);
    expect(visibleTabs(tabs, null).map((t) => t.id)).toEqual(["settings"]);

    // Back to a project: the tab it showed, else its first, else a global.
    expect(tabAfterSwitch(tabs, "/p/app", "session:2")).toBe("session:2");
    expect(tabAfterSwitch(tabs, "/p/api", "session:2")).toBe("file:/p/api/a.rs");
    expect(tabAfterSwitch(tabs, "/p/site", null)).toBe("settings");
    expect(tabAfterSwitch([], "/p/site", null)).toBeNull();
  });

  it("places a new tab in its project", () => {
    expect(scopeOf("settings", "/p/api")).toBeNull();
    expect(scopeOf("memory", "/p/api")).toBe("/p/api");
    // A session belongs to its own project, wherever it is opened from.
    expect(scopeOf("session", "/p/api", "/p/app")).toBe("/p/app");
    expect(scopeOf("session", "/p/api", null)).toBe("/p/api");
  });

  it("counts the AIs working in each project", () => {
    const counts = runningByProject([
      session("/p/api", "running"),
      session("/p/api/", "running"),
      session("/p/api", "idle"),
      session("/p/app", "closed"),
    ]);
    expect(runningIn(counts, "/p/api")).toBe(2);
    expect(runningIn(counts, "/p/app")).toBe(0);
  });

  it("offers the open and recent projects not linked yet", () => {
    const [api, app, site, lib] = [project("api"), project("app"), project("site"), project("lib")];
    const links: ProjectLink[] = [{ project: site, note: "", createdAt: "" }];
    expect(linkCandidates(api, [api, app, site], [lib, app, api], links).map((p) => p.id)).toEqual(["app", "lib"]);
    expect(linkCandidates(null, [api], [], [])).toEqual([]);
  });

  it("recognizes a question from a related project's AI", () => {
    expect(askingProject("[Pergunta da IA do projeto app, em /p/app, que trabalha junto…]\n\nQual rota?")).toBe("app");
    expect(askingProject("Qual rota?")).toBeNull();
  });
});
