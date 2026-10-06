import { describe, expect, it } from "vitest";
import {
  contextProblem,
  contextWarning,
  isNewerEngine,
  memoryNeeded,
  progressText,
  repoId,
  serverLine,
  sourceLabel,
  systemLine,
} from "./local";
import type { LocalModel } from "./types";

const GB = 1024 ** 3;

function model(patch: Partial<LocalModel> = {}): LocalModel {
  return {
    id: "qwen3-8b",
    name: "Qwen3 8B",
    files: ["/d/local/models/qwen3-8b/Qwen3-8B-Q4_K_M.gguf"],
    size: 5 * GB,
    sha256: null,
    source: { kind: "catalog", entry: "qwen3-8b", repo: "Qwen/Qwen3-8B-GGUF", file: "Qwen3-8B-Q4_K_M.gguf" },
    context: 16_384,
    trainedContext: 40_960,
    architecture: "qwen3",
    sizeLabel: "8B",
    quantization: "Q4_K_M",
    tools: true,
    chatTemplate: true,
    kvBytesPerToken: 147_456,
    addedAt: "2026-10-06T00:00:00Z",
    ...patch,
  };
}

describe("local models", () => {
  it("describes this computer and the engine", () => {
    expect(systemLine({ os: "windows", arch: "x64", nvidia: true, vulkan: true, memoryBytes: 16 * GB })).toBe(
      "Windows x64 · 16 GB de memória · placa NVIDIA",
    );
    expect(systemLine({ os: "macos", arch: "arm64", nvidia: false, vulkan: false, memoryBytes: null })).toBe(
      "macOS arm64 · Apple Silicon",
    );
    expect(serverLine({ state: "stopped" })).toContain("liga sozinho");
    expect(serverLine({ state: "ready", model: "qwen3-8b", port: 1, context: 16_384 })).toBe(
      "Rodando qwen3-8b · contexto 16.384 tokens",
    );
  });

  it("checks the context a model runs with", () => {
    const m = model();
    expect(contextProblem(m, 32_768)).toBeNull();
    expect(contextProblem(m, 65_536)).toContain("40.960");
    expect(contextProblem(m, 1000)).toContain("2.048");
    expect(contextWarning(8192)).toContain("10 mil");
    expect(contextWarning(16_384)).toBeNull();
    // 5 GB + 16k × 144 KB + 0,5 GB
    expect(memoryNeeded(m)).toBe(5 * GB + 16_384 * 147_456 + 512 * 1024 * 1024);
  });

  it("names sources, progress and engine updates", () => {
    expect(sourceLabel({ kind: "ollama", name: "phi4-mini:latest" })).toBe("importado do Ollama (phi4-mini:latest)");
    expect(sourceLabel({ kind: "huggingFace", repo: "a/b", file: "x.gguf" })).toBe("Hugging Face · a/b");
    expect(progressText(512, null)).toBe("512 B");
    expect(progressText(GB, 4 * GB)).toBe("25% · 1.0 GB de 4.0 GB");
    expect(isNewerEngine("b9000", "b9100")).toBe(true);
    expect(isNewerEngine("b9100", "b9100")).toBe(false);
    expect(repoId(" https://huggingface.co/Qwen/Qwen3-8B-GGUF/ ")).toBe("Qwen/Qwen3-8B-GGUF");
    expect(repoId("a/b")).toBe("a/b");
  });
});
