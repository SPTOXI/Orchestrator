// Pure helpers for the MEMORY views (unit tested; ADR-0012).

import type { DecisionStatus, MemoryKind, MemorySource } from "./types";

export const MEMORY_KIND_LABELS: Record<MemoryKind, string> = {
  architecture: "Arquitetura",
  stack: "Stack",
  convention: "Convenção",
  rule: "Regra",
  note: "Nota",
};

export const DECISION_STATUS_LABELS: Record<DecisionStatus, string> = {
  proposed: "Proposta",
  accepted: "Aceita",
  superseded: "Substituída",
  rejected: "Rejeitada",
};

export const SOURCE_LABELS: Record<MemorySource, string> = {
  user: "usuário",
  agent: "IA",
  detector: "detectado",
};

const CHANGE_LABELS: Record<string, string> = {
  created: "criado",
  modified: "modificado",
  written: "modificado",
  deleted: "removido",
  moved: "movido",
};

export function changeLabel(change: string): string {
  return CHANGE_LABELS[change] ?? change;
}

/** "banco, SQL,  banco" → ["banco", "SQL"] (no blanks, no repeats). */
export function parseTags(text: string): string[] {
  const out: string[] = [];
  for (const raw of text.split(",")) {
    const tag = raw.trim();
    if (tag && !out.some((t) => t.toLowerCase() === tag.toLowerCase())) out.push(tag);
  }
  return out;
}

/** Snippet with the matching terms between `[` and `]` → pieces to render. */
export function splitSnippet(snippet: string): Array<{ text: string; match: boolean }> {
  const out: Array<{ text: string; match: boolean }> = [];
  const re = /\[([^\]]*)\]/g;
  let last = 0;
  for (let m = re.exec(snippet); m; m = re.exec(snippet)) {
    if (m.index > last) out.push({ text: snippet.slice(last, m.index), match: false });
    out.push({ text: m[1] ?? "", match: true });
    last = m.index + m[0].length;
  }
  if (last < snippet.length) out.push({ text: snippet.slice(last), match: false });
  return out;
}

/** Exit code as shown in L1: "ok", "saída 1", "em segundo plano". */
export function exitLabel(exitCode: number | null, background: boolean): string {
  if (background) return "em segundo plano";
  if (exitCode === null) return "sem código";
  return exitCode === 0 ? "ok" : `saída ${exitCode}`;
}
