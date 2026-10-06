import { describe, expect, it } from "vitest";
import { OutputSync, utf8Length } from "./outputSync";

describe("OutputSync", () => {
  it("queues live chunks until the snapshot and drops duplicates", () => {
    const out: string[] = [];
    const sync = new OutputSync((d) => out.push(d));

    sync.push(0, "hello "); // already contained in the snapshot
    sync.push(6, "world"); // arrives after the snapshot was taken
    expect(out).toEqual([]);

    sync.snapshot("hello ", 6);
    expect(out).toEqual(["hello ", "world"]);
    expect(sync.position).toBe(11);

    sync.push(6, "world"); // duplicate
    sync.push(11, "!");
    expect(out.join("")).toBe("hello world!");
  });

  it("uses UTF-8 byte offsets", () => {
    const out: string[] = [];
    const sync = new OutputSync((d) => out.push(d));
    sync.snapshot("", 0);
    sync.push(0, "ação");
    expect(sync.position).toBe(utf8Length("ação"));
    expect(utf8Length("ação")).toBe(6);
    sync.push(6, "✓");
    expect(out.join("")).toBe("ação✓");
  });

  it("writes the rendered form but advances by the raw data", () => {
    const out: string[] = [];
    const sync = new OutputSync((d) => out.push(d));
    sync.push(0, "err", "<red>err</red>");
    sync.snapshot("", 0);
    expect(out).toEqual(["<red>err</red>"]);
    expect(sync.position).toBe(3);
  });

  it("handles an empty snapshot", () => {
    const out: string[] = [];
    const sync = new OutputSync((d) => out.push(d));
    sync.snapshot("", 42);
    sync.push(40, "old");
    sync.push(42, "new");
    expect(out).toEqual(["new"]);
  });
});
