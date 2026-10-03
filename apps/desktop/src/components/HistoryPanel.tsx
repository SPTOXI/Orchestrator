// HISTORY panel: audit events from the database (history_query, ADR-0012)
// plus the live runtime://audit stream.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { auditEvents } from "../lib/events";
import { formatDuration, formatTime } from "../lib/format";
import { errorMessage, memoryApi } from "../lib/runtime";
import type { AuditEvent, EventKind } from "../lib/types";

const MAX_EVENTS = 2000;
const READ_ONLY_TOOL = /\.(list|read)$/;

function originLabel(event: AuditEvent): string {
  switch (event.origin.type) {
    case "user":
      return "usuário";
    case "agent":
      return event.origin.provider
        ? `IA ${event.origin.provider} · sessão …${(event.origin.sessionId ?? event.origin.agentId).slice(-6)}`
        : `agente ${event.origin.agentId}`;
    case "system":
      return "sistema";
    case "council":
      return "Conselho (Full)";
  }
}

/** Successful query (no state change). Uses the catalog's `readOnly` flag;
 * events recorded before it existed fall back to the tool name. */
function isReadOnlyCall(event: AuditEvent): boolean {
  if (event.kind !== "TOOL_CALLED" || event.data.ok !== true) return false;
  if (typeof event.data.readOnly === "boolean") return event.data.readOnly;
  return READ_ONLY_TOOL.test(String(event.data.tool));
}

const KIND_FILTERS: Array<{ value: EventKind | ""; label: string }> = [
  { value: "", label: "Todos os eventos" },
  { value: "TOOL_CALLED", label: "TOOL_CALLED" },
  { value: "FILE_CHANGED", label: "FILE_CHANGED" },
  { value: "COMMAND_EXECUTED", label: "COMMAND_EXECUTED" },
  { value: "PROCESS_EXITED", label: "PROCESS_EXITED" },
  { value: "TERMINAL_EXITED", label: "TERMINAL_EXITED" },
  { value: "PROJECT_OPENED", label: "PROJECT_OPENED" },
  { value: "GIT_COMMIT", label: "GIT_COMMIT" },
  { value: "GIT_PUSH", label: "GIT_PUSH" },
  { value: "PROVIDER_SWITCHED", label: "PROVIDER_SWITCHED" },
  { value: "SESSION_STARTED", label: "SESSION_STARTED" },
  { value: "SESSION_RESUMED", label: "SESSION_RESUMED" },
  { value: "SESSION_CLOSED", label: "SESSION_CLOSED" },
  { value: "TURN_COMPLETED", label: "TURN_COMPLETED" },
  { value: "CONNECTION_SAVED", label: "CONNECTION_SAVED" },
  { value: "CONNECTION_REMOVED", label: "CONNECTION_REMOVED" },
  { value: "COUNCIL_CONFIGURED", label: "COUNCIL_CONFIGURED" },
  { value: "COUNCIL_DELIBERATED", label: "COUNCIL_DELIBERATED" },
  { value: "ROUTE_DECIDED", label: "ROUTE_DECIDED" },
  { value: "PROJECT_CREATED", label: "PROJECT_CREATED" },
  { value: "MEMORY_SAVED", label: "MEMORY_SAVED" },
  { value: "MEMORY_REMOVED", label: "MEMORY_REMOVED" },
  { value: "DECISION_SAVED", label: "DECISION_SAVED" },
  { value: "CONTEXT_BUILT", label: "CONTEXT_BUILT" },
  { value: "HANDOFF_CREATED", label: "HANDOFF_CREATED" },
  { value: "HANDOFF_ACCEPTED", label: "HANDOFF_ACCEPTED" },
  { value: "TASK_CREATED", label: "TASK_CREATED" },
  { value: "TASK_STARTED", label: "TASK_STARTED" },
  { value: "TASK_COMPLETED", label: "TASK_COMPLETED" },
  { value: "TASK_UPDATED", label: "TASK_UPDATED" },
  { value: "AGENT_STARTED", label: "AGENT_STARTED" },
  { value: "AGENT_FINISHED", label: "AGENT_FINISHED" },
  { value: "AUTONOMY_CHANGED", label: "AUTONOMY_CHANGED" },
  { value: "APPROVAL_REQUESTED", label: "APPROVAL_REQUESTED" },
  { value: "APPROVAL_DECIDED", label: "APPROVAL_DECIDED" },
  { value: "EXECUTION_PAUSED", label: "EXECUTION_PAUSED" },
  { value: "EXECUTION_RESUMED", label: "EXECUTION_RESUMED" },
  { value: "GITHUB_PR_CREATED", label: "GITHUB_PR_CREATED" },
  { value: "GITHUB_PR_MERGED", label: "GITHUB_PR_MERGED" },
  { value: "GITHUB_ISSUE_CREATED", label: "GITHUB_ISSUE_CREATED" },
  { value: "CONTEXT_COMPACTED", label: "CONTEXT_COMPACTED" },
  { value: "APP_UPDATED", label: "APP_UPDATED" },
  { value: "PROJECT_LINKED", label: "PROJECT_LINKED" },
  { value: "PROJECT_ASKED", label: "PROJECT_ASKED" },
];

