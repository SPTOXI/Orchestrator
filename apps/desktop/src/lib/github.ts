// Pure helpers of the GitHub screens (unit tested; ADR-0017). What GitHub
// says comes from the runtime; here we only name it and suggest defaults.

import type { CheckState, GitHubStatus, MergeMethod, PullReview, PullSummary, Task } from "./types";

export const CHECK_LABELS: Record<CheckState, string> = {
  success: "CI passou",
  failure: "CI falhou",
  pending: "CI rodando",
  none: "sem CI",
};

/** One character for lists: ✓ ✗ ● or nothing. */
export const CHECK_MARKS: Record<CheckState, string> = {
  success: "✓",
  failure: "✗",
  pending: "●",
  none: "",
};

export const PULL_STATE_LABELS: Record<PullSummary["state"], string> = {
  open: "aberto",
  closed: "fechado",
  merged: "integrado",
};

export const REVIEW_LABELS: Record<PullReview["state"], string> = {
  approved: "aprovou",
  changes_requested: "pediu mudanças",
  commented: "comentou",
  dismissed: "revisão descartada",
};

export const MERGE_LABELS: Record<MergeMethod, string> = {
  merge: "Merge commit",
  squash: "Squash and merge",
  rebase: "Rebase and merge",
};

/** Where the token in use comes from, in words. */
export function tokenSourceLabel(source: string | null): string {
  if (!source) return "nenhum token";
  if (source === "vault") return "token salvo no cofre do sistema";
  if (source === "gh") return "token do GitHub CLI (gh auth login)";
  if (source.startsWith("env:")) return `variável de ambiente ${source.slice(4)}`;
  return source;
}

/** What `mergeableState` means for the button. */
export function mergeHint(state: string | null, mergeable: boolean | null): string | null {
  if (mergeable === null) return "O GitHub ainda está calculando se dá para fazer o merge.";
  switch (state) {
    case "dirty":
      return "Há conflito com a base: resolva antes do merge.";
    case "blocked":
      return "A proteção da branch bloqueia o merge (revisão ou CI obrigatória).";
    case "behind":
      return "A branch está atrás da base; o repositório exige atualizá-la.";
    case "unstable":
      return "Há verificações falhando; o GitHub ainda permite o merge.";
    case "draft":
      return "É um rascunho: marque como pronto no GitHub antes do merge.";
    default:
      return mergeable ? null : "O GitHub não permite o merge agora.";
  }
}

/** What must happen before a pull request can be opened from the branch. */
export function pushState(status: Pick<GitHubStatus, "branch" | "upstream" | "ahead">): {
  ready: boolean;
  message: string | null;
} {
  if (!status.branch) return { ready: false, message: "HEAD destacado: faça checkout de uma branch." };
  if (!status.upstream)
    return { ready: false, message: `A branch ${status.branch} ainda não está no GitHub: envie (push) primeiro.` };
  if (status.ahead > 0)
    return {
      ready: false,
      message: `A branch ${status.branch} tem ${status.ahead} ${status.ahead === 1 ? "commit" : "commits"} não enviados: envie (push) primeiro.`,
    };
  return { ready: true, message: null };
}

/** "feat/retentativas-no-gateway" → "Retentativas no gateway". */
export function titleFromBranch(branch: string | null): string {
  if (!branch) return "";
  const last = branch.split("/").pop() ?? branch;
  const words = last.replace(/[-_]+/g, " ").trim();
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : "";
}

/** Title and description of a pull request made from a task. */
export function fromTask(task: Pick<Task, "title" | "description" | "result" | "files">): {
  title: string;
  body: string;
} {
  const parts: string[] = [];
  if (task.description.trim()) parts.push(task.description.trim());
  if (task.result.trim()) parts.push(`## Resultado\n\n${task.result.trim()}`);
  if (task.files.length > 0) parts.push(`## Arquivos\n\n${task.files.map((f) => `- \`${f}\``).join("\n")}`);
  return { title: task.title.trim(), body: parts.join("\n\n") };
}
