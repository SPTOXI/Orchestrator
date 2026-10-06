// Pure helpers for "Modelos locais" (ADR-0025). Unit tested.

import { formatBytes } from "./format";
import type { LocalBackend, LocalModel, LocalModelSource, LocalServerStatus, LocalSystem } from "./types";

/** Instructions and tools take about 10k tokens: below this, little room. */
export const MIN_COMFORTABLE_CONTEXT = 16_384;

export const BACKEND_LABELS: Record<LocalBackend, string> = {
  cpu: "Processador (CPU)",
  vulkan: "Placa de vídeo (Vulkan)",
  cuda: "Placa NVIDIA (CUDA)",
  metal: "Apple (Metal)",
};

/** "Windows x64 · 16 GB de memória · placa NVIDIA" */
export function systemLine(system: LocalSystem): string {
  const os = { windows: "Windows", macos: "macOS", linux: "Linux", other: "Sistema" }[system.os];
  const parts = [`${os} ${system.arch === "other" ? "" : system.arch}`.trim()];
  if (system.memoryBytes) parts.push(`${Math.round(system.memoryBytes / 1024 ** 3)} GB de memória`);
  if (system.nvidia) parts.push("placa NVIDIA");
  else if (system.vulkan) parts.push("placa com Vulkan");
  else if (system.os === "macos" && system.arch === "arm64") parts.push("Apple Silicon");
  else parts.push("sem placa de vídeo detectada");
  return parts.join(" · ");
}

export function serverLine(status: LocalServerStatus): string {
  switch (status.state) {
    case "stopped":
      return "Desligado: liga sozinho quando uma sessão usa um modelo local.";
    case "starting":
      return `Ligando ${status.model}…`;
    case "ready":
      return `Rodando ${status.model} · contexto ${status.context.toLocaleString("pt-BR")} tokens`;
    case "failed":
      return `Falhou ao ligar ${status.model}: ${status.error}`;
  }
}

export function sourceLabel(source: LocalModelSource): string {
  switch (source.kind) {
    case "catalog":
      return "catálogo";
    case "huggingFace":
      return `Hugging Face · ${source.repo}`;
    case "ollama":
      return `importado do Ollama (${source.name})`;
    case "file":
      return "arquivo do computador";
  }
}

/** Memory to run it at its context, roughly. */
export function memoryNeeded(model: LocalModel, context = model.context): number {
  return model.size + (model.kvBytesPerToken ?? 0) * context + 512 * 1024 * 1024;
}

/** "precisa de ≈ 7.4 GB de memória" */
export function memoryLine(model: LocalModel, context = model.context): string {
  return `precisa de ≈ ${formatBytes(memoryNeeded(model, context))} de memória`;
}

/** Why a context is a bad idea, if it is. */
export function contextProblem(model: LocalModel, context: number): string | null {
  if (!Number.isInteger(context) || context < 2048) return "use pelo menos 2.048 tokens";
  if (model.trainedContext !== null && context > model.trainedContext) {
    return `o modelo foi treinado para até ${model.trainedContext.toLocaleString("pt-BR")} tokens`;
  }
  return null;
}

export function contextWarning(context: number): string | null {
  return context < MIN_COMFORTABLE_CONTEXT
    ? "Pouco: as instruções e ferramentas do Orchestrator ocupam ≈ 10 mil tokens, sobra pouco para a conversa."
    : null;
}

/** b9100 is newer than b9000. */
export function isNewerEngine(current: string, latest: string): boolean {
  const n = (tag: string) => Number(/(\d+)/.exec(tag)?.[1] ?? NaN);
  const [a, b] = [n(current), n(latest)];
  return Number.isFinite(a) && Number.isFinite(b) ? b > a : current !== latest;
}

/** "42% · 2,1 GB de 5,0 GB" */
export function progressText(done: number, total: number | null): string {
  if (!total) return formatBytes(done);
  return `${Math.floor((done / total) * 100)}% · ${formatBytes(done)} de ${formatBytes(total)}`;
}

/** "owner/repo" from what was typed (a Hugging Face URL works too), as the
 * app keys its downloads. */
export function repoId(text: string): string {
  return text
    .trim()
    .replace(/\/+$/, "")
    .replace(/^(https?:\/\/)?huggingface\.co\//, "")
    .split("/")
    .slice(0, 2)
    .join("/");
}
