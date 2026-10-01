import { describe, expect, it } from "vitest";
import type { UpdateStatus } from "./types";
import { bundleLabel, checkedText, formatBytes, progressText, updateChip } from "./updates";

const status = (over: Partial<UpdateStatus> = {}): UpdateStatus => ({
  version: "0.1.0",
  commit: null,
  os: "linux",
  arch: "x86_64",
  bundle: null,
  configured: true,
  notConfigured: null,
  endpoint: null,
  autoCheck: true,
  lastCheck: null,
  lastError: null,
  phase: "idle",
  available: null,
  warning: null,
  ...over,
});

describe("updates", () => {
  it("names the kind of installation", () => {
    expect(bundleLabel("deb")).toBe("pacote .deb");
    expect(bundleLabel("nsis")).toBe("instalador .exe");
    expect(bundleLabel(null)).toBe("build local, sem instalador");
    expect(bundleLabel("novo")).toBe("novo");
  });

  it("writes sizes and progress", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(12_500_000)).toBe("12,5 MB");
    expect(progressText(12_500_000, 50_000_000)).toBe("12,5 MB de 50 MB (25%)");
    expect(progressText(2_000, null)).toBe("2 kB");
  });

  it("says when it last looked", () => {
    const now = new Date("2026-10-01T12:00:00Z");
    expect(checkedText(null, now)).toBe("ainda não procurou");
    expect(checkedText("2026-10-01T11:59:40Z", now)).toBe("agora há pouco");
    expect(checkedText("2026-10-01T11:55:00Z", now)).toBe("há 5 min");
    expect(checkedText("2026-10-01T09:00:00Z", now)).toBe("há 3 h");
  });

  it("shows a chip only when there is something to do", () => {
    expect(updateChip(null)).toBeNull();
    expect(updateChip(status())).toBeNull();
    const available = { version: "0.2.0", currentVersion: "0.1.0", date: null, notes: null };
    expect(updateChip(status({ available }))).toBe("Atualização 0.2.0");
    expect(updateChip(status({ available, phase: "downloading" }))).toBe("Baixando atualização…");
    expect(updateChip(status({ available, phase: "installed" }))).toBe("Reiniciar para atualizar");
  });
});
