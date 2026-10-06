import { describe, expect, it } from "vitest";
import { addRecent, removeRecent, type RecentProject } from "./recent";

const p = (path: string): RecentProject => ({ path, name: path, openedAt: "2026-01-01T00:00:00Z" });

describe("recent projects", () => {
  it("moves a reopened project to the front without duplicates", () => {
    const list = [p("/a"), p("/b"), p("/c")];
    expect(addRecent(list, p("/b")).map((x) => x.path)).toEqual(["/b", "/a", "/c"]);
  });

  it("keeps at most max entries", () => {
    const list = [p("/a"), p("/b")];
    expect(addRecent(list, p("/c"), 2).map((x) => x.path)).toEqual(["/c", "/a"]);
  });

  it("removes by path", () => {
    expect(removeRecent([p("/a"), p("/b")], "/a").map((x) => x.path)).toEqual(["/b"]);
  });
});
