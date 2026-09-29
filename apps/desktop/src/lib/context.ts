// Pure helpers for the context and handoff views (unit tested; ADR-0013).

import type { Handoff, HandoffPacket, SectionKind } from "./types";

/** Start of the request the Orchestrator sends to a session when preparing
 * a handoff (engine `packet::AGENT_PROMPT`). */
export const HANDOFF_PROMPT_PREFIX = "The user is handing this work over";

export const SECTION_LABELS: Record<SectionKind, string> = {
  task: "Tarefa",
  working: "Memória de trabalho (L1)",
  project: "Memória do projeto (L2) e decisões",
  files: "Arquivos relevantes",
  errors: "Erros recentes",
  history: "Histórico relevante (L3)",
  git: "Estado do Git",
  handoff: "Handoff",
};

/** List fields of a packet, in the master document's order, with labels. */
export const PACKET_LISTS: Array<{ key: keyof HandoffPacket & string; label: string; hint: string }> = [
  { key: "completed", label: "Concluído", hint: "o que já foi feito" },
  { key: "remaining", label: "Falta", hint: "o que ainda precisa ser feito" },
  { key: "files", label: "Arquivos", hint: "caminhos alterados ou importantes" },
  { key: "commands", label: "Comandos", hint: "comandos executados e resultado" },
  { key: "errors", label: "Erros", hint: "problemas em aberto" },
  { key: "decisions", label: "Decisões", hint: "decisões tomadas e por quê" },
  { key: "tests", label: "Testes", hint: "testes executados ou necessários" },
];

export function emptyPacket(): HandoffPacket {
  return {
    goal: "",
    status: "",
    completed: [],
    remaining: [],
    files: [],
    commands: [],
    errors: [],
    decisions: [],
    tests: [],
    nextAction: "",
  };
}

/** One item per non-blank line, trimmed, bullets removed. */
export function linesOf(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.replace(/^\s*[-*•]\s*/, "").trim())
    .filter((line) => line.length > 0);
}

/** "~1.234 tokens de 1.500 · 6 seções · 2 omissões" */
export function summaryLine(summary: {
  tokens: number;
  budget: number;
  sections: readonly unknown[];
  omitted: readonly string[];
}): string {
  const sections = summary.sections.length;
  const omitted = summary.omitted.length;
  const parts = [
    `~${summary.tokens.toLocaleString("pt-BR")} tokens de ${summary.budget.toLocaleString("pt-BR")}`,
    `${sections} ${sections === 1 ? "seção" : "seções"}`,
  ];
  if (omitted > 0) parts.push(`${omitted} ${omitted === 1 ? "corte" : "cortes"} pelo orçamento`);
  return parts.join(" · ");
}

export function handoffStatusLabel(handoff: Pick<Handoff, "status">): string {
  return handoff.status === "accepted" ? "assumido" : "pendente";
}

/** Task text the router and the context use for a handoff. */
export function handoffTask(packet: Pick<HandoffPacket, "goal" | "nextAction">): string {
  return [packet.goal, packet.nextAction].filter((part) => part.trim()).join(". ");
}
