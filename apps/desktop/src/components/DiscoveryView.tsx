// Main area: find projects on disk (project.discover) and open one.

import { type FormEvent, useState } from "react";
import { errorMessage, projectApi } from "../lib/runtime";
import type { DiscoverOutput } from "../lib/types";

interface Props {
  active: boolean;
  onOpenProject: (path: string) => void;
}

export function DiscoveryView({ active, onOpenProject }: Props) {
  const [roots, setRoots] = useState("");
  const [depth, setDepth] = useState(4);
  const [filter, setFilter] = useState("");
  const [result, setResult] = useState<DiscoverOutput | null>(null);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const scan = async (e?: FormEvent) => {
    e?.preventDefault();
    setScanning(true);
    setError(null);
    try {
      const list = roots
        .split("\n")
        .map((r) => r.trim())
        .filter(Boolean);
      setResult(await projectApi.discover({ roots: list.length ? list : undefined, maxDepth: depth }));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setScanning(false);
    }
  };

  const needle = filter.trim().toLowerCase();
  const projects = (result?.projects ?? []).filter(
    (p) => !needle || p.name.toLowerCase().includes(needle) || p.path.toLowerCase().includes(needle),
  );

  return (
    <div className="editor" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">Procurar projetos</span>
        <span className="meta">
          {result
            ? `${result.projects.length} projetos · ${result.scannedDirs} pastas verificadas${result.truncated ? " (limite atingido)" : ""}`
            : "Localiza pastas com .git, package.json, pyproject.toml, Cargo.toml, go.mod, Dockerfile…"}
        </span>
      </div>
      <form className="discovery-form" onSubmit={scan}>
        <textarea
          className="mono"
          rows={2}
          placeholder="Raízes, uma por linha (vazio = pasta do usuário e pastas comuns como C:\Projetos)"
          value={roots}
          onChange={(e) => setRoots(e.target.value)}
        />
        <div className="row">
          <label className="check">
            profundidade
            <select value={depth} onChange={(e) => setDepth(Number(e.target.value))}>
              {[2, 3, 4, 5, 6].map((d) => (
                <option key={d} value={d}>
                  {d}
                </option>
              ))}
            </select>
          </label>
          <input placeholder="Filtrar resultados…" value={filter} onChange={(e) => setFilter(e.target.value)} />
          <button className="button primary" type="submit" disabled={scanning}>
            {scanning ? "Procurando…" : "Procurar"}
          </button>
        </div>
      </form>
      {error && <div className="inline-error">{error}</div>}
      <ul className="list discovery-list">
        {result && projects.length === 0 && <li className="empty">Nenhum projeto encontrado.</li>}
        {projects.map((p) => (
          <li key={p.path} className="list-item static">
            <span className={`dot ${p.isGitRepo ? "ok" : "off"}`} title={p.isGitRepo ? "repositório Git" : "sem Git"} />
            <div className="grow">
              <div className="title">{p.name}</div>
              <div className="meta mono ellipsis" title={p.path}>
                {p.path}
              </div>
              <div className="chip-list">
                {p.markers.map((m) => (
                  <span key={m} className="tag small">
                    {m}
                  </span>
                ))}
              </div>
            </div>
            <button className="button small primary" onClick={() => onOpenProject(p.path)}>
              Abrir
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
