// HISTORY panel: audit events (runtime://audit + history_recent).

import { useEffect, useMemo, useState } from "react";
import { auditEvents } from "../lib/events";
import { formatDuration, formatTime } from "../lib/format";
import { appApi, errorMessage } from "../lib/runtime";
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
];

interface Props {
  ready: boolean;
  auditLog: string | null;
}

export function HistoryPanel({ ready, auditLog }: Props) {
  const [events, setEvents] = useState<AuditEvent[]>([]);
  const [kind, setKind] = useState<EventKind | "">("");
  const [text, setText] = useState("");
  const [hideReads, setHideReads] = useState(true);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!ready) return;
    let buffered: AuditEvent[] = [];
    let loaded = false;
    const unsubscribe = auditEvents.subscribe((event) => {
      if (!loaded) {
        buffered.push(event);
        return;
      }
      setEvents((all) => [...all.slice(-(MAX_EVENTS - 1)), event]);
    });
    appApi
      .history(1000)
      .then((recent) => {
        const known = new Set(recent.map((e) => e.id));
        setEvents([...recent, ...buffered.filter((e) => !known.has(e.id))].slice(-MAX_EVENTS));
        buffered = [];
        loaded = true;
      })
      .catch((e) => setError(errorMessage(e)));
    return unsubscribe;
  }, [ready]);

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
      </ul>
      {auditLog && (
        <div className="panel-footer meta mono" title={auditLog}>
          log: {auditLog}
        </div>
      )}
    </div>
  );
}
