// Pure helpers for the API connection editor (unit tested).

import type { ApiKind, Connection, ModelEntry, ToolMode } from "./types";

export const KIND_LABELS: Record<ApiKind, string> = {
  openai: "OpenAI (compatível)",
  anthropic: "Anthropic",
  gemini: "Google Gemini",
  generic: "API genérica (perfil)",
};

/** Connection id from a display name: "OpenAI Pessoal" → "openai-pessoal". */
export function slugify(name: string): string {
  return (
    name
      .normalize("NFD")
      .replace(/[̀-ͯ]/g, "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 48) || "api"
  );
}

export function emptyModel(id: string): ModelEntry {
  return {
    id,
    name: null,
    contextWindow: null,
    maxOutputTokens: null,
    supportsTools: null,
    supportsVision: null,
    inputPrice: null,
    outputPrice: null,
    tags: [],
    extraBody: null,
    enabled: true,
  };
}

/** Enabled automatically when a first discovery returns this many or fewer. */
const AUTO_ENABLE_LIMIT = 8;

/**
 * Adds discovered models to the list. What the user already configured
 * wins; missing metadata (context, prices, name) is filled in. New models
 * start disabled unless the list had nothing enabled and few were found.
 */
export function mergeModels(existing: ModelEntry[], found: ModelEntry[]): ModelEntry[] {
  const byId = new Map(existing.map((m) => [m.id, m]));
  const enableNew = !existing.some((m) => m.enabled) && found.length <= AUTO_ENABLE_LIMIT;
  const merged = existing.map((model) => {
    const discovered = found.find((f) => f.id === model.id);
    if (!discovered) return model;
    return {
      ...model,
      name: model.name ?? discovered.name,
      contextWindow: model.contextWindow ?? discovered.contextWindow,
      maxOutputTokens: model.maxOutputTokens ?? discovered.maxOutputTokens,
      supportsTools: model.supportsTools ?? discovered.supportsTools,
      inputPrice: model.inputPrice ?? discovered.inputPrice,
      outputPrice: model.outputPrice ?? discovered.outputPrice,
      tags: model.tags.length ? model.tags : discovered.tags,
    };
  });
  for (const model of found) {
    if (!byId.has(model.id)) merged.push({ ...model, enabled: enableNew });
  }
  return merged;
}

export function defaultToolMode(kind: ApiKind): ToolMode {
  return kind === "generic" ? "prompt" : "native";
}

/** Parses a JSON text field; empty text is `null`. */
export function parseJson(text: string): { ok: true; value: unknown } | { ok: false; error: string } {
  if (!text.trim()) return { ok: true, value: null };
  try {
    return { ok: true, value: JSON.parse(text) };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export function toJsonText(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "object" && Object.keys(value as object).length === 0) return "";
  return JSON.stringify(value, null, 2);
}

/** "US$ 2,50 / 10,00 por 1M" */
export function formatPrices(model: Pick<ModelEntry, "inputPrice" | "outputPrice">): string | null {
  if (model.inputPrice === null && model.outputPrice === null) return null;
  const fmt = (n: number | null) => (n === null ? "?" : n.toLocaleString("pt-BR", { maximumFractionDigits: 4 }));
  return `US$ ${fmt(model.inputPrice)} / ${fmt(model.outputPrice)} por 1M`;
}

/** Copy safe to edit (the preset objects are shared). */
export function cloneConnection(connection: Connection): Connection {
  return JSON.parse(JSON.stringify(connection)) as Connection;
}
