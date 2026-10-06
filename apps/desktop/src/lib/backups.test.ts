import { describe, expect, it } from "vitest";
import { backupMeta, dataChip, isWarning } from "./backups";
import type { BackupInfo } from "./types";

const backup = (over: Partial<BackupInfo> = {}): BackupInfo => ({
  id: "20261002-101500-atualizacao",
  createdAt: "2026-10-02T10:15:00Z",
  reason: "update",
  label: "Antes de atualizar da 0.1.0 para a 0.2.0",
  appVersion: "0.1.0",
  schema: 4,
  files: ["connections.json", "rules.md", "skills/revisar-pr/SKILL.md"],
  sizeBytes: 2048,
  ...over,
});

describe("backups", () => {
  it("says why, which version, how big and what is in it", () => {
    const meta = backupMeta(backup());
    expect(meta).toContain("antes de uma atualização");
    expect(meta).toContain("versão 0.1.0");
    expect(meta).toContain("3 arquivos de configuração");
    expect(meta).not.toContain("sem banco");
    expect(backupMeta(backup({ schema: null, files: ["rules.md"], reason: "manual" }))).toMatch(
      /feito por você.*1 arquivo de configuração · sem banco$/,
    );
  });

  it("tells the status bar what the start did", () => {
    expect(dataChip([])).toBeNull();
    expect(dataChip(["Backup feito ao abrir: Versão nova aberta: 0.1.0 → 0.2.0 (x)."])).toEqual({
      text: "Backup feito ao abrir",
      warn: false,
    });
    expect(dataChip(["Dados restaurados do backup \"bom\" (02/10/2026 10:15 UTC)."])).toEqual({
      text: "Dados restaurados",
      warn: false,
    });
    const kept = "mcp.json inválido (x) — o conteúdo original foi guardado em /d/mcp.json.unreadable-20261002";
    expect(isWarning(kept)).toBe(true);
    expect(dataChip([kept])).toEqual({ text: "Dados: veja o aviso", warn: true });
    expect(dataChip(["O backup ao abrir não foi feito: disco cheio."])?.warn).toBe(true);
  });
});
