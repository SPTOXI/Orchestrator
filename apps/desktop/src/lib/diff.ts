// Unified diff parsing for the diff view (pure, unit tested).

export type DiffLineKind = "file" | "meta" | "hunk" | "add" | "del" | "context";

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
  /** Line number in the old file (context and deletions). */
  oldLine?: number;
  /** Line number in the new file (context and additions). */
  newLine?: number;
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;
const META_PREFIXES = [
  "index ",
  "--- ",
  "+++ ",
  "new file mode",
  "deleted file mode",
  "old mode",
  "new mode",
  "similarity index",
  "dissimilarity index",
  "rename from",
  "rename to",
  "copy from",
  "copy to",
  "Binary files",
  "\\ No newline",
];

export function parseUnifiedDiff(patch: string): DiffLine[] {
  const lines = patch.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  const out: DiffLine[] = [];
  let oldLine = 0;
  let newLine = 0;
  let inHunk = false;

  for (const text of lines) {
    if (text.startsWith("diff --git ")) {
      inHunk = false;
      out.push({ kind: "file", text });
      continue;
    }
    const hunk = HUNK.exec(text);
    if (hunk) {
      inHunk = true;
      oldLine = Number(hunk[1]);
      newLine = Number(hunk[2]);
      out.push({ kind: "hunk", text });
      continue;
    }
    if (!inHunk || META_PREFIXES.some((p) => text.startsWith(p))) {
      out.push({ kind: "meta", text });
      continue;
    }
    if (text.startsWith("+")) {
      out.push({ kind: "add", text, newLine: newLine++ });
    } else if (text.startsWith("-")) {
      out.push({ kind: "del", text, oldLine: oldLine++ });
    } else {
      out.push({ kind: "context", text, oldLine: oldLine++, newLine: newLine++ });
    }
  }
  return out;
}
