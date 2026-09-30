// Pure helpers for the router and Council views (unit tested; ADR-0011).

import type {
  CouncilMember,
  CouncilMode,
  Deliberation,
  ModelRef,
  Preference,
  ProviderInfo,
} from "./types";

export const MODE_LABELS: Record<CouncilMode, string> = {
  off: "Desligado",
  suggest: "Sugerir",
  full: "Full",
};

export const MODE_HINTS: Record<CouncilMode, string> = {
  off: "Só o roteador, sem gastar tokens: ele ordena os modelos e você escolhe.",
  suggest: "O Conselho delibera e recomenda; você aprova ou escolhe outro modelo.",
  full: "O Conselho decide e abre a sessão sozinho, enviando a tarefa. As ferramentas que a sessão pedir passam pelo modo de autonomia do projeto.",
};

export const PREFERENCE_LABELS: Record<Preference, string> = {
  quality: "Qualidade",
  balanced: "Equilíbrio",
  cost: "Custo",
  speed: "Velocidade",
};

/** Label of the main button of the route view. */
export function primaryAction(mode: CouncilMode): string {
  switch (mode) {
    case "off":
      return "Recomendar";
    case "suggest":
      return "Consultar o Conselho";
    case "full":
      return "Decidir e iniciar";
  }
}

export function refKey(ref: ModelRef): string {
  return `${ref.provider}/${ref.model}`;
}

export function sameRef(a: ModelRef | null | undefined, b: ModelRef | null | undefined): boolean {
  return !!a && !!b && a.provider === b.provider && a.model === b.model;
}

/** Providers that can sit on the Council. */
export function councilProviders(providers: ProviderInfo[]): ProviderInfo[] {
  return providers.filter((p) => p.capabilities.completion);
}

/** "Claude / claude-opus" (or the raw ids when the provider is gone). */
export function memberLabel(member: CouncilMember, providers: ProviderInfo[]): string {
  const provider = providers.find((p) => p.id === member.provider);
  const model = member.model ?? provider?.capabilities.defaultModel ?? "modelo padrão";
  return `${provider?.name ?? member.provider} / ${model}`;
}

/** Indexes of members that repeat an earlier one. */
export function duplicateMembers(members: CouncilMember[]): number[] {
  const seen = new Set<string>();
  const out: number[] = [];
  members.forEach((m, i) => {
    const key = `${m.provider}\u0000${m.model ?? ""}`;
    if (seen.has(key)) out.push(i);
    seen.add(key);
  });
  return out;
}

export function formatPercent(value: number): string {
  return `${Math.round(value * 100)}%`;
}

/** "Conselho", "Conselho (cache)" or "Roteador". */
export function decisionSource(deliberation: Deliberation): string {
  if (!deliberation.decision) return "sem decisão";
  if (deliberation.decision.source === "router") return "Roteador";
  return deliberation.cached ? "Conselho (cache)" : "Conselho";
}

/** "2 de 3 membros válidos" */
export function votesSummary(deliberation: Deliberation): string {
  const valid = deliberation.votes.filter((v) => !v.error).length;
  return `${valid} de ${deliberation.votes.length} ${deliberation.votes.length === 1 ? "membro válido" : "membros válidos"}`;
}

/** "128k" → 128000, "1M" → 1000000, "32000" → 32000; empty → null. */
export function parseContext(text: string): number | null | undefined {
  const t = text.trim().toLowerCase().replace(",", ".");
  if (!t) return null;
  const match = /^(\d+(?:\.\d+)?)\s*([km]?)$/.exec(t);
  if (!match) return undefined;
  const factor = match[2] === "m" ? 1_000_000 : match[2] === "k" ? 1_000 : 1;
  return Math.round(Number(match[1]) * factor);
}

/** 200000 → "200k", 1000000 → "1M". */
export function formatContext(tokens: number | null): string {
  if (tokens === null) return "?";
  if (tokens >= 1_000_000 && tokens % 1_000_000 === 0) return `${tokens / 1_000_000}M`;
  if (tokens >= 1_000) return `${Math.floor(tokens / 1_000)}k`;
  return String(tokens);
}

/** "US$ 3 / 15" per million tokens, or "preço ?" */
export function formatPricePair(input: number | null, output: number | null): string {
  if (input === null && output === null) return "preço ?";
  const f = (v: number | null) => (v === null ? "?" : String(v));
  return `US$ ${f(input)} / ${f(output)}`;
}
