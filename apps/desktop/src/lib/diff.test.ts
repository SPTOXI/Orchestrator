import { describe, expect, it } from "vitest";
import { parseUnifiedDiff } from "./diff";

const PATCH = [
  "diff --git a/src/a.ts b/src/a.ts",
  "index 111..222 100644",
  "--- a/src/a.ts",
  "+++ b/src/a.ts",
  "@@ -3,3 +3,4 @@ export function a() {",
  " const x = 1;",
  "-const y = 2;",
  "+const y = 3;",
  "+const z = 4;",
  " return x;",
  "\\ No newline at end of file",
  "",
].join("\n");

describe("parseUnifiedDiff", () => {
  it("classifies lines and numbers them", () => {
    const lines = parseUnifiedDiff(PATCH);
    expect(lines.map((l) => l.kind)).toEqual([
      "file",
      "meta",
      "meta",
      "meta",
      "hunk",
      "context",
      "del",
      "add",
      "add",
      "context",
      "meta",
    ]);
    expect(lines[5]).toMatchObject({ oldLine: 3, newLine: 3 });
    expect(lines[6]).toMatchObject({ kind: "del", oldLine: 4 });
    expect(lines[7]).toMatchObject({ kind: "add", newLine: 4 });
    expect(lines[8]).toMatchObject({ kind: "add", newLine: 5 });
    expect(lines[9]).toMatchObject({ oldLine: 5, newLine: 6 });
  });

  it("handles an empty patch", () => {
    expect(parseUnifiedDiff("")).toEqual([]);
  });
});
