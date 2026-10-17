// Pure helpers of the "Tokens e custo" tab (ADR-0018).

import type { PromptBreakdown, ToolGroup } from "./types";

export const SPEND_PERIODS: Array<{ days: number; label: string }> = [
  { days: 1, label: "Hoje" },
  { days: 7, label: "7 dias" },
  { days: 30, label: "30 dias" },
];

export function spendLabel(days: number): string {
  if (days <= 1) return "Hoje (desde a meia-noite)";
  return `Últimos ${days} dias`;
}

/** Share of the input read from the cache, 0–100 (null without input). */
export function cacheRatio(cached: number, input: number): number | null {
  if (!input) return null;
  return Math.round((Math.min(cached, input) / input) * 100);
}

export interface PromptPart {
  key: keyof PromptBreakdown;
  label: string;
  tokens: number;
  /** Share of the estimated prompt, 0–100. */
  percent: number;
}

const PROMPT_PARTS: Array<{ key: keyof PromptBreakdown; label: string }> = [
  { key: "system", label: "Instruções e contexto" },
  { key: "tools", label: "Definições de ferramentas" },
  { key: "history", label: "Conversa" },
  { key: "toolResults", label: "Resultados de ferramentas" },
];

/** The parts of the prompts, largest first; empty without data. */
export function promptParts(prompt: PromptBreakdown | null | undefined): PromptPart[] {
  if (!prompt) return [];
  const total = PROMPT_PARTS.reduce((sum, part) => sum + (prompt[part.key] ?? 0), 0);
  if (!total) return [];
  return PROMPT_PARTS.map((part) => ({
    ...part,
    tokens: prompt[part.key] ?? 0,
    percent: Math.round(((prompt[part.key] ?? 0) / total) * 100),
  })).sort((a, b) => b.tokens - a.tokens);
}

/** "todas (~9.800 tokens por pedido)" or "3 de 12 grupos (~2.100 tokens por pedido)". */
export function toolGroupsLabel(groups: ToolGroup[], selected: string[] | null | undefined): string {
  const pick = selected ? groups.filter((g) => selected.includes(g.group)) : groups;
  const tokens = pick.reduce((sum, g) => sum + g.tokens, 0).toLocaleString("pt-BR");
  if (!selected) return `todas (~${tokens} tokens por pedido)`;
  if (pick.length === 0) return "nenhuma";
  return `${pick.length} de ${groups.length} grupos (~${tokens} tokens por pedido)`;
}
