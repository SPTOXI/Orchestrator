// Formatting and path helpers (pure functions, unit tested).

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 2 : 1)} s`;
  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds % 60);
  return `${minutes} min ${rest} s`;
}

export function formatTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleTimeString("pt-BR", { hour12: false });
}

/** Separator used by a path ("\\" for Windows-style paths). */
export function separatorOf(path: string): "/" | "\\" {
  return path.includes("\\") && !path.includes("/") ? "\\" : "/";
}

export function joinPath(dir: string, name: string): string {
  const sep = separatorOf(dir);
  if (dir.endsWith("/") || dir.endsWith("\\")) return `${dir}${name}`;
  return `${dir}${sep}${name}`;
}

/** Last path segment ("" for a root). */
export function baseName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return index === -1 ? trimmed : trimmed.slice(index + 1);
}

/** Parent directory; a root ("/", "C:\\") is its own parent. */
export function parentPath(path: string): string {
  const sep = separatorOf(path);
  const trimmed = path.length > 1 ? path.replace(/[\\/]+$/, "") : path;
  if (/^[A-Za-z]:$/.test(trimmed)) return `${trimmed}${sep}`;
  const index = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (index < 0) return path;
  if (index === 0) return trimmed.slice(0, 1);
  const parent = trimmed.slice(0, index);
  return /^[A-Za-z]:$/.test(parent) ? `${parent}${sep}` : parent;
}

/** True when `path` is `dir` itself or inside it. */
export function isInside(path: string, dir: string): boolean {
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");
  const a = norm(path);
  const b = norm(dir);
  return a === b || a.startsWith(`${b}/`);
}

export type Eol = "\n" | "\r\n";

/** Line ending used by a text (CRLF when any line uses it). */
export function detectEol(text: string): Eol {
  return text.includes("\r\n") ? "\r\n" : "\n";
}

/** Normalizes CRLF to LF (what a <textarea> does to its value). */
export function toLf(text: string): string {
  return text.replace(/\r\n/g, "\n");
}

/** Restores the file's line ending before writing it back. */
export function withEol(text: string, eol: Eol): string {
  return eol === "\r\n" ? toLf(text).replace(/\n/g, "\r\n") : text;
}

// ANSI escape sequences (colors, cursor movement) for plain-text views.
const ANSI = /\u001b\[[0-9;?]*[ -/]*[@-~]|\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)|\u001b[@-Z\\-_]/g;

export function stripAnsi(text: string): string {
  return text.replace(ANSI, "");
}
