// Data and backups (ADR-0022): how the screen and the status bar tell
// about them.

import { formatBytes } from "./format";
import type { BackupInfo, BackupReason } from "./types";

export const REASONS: Record<BackupReason, string> = {
  manual: "feito por você",
  update: "antes de uma atualização",
  newVersion: "ao abrir uma versão nova",
  migration: "antes de atualizar o banco",
  beforeRestore: "antes de uma restauração",
};

/** Date, why, version, size and what it holds. */
export function backupMeta(backup: BackupInfo, locale = "pt-BR"): string {
  const parts = [
    new Date(backup.createdAt).toLocaleString(locale),
    REASONS[backup.reason],
    `versão ${backup.appVersion}`,
    formatBytes(backup.sizeBytes),
    backup.files.length === 1 ? "1 arquivo de configuração" : `${backup.files.length} arquivos de configuração`,
  ];
  if (backup.schema === null) parts.push("sem banco");
  return parts.join(" · ");
}

/** A notice of the start that asks for the user's attention. */
export function isWarning(notice: string): boolean {
  return /não foi|parou|ilegível|guardado em|guardar uma cópia/.test(notice);
}

/** What the start did to the data, in a few words for the status bar. */
export function dataChip(notices: string[]): { text: string; warn: boolean } | null {
  if (notices.length === 0) return null;
  const warn = notices.some(isWarning);
  if (notices.some((n) => n.startsWith("Dados restaurados"))) return { text: "Dados restaurados", warn };
  if (warn) return { text: "Dados: veja o aviso", warn };
  return { text: "Backup feito ao abrir", warn };
}
