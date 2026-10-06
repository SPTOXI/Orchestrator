// MEMORY panel (sidebar): what the project remembers (ADR-0012) — totals,
// a glance at L1 and a quick search; details open in the main area.

import { type FormEvent, useState } from "react";
import { formatTime, relativePath } from "../lib/format";
import { changeLabel, MEMORY_KIND_LABELS } from "../lib/memory";
import type { Memory } from "../lib/useMemory";
import type { MemorySection } from "./MemoryView";
import { statusDot, statusLabel } from "./ProvidersPanel";

interface Props {
  ready: boolean;
  memory: Memory;
  projectName: string | null;
  database: string | null;
  onOpen: (section: MemorySection, query?: string) => void;
  onOpenSession: (id: string) => void;
}

export function MemoryPanel({ ready, memory, projectName, database, onOpen, onOpenSession }: Props) {
  const [query, setQuery] = useState("");
  const overview = memory.overview;
  const working = overview?.working;

  const search = (event: FormEvent) => {
    event.preventDefault();
    if (query.trim()) onOpen("search", query);
  };

  return (
    <div className="panel">
      <div className="panel-header">
        <span>Memory</span>
        {overview && <span className="meta">{overview.project.name}</span>}
      </div>
      {memory.error && <div className="inline-error">{memory.error}</div>}
      <div className="scroll">
        {!projectName && (
          <div className="placeholder">
            <p>A memória pertence ao projeto.</p>
            <p className="meta">Abra um projeto para ver sessões, arquivos, comandos, erros, memória e decisões dele.</p>
          </div>
        )}
        {overview && (
          <>
            <form className="pad" onSubmit={search}>
              <input
                className="full"
                value={query}
                disabled={!ready}
                placeholder="Buscar na memória (L3)…"
                onChange={(e) => setQuery(e.target.value)}
              />
            </form>
            <ul className="list memory-counts">
              <li className="list-item" onClick={() => onOpen("project")}>
                <span className="grow">Memória do projeto (L2)</span>
                <span className="badge">{overview.memoryEntries}</span>
              </li>
              <li className="list-item" onClick={() => onOpen("decisions")}>
                <span className="grow">Decisões</span>
                <span className="badge">{overview.decisions}</span>
              </li>
              <li className="list-item" onClick={() => onOpen("working")}>
                <span className="grow">Sessões</span>
                <span className="badge">{overview.sessions}</span>
              </li>
              <li className="list-item static">
                <span className="grow">Eventos no histórico</span>
                <span className="badge">{overview.events}</span>
              </li>
            </ul>

            {memory.entries.some((e) => e.pinned) && (
              <>
                <div className="section-title">Fixadas</div>
                <ul className="list">
                  {memory.entries
                    .filter((e) => e.pinned)
                    .slice(0, 5)
                    .map((e) => (
                      <li key={e.id} className="list-item" onClick={() => onOpen("project")} title={e.content}>
                        <div className="grow">
                          <div className="title ellipsis">{e.title}</div>
                          <div className="meta">{MEMORY_KIND_LABELS[e.kind]}</div>
                        </div>
                      </li>
                    ))}
                </ul>
              </>
            )}

            <div className="section-title row">
              <span className="grow">Trabalho agora (L1)</span>
              <button className="link" onClick={() => onOpen("working")}>
                Abrir
              </button>
            </div>
            <ul className="list">
              {working &&
                working.sessions.length === 0 &&
                working.files.length === 0 &&
                working.errors.length === 0 && <li className="empty">Nada registrado ainda.</li>}
              {working?.sessions.slice(0, 2).map((s) => (
                <li key={s.id} className="list-item" onClick={() => onOpenSession(s.id)} title={s.provider}>
                  <span className={`dot ${statusDot(s.status)}`} />
                  <div className="grow">
                    <div className="title ellipsis">{s.title}</div>
                    <div className="meta">
                      {statusLabel(s.status)} · {s.turns} {s.turns === 1 ? "turno" : "turnos"} · {formatTime(s.updatedAt)}
                    </div>
                  </div>
                </li>
              ))}
              {working?.files.slice(0, 5).map((f) => (
                <li key={f.path} className="list-item static" title={f.path}>
                  <div className="grow">
                    <div className="title mono ellipsis">{relativePath(f.path, overview.project.path)}</div>
                    <div className="meta">
                      {changeLabel(f.change)} · {formatTime(f.at)}
                    </div>
                  </div>
                </li>
              ))}
              {working?.errors.slice(0, 3).map((e, i) => (
                <li key={`e${i}`} className="list-item static" title={e.detail ?? e.summary}>
                  <span className="dot err" />
                  <div className="grow">
                    <div className="title ellipsis">{e.summary}</div>
                    <div className="meta err-text ellipsis">{e.detail ?? e.kind}</div>
                  </div>
                </li>
              ))}
            </ul>
          </>
        )}
      </div>
      {database && (
        <div className="panel-footer meta mono ellipsis" title={database}>
          banco: {database}
        </div>
      )}
    </div>
  );
}
