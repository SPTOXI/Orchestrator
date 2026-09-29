// Main area: memory of the open project (ADR-0012) — L1 working memory,
// L2 project memory, decisions and L3 search.

import { type FormEvent, useEffect, useState } from "react";
import { handoffStatusLabel } from "../lib/context";
import { formatTime, relativePath } from "../lib/format";
import {
  changeLabel,
  DECISION_STATUS_LABELS,
  exitLabel,
  MEMORY_KIND_LABELS,
  parseTags,
  SOURCE_LABELS,
  splitSnippet,
} from "../lib/memory";
import { errorMessage, memoryApi } from "../lib/runtime";
import type { Memory } from "../lib/useMemory";
import type { DecisionStatus, MemoryEntry, MemoryKind, ProjectDecision, SearchHit } from "../lib/types";
import { PlusIcon } from "./icons";

export type MemorySection = "working" | "project" | "decisions" | "search";

interface Props {
  ready: boolean;
  active: boolean;
  memory: Memory;
  projectId: string | null;
  section: MemorySection;
  /** Search text coming from the sidebar. */
  query: string;
  /** Changes when the sidebar asks for a section again. */
  nonce: number;
  onOpenSession: (id: string) => void;
  onOpenFile: (path: string) => void;
  onOpenHandoff: (handoffId: string) => void;
}

const SECTIONS: Array<[MemorySection, string]> = [
  ["working", "Trabalho (L1)"],
  ["project", "Projeto (L2)"],
  ["decisions", "Decisões"],
  ["search", "Busca (L3)"],
];

const KINDS = Object.keys(MEMORY_KIND_LABELS) as MemoryKind[];
const STATUSES = Object.keys(DECISION_STATUS_LABELS) as DecisionStatus[];

function Snippet({ text }: { text: string }) {
  return (
    <span>
      {splitSnippet(text).map((piece, i) => (piece.match ? <mark key={i}>{piece.text}</mark> : <span key={i}>{piece.text}</span>))}
    </span>
  );
}

