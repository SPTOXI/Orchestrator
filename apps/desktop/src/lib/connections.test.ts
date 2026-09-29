import { describe, expect, it } from "vitest";
import { emptyModel, formatPrices, mergeModels, parseJson, slugify, toJsonText } from "./connections";

describe("connections", () => {
  it("slugifies names into provider ids", () => {
    expect(slugify("OpenAI Pessoal")).toBe("openai-pessoal");
    expect(slugify("  Cláudio & Gêmeos!! ")).toBe("claudio-gemeos");
    expect(slugify("***")).toBe("api");
  });

  it("merges discovered models without overriding user edits", () => {
    const mine = { ...emptyModel("a"), inputPrice: 9, tags: ["meu"] };
    const found = [
      { ...emptyModel("a"), inputPrice: 1, outputPrice: 2, contextWindow: 1000, tags: ["x"] },
      { ...emptyModel("b") },
    ];
    const merged = mergeModels([mine], found);
    expect(merged[0]).toMatchObject({ id: "a", inputPrice: 9, outputPrice: 2, contextWindow: 1000, tags: ["meu"] });
    expect(merged[1]).toMatchObject({ id: "b", enabled: false });
  });

  it("enables a small first discovery", () => {
    const merged = mergeModels([], [emptyModel("a"), emptyModel("b")]);
    expect(merged.every((m) => m.enabled)).toBe(true);
    const many = mergeModels(
      [],
      Array.from({ length: 20 }, (_, i) => emptyModel(`m${i}`)),
    );
    expect(many.some((m) => m.enabled)).toBe(false);
  });

  it("parses and prints JSON fields", () => {
    expect(parseJson("")).toEqual({ ok: true, value: null });
    expect(parseJson('{"a":1}')).toEqual({ ok: true, value: { a: 1 } });
    expect(parseJson("{").ok).toBe(false);
    expect(toJsonText(null)).toBe("");
    expect(toJsonText({})).toBe("");
    expect(toJsonText({ a: 1 })).toBe('{\n  "a": 1\n}');
  });

  it("formats prices", () => {
    expect(formatPrices({ inputPrice: null, outputPrice: null })).toBeNull();
    expect(formatPrices({ inputPrice: 2.5, outputPrice: 10 })).toBe("US$ 2,5 / 10 por 1M");
  });
});
