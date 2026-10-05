import { describe, expect, it } from "vitest";
import {
  councilProviders,
  councilSummary,
  decisionSource,
  duplicateMembers,
  formatContext,
  formatPricePair,
  memberLabel,
  moveMember,
  parseContext,
  primaryAction,
  sameRef,
  seatRole,
  votesSummary,
} from "./council";
import type { Analysis, Deliberation, ProviderInfo, TokenUsage, Vote } from "./types";

const usage: TokenUsage = {
  inputTokens: 0,
  outputTokens: 0,
  cachedInputTokens: 0,
  reasoningTokens: 0,
  costUsd: null,
  estimated: false,
};

function provider(id: string, completion: boolean): ProviderInfo {
  return {
    id,
    name: id.toUpperCase(),
    vendor: "teste",
    description: "",
    active: false,
    capabilities: {
      streaming: true,
      toolCalls: true,
      resume: true,
      cancel: true,
      nativeSubagents: false,
      reasoning: false,
      tokenUsage: true,
      cost: false,
      completion,
      models: [],
      defaultModel: `${id}-default`,
    },
  };
}

function vote(error: string | null): Vote {
  return {
    member: { provider: "a", model: null },
    providerName: "A",
    model: null,
    choice: null,
    ranking: [],
    confidence: null,
    reason: null,
    error,
    usage,
    durationMs: 0,
  };
}

function deliberation(patch: Partial<Deliberation>): Deliberation {
  return {
    id: "d1",
    createdAt: "2026-09-29T10:00:00Z",
    task: "",
    mode: "suggest",
    recommendation: {
      activity: "code",
      detected: true,
      preference: "balanced",
      needsTools: true,
      minContext: null,
      candidates: [],
      excluded: [],
    },
    shortlist: [],
    votes: [],
    analyses: [],
    plan: null,
    seats: [],
    projectPath: null,
    decision: null,
    usage,
    cached: false,
    cachedFrom: null,
    savedUsage: null,
    notices: [],
    autoApply: false,
    durationMs: 0,
    ...patch,
  };
}

describe("council helpers", () => {
  it("names the action of each mode", () => {
    expect(primaryAction("off")).toBe("Recomendar");
    expect(primaryAction("suggest")).toBe("Analisar com o Conselho");
    expect(primaryAction("full")).toBe("Analisar e executar");
  });

  it("lists only providers that answer one-off requests", () => {
    const list = [provider("a", true), provider("b", false)];
    expect(councilProviders(list).map((p) => p.id)).toEqual(["a"]);
    expect(memberLabel({ provider: "a", model: null }, list)).toBe("A / a-default");
    expect(memberLabel({ provider: "gone", model: "m" }, list)).toBe("gone / m");
  });

  it("finds repeated members", () => {
    expect(
      duplicateMembers([
        { provider: "a", model: null },
        { provider: "a", model: "x" },
        { provider: "a", model: null },
      ]),
    ).toEqual([2]);
  });

  it("parses and formats context sizes", () => {
    expect(parseContext("128k")).toBe(128_000);
    expect(parseContext(" 1M ")).toBe(1_000_000);
    expect(parseContext("1,5m")).toBe(1_500_000);
    expect(parseContext("32000")).toBe(32_000);
    expect(parseContext("")).toBeNull();
    expect(parseContext("muito")).toBeUndefined();
    expect(formatContext(200_000)).toBe("200k");
    expect(formatContext(2_000_000)).toBe("2M");
    expect(formatContext(null)).toBe("?");
    expect(formatPricePair(3, 15)).toBe("US$ 3 / 15");
    expect(formatPricePair(null, null)).toBe("preço ?");
  });

  it("describes who decided and how many votes counted", () => {
    const decision = {
      provider: "a",
      model: "m",
      providerName: "A",
      modelName: "m",
      reason: "",
      agreement: 1,
    };
    expect(decisionSource(deliberation({}))).toBe("sem decisão");
    expect(decisionSource(deliberation({ decision: { ...decision, source: "router" } }))).toBe("Roteador");
    expect(decisionSource(deliberation({ decision: { ...decision, source: "council" }, cached: true }))).toBe(
      "Conselho (cache)",
    );
    expect(votesSummary(deliberation({ votes: [vote(null), vote("sem resposta")] }))).toBe(
      "1 de 2 membros válidos",
    );
    expect(votesSummary(deliberation({ votes: [vote(null)] }))).toBe("1 de 1 membro válido");
    expect(sameRef({ provider: "a", model: "m" }, { provider: "a", model: "m" })).toBe(true);
    expect(sameRef({ provider: "a", model: "m" }, null)).toBe(false);
  });

  it("orders the members: the first executes, the others back it up", () => {
    const members = [
      { provider: "claude", model: null },
      { provider: "gemini", model: null },
      { provider: "codex", model: null },
    ];
    expect(moveMember(members, 1, -1).map((m) => m.provider)).toEqual(["gemini", "claude", "codex"]);
    expect(moveMember(members, 1, 1).map((m) => m.provider)).toEqual(["claude", "codex", "gemini"]);
    expect(moveMember(members, 0, -1)).toBe(members);
    expect(moveMember(members, 2, 1)).toBe(members);
    expect([0, 1, 2].map(seatRole)).toEqual(["executa", "1ª reserva", "2ª reserva"]);
  });

  it("sums up a joint analysis, or the votes of an older deliberation", () => {
    const analysis = (text: string | null): Analysis => ({
      member: { provider: "a", model: null },
      providerName: "A",
      model: null,
      text,
      error: text === null ? "sobrecarregado" : null,
      usage,
      durationMs: 0,
    });
    const plan = { text: "1. fila", source: "synthesis" as const, by: null, byName: "A", usage, failures: [] };
    expect(councilSummary(deliberation({ analyses: [analysis("ok"), analysis(null)], plan }))).toBe(
      "1 de 2 análises · plano conjunto",
    );
    expect(councilSummary(deliberation({ analyses: [analysis(null)] }))).toBe("0 de 1 análise · sem plano");
    expect(councilSummary(deliberation({ votes: [vote(null)] }))).toBe("1 de 1 membro válido");
  });
});
