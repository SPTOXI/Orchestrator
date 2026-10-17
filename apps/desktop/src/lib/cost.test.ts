import { describe, expect, it } from "vitest";
import { cacheRatio, promptParts, spendLabel, toolGroupsLabel } from "./cost";

describe("cost", () => {
  it("names the period", () => {
    expect(spendLabel(1)).toBe("Hoje (desde a meia-noite)");
    expect(spendLabel(7)).toBe("Últimos 7 dias");
  });

  it("measures the cache share", () => {
    expect(cacheRatio(800, 1000)).toBe(80);
    expect(cacheRatio(0, 0)).toBeNull();
    expect(cacheRatio(5, 3)).toBe(100);
  });

  it("splits the prompt, largest part first", () => {
    const parts = promptParts({ system: 10, tools: 60, history: 20, toolResults: 10 });
    expect(parts.map((p) => p.key)).toEqual(["tools", "history", "system", "toolResults"]);
    expect(parts[0]?.percent).toBe(60);
    expect(promptParts({ system: 0, tools: 0, history: 0, toolResults: 0 })).toEqual([]);
    expect(promptParts(undefined)).toEqual([]);
  });

  it("describes the tools a session offers", () => {
    const groups = [
      { group: "filesystem", tools: 5, tokens: 1000 },
      { group: "github", tools: 11, tokens: 3000 },
    ];
    expect(toolGroupsLabel(groups, null)).toBe("todas (~4.000 tokens por pedido)");
    expect(toolGroupsLabel(groups, ["filesystem"])).toBe("1 de 2 grupos (~1.000 tokens por pedido)");
    expect(toolGroupsLabel(groups, [])).toBe("nenhuma");
  });
});
