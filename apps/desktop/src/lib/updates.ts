// Words for the "Sobre e atualizações" tab (ADR-0019).

import type { UpdateStatus } from "./types";

const BUNDLES: Record<string, string> = {
  deb: "pacote .deb",
  rpm: "pacote .rpm",
  appimage: "AppImage",
  msi: "instalador MSI",
  nsis: "instalador .exe",
  app: "app do macOS",
  dmg: "app do macOS",
};

/** How this copy was installed. */
export function bundleLabel(bundle: string | null): string {
  if (!bundle) return "build local, sem instalador";
  return BUNDLES[bundle] ?? bundle;
}

/** "12,5 MB" (decimal units, as download sizes are usually given). */
export function formatBytes(bytes: number): string {
  if (bytes < 1000) return `${bytes} B`;
  const units = ["kB", "MB", "GB"];
  let value = bytes;
  let unit = "B";
  for (const next of units) {
    if (value < 1000) break;
    value /= 1000;
    unit = next;
  }
  return `${value.toLocaleString("pt-BR", { maximumFractionDigits: 1 })} ${unit}`;
}

/** "12,5 MB de 50 MB (25%)", or just what came when the size is unknown. */
export function progressText(downloaded: number, total: number | null): string {
  if (!total) return formatBytes(downloaded);
  const percent = Math.min(100, Math.floor((downloaded / total) * 100));
  return `${formatBytes(downloaded)} de ${formatBytes(total)} (${percent}%)`;
}

/** "agora há pouco", "há 5 min", "há 3 h", or the date. */
export function checkedText(lastCheck: string | null, now: Date = new Date()): string {
  if (!lastCheck) return "ainda não procurou";
  const at = new Date(lastCheck);
  const minutes = Math.floor((now.getTime() - at.getTime()) / 60_000);
  if (minutes < 1) return "agora há pouco";
  if (minutes < 60) return `há ${minutes} min`;
  if (minutes < 24 * 60) return `há ${Math.floor(minutes / 60)} h`;
  return at.toLocaleDateString("pt-BR");
}

/** What the status bar says about updates, if anything. */
export function updateChip(status: UpdateStatus | null): string | null {
  if (!status) return null;
  if (status.phase === "installed") return "Reiniciar para atualizar";
  if (status.phase === "downloading") return "Baixando atualização…";
  if (status.available) return `Atualização ${status.available.version}`;
  return null;
}
