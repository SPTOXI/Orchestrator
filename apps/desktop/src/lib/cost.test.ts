import { describe, expect, it } from "vitest";
import { cacheRatio, spendLabel } from "./cost";

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
});
