import { describe, expect, it } from "vitest";
import { changeLabel, exitLabel, parseTags, splitSnippet } from "./memory";

describe("memory helpers", () => {
  it("parses tags without blanks or repeats", () => {
    expect(parseTags("banco, SQL,  banco , ,sql")).toEqual(["banco", "SQL"]);
    expect(parseTags("")).toEqual([]);
  });

  it("splits search snippets into matches", () => {
    expect(splitSnippet("…a fila de [retentativas] do [worker]")).toEqual([
      { text: "…a fila de ", match: false },
      { text: "retentativas", match: true },
      { text: " do ", match: false },
      { text: "worker", match: true },
    ]);
    expect(splitSnippet("sem destaque")).toEqual([{ text: "sem destaque", match: false }]);
  });

  it("labels file changes and exit codes", () => {
    expect(changeLabel("created")).toBe("criado");
    expect(changeLabel("renamed")).toBe("renamed");
    expect(exitLabel(0, false)).toBe("ok");
    expect(exitLabel(101, false)).toBe("saída 101");
    expect(exitLabel(null, true)).toBe("em segundo plano");
  });
});
