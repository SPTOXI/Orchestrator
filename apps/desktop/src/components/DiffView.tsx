// Main area: colored unified diff of one file (git.diff).

import { useCallback, useEffect, useMemo, useState } from "react";
import { parseUnifiedDiff } from "../lib/diff";
import { errorMessage, gitApi } from "../lib/runtime";
import type { GitDiff } from "../lib/types";
import { RefreshIcon } from "./icons";

interface Props {
  repo: string;
  file: string;
  staged: boolean;
  active: boolean;
  /** Changes when the repository status changes, to reload the diff. */
  version: string;
}

export function DiffView({ repo, file, staged, active, version }: Props) {
  const [diff, setDiff] = useState<GitDiff | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setDiff(await gitApi.diff({ path: repo, files: [file], staged }));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [repo, file, staged]);

  useEffect(() => {
    void load();
  }, [load, version]);

  const lines = useMemo(() => parseUnifiedDiff(diff?.patch ?? ""), [diff]);
  const stats = diff?.files[0];

  return (
    <div className="editor" hidden={!active}>
      <div className="editor-toolbar">
        <span className="mono path" title={`${repo} — ${file}`}>
          {file}
        </span>
        <span className="badge">{staged ? "staged" : "working tree"}</span>
        {stats && !stats.binary && (
          <span className="meta">
            <span className="diff-add-count">+{stats.additions ?? 0}</span>{" "}
            <span className="diff-del-count">−{stats.deletions ?? 0}</span>
          </span>
        )}
        <button className="icon-button" title="Recarregar" onClick={() => void load()}>
          <RefreshIcon />
        </button>
      </div>
      {error && <div className="inline-error">{error}</div>}
      {diff?.truncated && <div className="inline-notice">Diff grande: exibindo só o início.</div>}
      {diff && lines.length === 0 && <div className="empty">Sem diferenças.</div>}
      {stats?.binary && <div className="empty">Arquivo binário.</div>}
      <div className="diff mono">
        {lines.map((line, index) => (
          <div key={index} className={`diff-line diff-${line.kind}`}>
            <span className="diff-num">{line.oldLine ?? ""}</span>
            <span className="diff-num">{line.newLine ?? ""}</span>
            <span className="diff-text">{line.text || " "}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