const PAGE = 500;

interface Props {
  ready: boolean;
  /** Database file, shown in the footer. */
  database: string | null;
  /** Open project (for "Só este projeto"). */
  projectId: string | null;
}

export function HistoryPanel({ ready, database, projectId }: Props) {
  const [events, setEvents] = useState<AuditEvent[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [kind, setKind] = useState<EventKind | "">("");
  const [text, setText] = useState("");
  const [hideReads, setHideReads] = useState(true);
  const [onlyProject, setOnlyProject] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const scope = onlyProject ? projectId : null;
  const reload = useRef<ReturnType<typeof setTimeout> | null>(null);

  const firstPage = useCallback(async () => {
    try {
      const page = await memoryApi.history({ limit: PAGE, projectId: scope });
      setEvents(page.events);
      setNext(page.next);
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [scope]);

  useEffect(() => {
    if (!ready) return;
    let buffered: AuditEvent[] = [];
    let loaded = false;
    const unsubscribe = auditEvents.subscribe((event) => {
      if (scope) {
        // Live events carry no project: re-read the scoped page instead.
        if (reload.current) clearTimeout(reload.current);
        reload.current = setTimeout(() => void firstPage(), 300);
        return;
      }
      if (!loaded) {
        buffered.push(event);
        return;
      }
      setEvents((all) => [...all.slice(-(MAX_EVENTS - 1)), event]);
    });
    void firstPage().then(() => {
      setEvents((recent) => {
        const known = new Set(recent.map((e) => e.id));
        return [...recent, ...buffered.filter((e) => !known.has(e.id))].slice(-MAX_EVENTS);
      });
      buffered = [];
      loaded = true;
    });
    return () => {
      unsubscribe();
      if (reload.current) clearTimeout(reload.current);
    };
  }, [ready, scope, firstPage]);

  const loadOlder = async () => {
    if (!next) return;
    setLoadingMore(true);
    try {
      const page = await memoryApi.history({ limit: PAGE, projectId: scope, before: next });
      setEvents((all) => [...page.events, ...all]);
      setNext(page.next);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoadingMore(false);
    }
  };

  const visible = useMemo(() => {
    const needle = text.trim().toLowerCase();
    return events
      .filter((e) => (kind ? e.kind === kind : true))
      .filter((e) => (hideReads ? !isReadOnlyCall(e) : true))
      .filter((e) => (needle ? e.summary.toLowerCase().includes(needle) || e.kind.toLowerCase().includes(needle) : true))
      .reverse();
  }, [events, kind, text, hideReads]);

  return (
    <div className="panel">
      <div className="panel-header">
        <span>History</span>
        <span className="meta">{events.length} eventos</span>
      </div>
      <div className="stack pad">
        <input placeholder="Filtrar…" value={text} onChange={(e) => setText(e.target.value)} />
        <div className="row">
          <select value={kind} onChange={(e) => setKind(e.target.value as EventKind | "")}>
            {KIND_FILTERS.map((k) => (
              <option key={k.value} value={k.value}>
                {k.label}
              </option>
            ))}
          </select>
        </div>
        <label className="check" title="Oculta consultas bem-sucedidas (ferramentas readOnly); continuam registradas">
          <input type="checkbox" checked={hideReads} onChange={(e) => setHideReads(e.target.checked)} />
          Ocultar consultas
        </label>
        <label className="check" title={projectId ? "Eventos do projeto aberto" : "Abra um projeto"}>
          <input
            type="checkbox"
            checked={onlyProject}
            disabled={!projectId}
            onChange={(e) => setOnlyProject(e.target.checked)}
          />
          Só este projeto
        </label>
      </div>
      {error && <div className="inline-error">{error}</div>}
      <ul className="events">
        {visible.length === 0 && <li className="empty">Nenhum evento.</li>}
        {visible.map((event) => {
          const failed = event.kind === "TOOL_CALLED" && event.data.ok === false;
          const duration = typeof event.data.durationMs === "number" ? event.data.durationMs : null;
          return (
            <li
              key={event.id}
              className={`event ${failed ? "failed" : ""}`}
              onClick={() => setExpanded((id) => (id === event.id ? null : event.id))}
            >
              <div className="event-head">
                <span className={`kind kind-${event.kind.toLowerCase()}`}>{event.kind}</span>
                <span className="meta">{formatTime(event.at)}</span>
              </div>
              <div className="event-summary mono">{event.summary}</div>
              <div className="meta">
                {originLabel(event)}
                {duration !== null && ` · ${formatDuration(duration)}`}
                {failed && " · falhou"}
              </div>
              {expanded === event.id && <pre className="event-data">{JSON.stringify(event, null, 2)}</pre>}
            </li>
          );
        })}
        {next && (
          <li className="load-more">
            <button className="button small" disabled={loadingMore} onClick={() => void loadOlder()}>
              {loadingMore ? "Carregando…" : "Carregar mais antigos"}
            </button>
          </li>
        )}
      </ul>
      {database && (
        <div className="panel-footer meta mono ellipsis" title={database}>
          banco: {database}
        </div>
      )}
    </div>
  );
}