function WorkingSection({
  memory,
  onOpenSession,
  onOpenFile,
  onOpenHandoff,
}: Pick<Props, "memory" | "onOpenSession" | "onOpenFile" | "onOpenHandoff">) {
  const working = memory.overview?.working;
  if (!working) return <div className="meta pad">Carregando…</div>;
  return (
    <div className="memory-grid">
      <section>
        <h3>Sessões</h3>
        {working.sessions.length === 0 && <div className="meta">Nenhuma sessão neste projeto.</div>}
        <ul className="plain-list">
          {working.sessions.map((s) => (
            <li key={s.id} className="clickable" onClick={() => onOpenSession(s.id)}>
              <div className="row">
                <span className={`dot ${s.status === "closed" ? "off" : "ok"}`} />
                <strong className="ellipsis">{s.title}</strong>
              </div>
              <span className="meta">
                {s.provider}
                {s.model ? ` / ${s.model}` : ""} · {s.turns} {s.turns === 1 ? "turno" : "turnos"} · {formatTime(s.updatedAt)}
              </span>
            </li>
          ))}
        </ul>
      </section>
      <section>
        <h3>Arquivos alterados</h3>
        {working.files.length === 0 && <div className="meta">Nenhuma alteração registrada.</div>}
        <ul className="plain-list">
          {working.files.map((f) => (
            <li key={f.path} className="clickable" onClick={() => onOpenFile(f.path)} title={f.path}>
              <span className="mono ellipsis">{relativePath(f.path, memory.overview?.project.path)}</span>
              <span className="meta">
                {changeLabel(f.change)} · {f.by === "agent" ? "IA" : f.by === "user" ? "usuário" : f.by} · {formatTime(f.at)}
              </span>
            </li>
          ))}
        </ul>
      </section>
      <section>
        <h3>Comandos</h3>
        {working.commands.length === 0 && <div className="meta">Nenhum comando registrado.</div>}
        <ul className="plain-list">
          {working.commands.map((c, i) => (
            <li key={i} title={c.command}>
              <span className="mono ellipsis">{c.command}</span>
              <span className={`meta ${c.exitCode ? "err-text" : ""}`}>
                {exitLabel(c.exitCode, c.background)} · {formatTime(c.at)}
              </span>
            </li>
          ))}
        </ul>
      </section>
      <section>
        <h3>Handoffs</h3>
        {memory.handoffs.length === 0 && (
          <div className="meta">Nenhum handoff. Use "Handoff" numa sessão para passar o trabalho a outra IA.</div>
        )}
        <ul className="plain-list">
          {memory.handoffs.slice(0, 8).map((h) => (
            <li key={h.id} className="clickable" onClick={() => onOpenHandoff(h.id)} title={h.packet.nextAction}>
              <span className={`badge status-${h.status === "accepted" ? "accepted" : "proposed"}`}>
                {handoffStatusLabel(h)}
              </span>
              <strong className="ellipsis">{h.packet.goal}</strong>
              <span className="meta ellipsis">
                {h.from.provider}
                {h.to ? ` → ${h.to.provider}` : ""} · {formatTime(h.createdAt)}
              </span>
            </li>
          ))}
        </ul>
      </section>
      <section>
        <h3>Erros recentes</h3>
        {working.errors.length === 0 && <div className="meta">Nenhum erro registrado.</div>}
        <ul className="plain-list">
          {working.errors.map((e, i) => (
            <li key={i}>
              <span className="ellipsis">{e.summary}</span>
              <span className="meta err-text ellipsis" title={e.detail ?? undefined}>
                {e.kind} · {formatTime(e.at)}
                {e.detail ? ` · ${e.detail}` : ""}
              </span>
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}

interface EntryDraft {
  id: string | null;
  kind: MemoryKind;
  title: string;
  content: string;
  tags: string;
  pinned: boolean;
}

function toDraft(entry: MemoryEntry | null): EntryDraft {
  return entry
    ? { id: entry.id, kind: entry.kind, title: entry.title, content: entry.content, tags: entry.tags.join(", "), pinned: entry.pinned }
    : { id: null, kind: "note", title: "", content: "", tags: "", pinned: false };
}

function ProjectSection({ ready, memory, projectId, focus }: Pick<Props, "ready" | "memory" | "projectId"> & { focus: string | null }) {
  const [draft, setDraft] = useState<EntryDraft | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);

  useEffect(() => {
    const entry = memory.entries.find((e) => e.id === focus);
    if (entry) setDraft(toDraft(entry));
  }, [focus, memory.entries]);

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!draft || !projectId) return;
    setBusy(true);
    setError(null);
    try {
      await memoryApi.save({
        id: draft.id,
        projectId,
        kind: draft.kind,
        title: draft.title,
        content: draft.content,
        tags: parseTags(draft.tags),
        pinned: draft.pinned,
      });
      setDraft(null);
      await memory.refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (id: string) => {
    if (confirm !== id) {
      setConfirm(id);
      return;
    }
    try {
      await memoryApi.remove(id);
      setConfirm(null);
      if (draft?.id === id) setDraft(null);
      await memory.refresh();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <div className="memory-columns">
      <section>
        <h3 className="row">
          <span className="grow">Entradas ({memory.entries.length})</span>
          <button className="button small" disabled={!ready || !projectId} onClick={() => setDraft(toDraft(null))}>
            <PlusIcon /> Nova entrada
          </button>
        </h3>
        {memory.entries.length === 0 && (
          <div className="meta form-hint">
            Arquitetura, stack, convenções, regras e notas do projeto. A stack detectada entra sozinha ao abrir o projeto.
          </div>
        )}
        <ul className="plain-list entries">
          {memory.entries.map((entry) => (
            <li
              key={entry.id}
              className={`clickable ${draft?.id === entry.id ? "selected" : ""}`}
              onClick={() => setDraft(toDraft(entry))}
            >
              <div className="row">
                {entry.pinned && <span className="badge ok">fixada</span>}
                <span className="tag small">{MEMORY_KIND_LABELS[entry.kind]}</span>
                <strong className="ellipsis grow">{entry.title}</strong>
                <span className="meta">{SOURCE_LABELS[entry.source]}</span>
              </div>
              <div className="meta clamp">{entry.content || "(sem conteúdo)"}</div>
              {entry.tags.length > 0 && (
                <div className="chip-list">
                  {entry.tags.map((t) => (
                    <span key={t} className="tag small">
                      {t}
                    </span>
                  ))}
                </div>
              )}
            </li>
          ))}
        </ul>
      </section>
      <section>
        {draft ? (
          <form className="memory-form" onSubmit={(e) => void save(e)}>
            <h3>{draft.id ? "Editar entrada" : "Nova entrada"}</h3>
            <label className="form-row">
              <span>Tipo</span>
              <select value={draft.kind} onChange={(e) => setDraft({ ...draft, kind: e.target.value as MemoryKind })}>
                {KINDS.map((k) => (
                  <option key={k} value={k}>
                    {MEMORY_KIND_LABELS[k]}
                  </option>
                ))}
              </select>
            </label>
            <label className="form-row">
              <span>Título</span>
              <input value={draft.title} onChange={(e) => setDraft({ ...draft, title: e.target.value })} />
            </label>
            <label className="form-row top">
              <span>Conteúdo</span>
              <textarea
                className="tall"
                value={draft.content}
                onChange={(e) => setDraft({ ...draft, content: e.target.value })}
              />
            </label>
            <label className="form-row">
              <span>Etiquetas</span>
              <input
                value={draft.tags}
                placeholder="banco, filas…"
                onChange={(e) => setDraft({ ...draft, tags: e.target.value })}
              />
            </label>
            <label className="form-row check-row">
              <span>Fixar no topo</span>
              <input type="checkbox" checked={draft.pinned} onChange={(e) => setDraft({ ...draft, pinned: e.target.checked })} />
            </label>
            {error && <div className="inline-error">{error}</div>}
            <div className="row form-actions">
              <button className="button primary" type="submit" disabled={busy || !draft.title.trim()}>
                {busy ? "Salvando…" : "Salvar"}
              </button>
              <button className="button" type="button" onClick={() => setDraft(null)}>
                Cancelar
              </button>
              <span className="grow" />
              {draft.id && (
                <button className="button danger" type="button" onClick={() => void remove(draft.id!)}>
                  {confirm === draft.id ? "Confirmar exclusão" : "Excluir"}
                </button>
              )}
            </div>
          </form>
        ) : (
          <div className="meta pad">Escolha uma entrada ou crie uma nova.</div>
        )}
      </section>
    </div>
  );
}

interface DecisionDraft {
  id: string | null;
  title: string;
  context: string;
  decision: string;
  consequences: string;
  status: DecisionStatus;
}

function toDecisionDraft(d: ProjectDecision | null): DecisionDraft {
  return d
    ? { id: d.id, title: d.title, context: d.context, decision: d.decision, consequences: d.consequences, status: d.status }
    : { id: null, title: "", context: "", decision: "", consequences: "", status: "proposed" };
}

function DecisionsSection({ ready, memory, projectId, focus }: Pick<Props, "ready" | "memory" | "projectId"> & { focus: string | null }) {
  const [draft, setDraft] = useState<DecisionDraft | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const decision = memory.decisions.find((d) => d.id === focus);
    if (decision) setDraft(toDecisionDraft(decision));
  }, [focus, memory.decisions]);

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!draft || !projectId) return;
    setBusy(true);
    setError(null);
    try {
      await memoryApi.saveDecision({ ...draft, projectId });
      setDraft(null);
      await memory.refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const field = (key: "context" | "decision" | "consequences", label: string, placeholder: string) => (
    <label className="form-row top">
      <span>{label}</span>
      <textarea
        value={draft?.[key] ?? ""}
        placeholder={placeholder}
        onChange={(e) => draft && setDraft({ ...draft, [key]: e.target.value })}
      />
    </label>
  );

  return (
    <div className="memory-columns">
      <section>
        <h3 className="row">
          <span className="grow">Decisões ({memory.decisions.length})</span>
          <button className="button small" disabled={!ready || !projectId} onClick={() => setDraft(toDecisionDraft(null))}>
            <PlusIcon /> Nova decisão
          </button>
        </h3>
        {memory.decisions.length === 0 && (
          <div className="meta form-hint">Decisões não são apagadas: mudam de estado (proposta, aceita, substituída, rejeitada).</div>
        )}
        <ul className="plain-list entries">
          {memory.decisions.map((d) => (
            <li
              key={d.id}
              className={`clickable ${draft?.id === d.id ? "selected" : ""}`}
              onClick={() => setDraft(toDecisionDraft(d))}
            >
              <div className="row">
                <span className={`badge status-${d.status}`}>{DECISION_STATUS_LABELS[d.status]}</span>
                <strong className="ellipsis grow">{d.title}</strong>
                <span className="meta">{formatTime(d.createdAt)}</span>
              </div>
              <div className="meta clamp">{d.decision}</div>
            </li>
          ))}
        </ul>
      </section>
      <section>
        {draft ? (
          <form className="memory-form" onSubmit={(e) => void save(e)}>
            <h3>{draft.id ? "Editar decisão" : "Nova decisão"}</h3>
            <label className="form-row">
              <span>Título</span>
              <input value={draft.title} onChange={(e) => setDraft({ ...draft, title: e.target.value })} />
            </label>
            <label className="form-row">
              <span>Estado</span>
              <select value={draft.status} onChange={(e) => setDraft({ ...draft, status: e.target.value as DecisionStatus })}>
                {STATUSES.map((s) => (
                  <option key={s} value={s}>
                    {DECISION_STATUS_LABELS[s]}
                  </option>
                ))}
              </select>
            </label>
            {field("context", "Contexto", "Por que a decisão é necessária")}
            {field("decision", "Decisão", "O que foi decidido")}
            {field("consequences", "Consequências", "O que muda, custos e riscos")}
            {error && <div className="inline-error">{error}</div>}
            <div className="row form-actions">
              <button className="button primary" type="submit" disabled={busy || !draft.title.trim() || !draft.decision.trim()}>
                {busy ? "Salvando…" : "Salvar"}
              </button>
              <button className="button" type="button" onClick={() => setDraft(null)}>
                Cancelar
              </button>
            </div>
          </form>
        ) : (
          <div className="meta pad">Escolha uma decisão ou registre uma nova.</div>
        )}
      </section>
    </div>
  );
}

const HIT_LABELS: Record<SearchHit["kind"], string> = {
  memory: "memória",
  decision: "decisão",
  message: "sessão",
  event: "evento",
  handoff: "handoff",
};

export function MemoryView({
  ready,
  active,
  memory,
  projectId,
  section: initialSection,
  query,
  nonce,
  onOpenSession,
  onOpenFile,
  onOpenHandoff,
}: Props) {
  const [section, setSection] = useState<MemorySection>(initialSection);
  const [text, setText] = useState(query);
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [focus, setFocus] = useState<string | null>(null);

  const search = async (value: string) => {
    if (!projectId || !value.trim()) {
      setHits(null);
      return;
    }
    setSearching(true);
    setError(null);
    try {
      setHits(await memoryApi.search(projectId, value));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSearching(false);
    }
  };

  // The sidebar asked for a section (and maybe a search).
  useEffect(() => {
    setSection(initialSection);
    if (initialSection === "search") {
      setText(query);
      void search(query);
    }
  }, [nonce]); // Only a new request from the sidebar re-syncs the view.

  const openHit = (hit: SearchHit) => {
    switch (hit.kind) {
      case "memory":
        setFocus(hit.refId);
        setSection("project");
        break;
      case "decision":
        setFocus(hit.refId);
        setSection("decisions");
        break;
      case "message":
        onOpenSession(hit.refId);
        break;
      case "handoff":
        onOpenHandoff(hit.refId);
        break;
      case "event":
        break;
    }
  };

  const overview = memory.overview;
  return (
    <div className="editor memory-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">Memória do projeto</span>
        <span className="meta grow ellipsis">
          {overview
            ? `${overview.project.name} · ${overview.memoryEntries} entradas · ${overview.decisions} decisões · ${overview.sessions} sessões · ${overview.events} eventos`
            : "Abra um projeto: a memória pertence ao projeto."}
        </span>
      </div>
      <div className="tabs memory-tabs">
        {SECTIONS.map(([id, label]) => (
          <button key={id} className={`tab ${section === id ? "active" : ""}`} onClick={() => setSection(id)}>
            {label}
          </button>
        ))}
      </div>
      {memory.error && <div className="inline-error">{memory.error}</div>}
      <div className="memory-body">
        {!projectId ? (
          <div className="meta pad">Abra um projeto para ver a memória dele.</div>
        ) : section === "working" ? (
          <WorkingSection
            memory={memory}
            onOpenSession={onOpenSession}
            onOpenFile={onOpenFile}
            onOpenHandoff={onOpenHandoff}
          />
        ) : section === "project" ? (
          <ProjectSection ready={ready} memory={memory} projectId={projectId} focus={focus} />
        ) : section === "decisions" ? (
          <DecisionsSection ready={ready} memory={memory} projectId={projectId} focus={focus} />
        ) : (
          <section className="memory-search">
            <form
              className="row"
              onSubmit={(e) => {
                e.preventDefault();
                void search(text);
              }}
            >
              <input
                className="grow"
                value={text}
                placeholder="Buscar em memória, decisões, conversas das sessões e erros…"
                onChange={(e) => setText(e.target.value)}
              />
              <button className="button primary" type="submit" disabled={!ready || searching}>
                {searching ? "Buscando…" : "Buscar"}
              </button>
            </form>
            {error && <div className="inline-error">{error}</div>}
            {hits && hits.length === 0 && <div className="meta pad">Nada encontrado.</div>}
            <ul className="plain-list hits">
              {(hits ?? []).map((hit, i) => (
                <li key={`${hit.kind}:${hit.refId}:${i}`} className={hit.kind === "event" ? "" : "clickable"} onClick={() => openHit(hit)}>
                  <div className="row">
                    <span className="tag small">{HIT_LABELS[hit.kind]}</span>
                    <strong className="ellipsis grow">{hit.title}</strong>
                    <span className="meta">{formatTime(hit.at)}</span>
                  </div>
                  <div className="meta">
                    <Snippet text={hit.snippet} />
                  </div>
                </li>
              ))}
            </ul>
          </section>
        )}
      </div>
    </div>
  );
}
