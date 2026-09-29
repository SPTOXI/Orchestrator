// Main area: one provider session — transcript (live), input, cancel,
// close/resume and subagents (ADR-0009).

import { type FormEvent, type KeyboardEvent, useEffect, useRef, useState } from "react";
import { formatDuration, formatTime } from "../lib/format";
import { errorMessage, sessionApi } from "../lib/runtime";
import { formatUsage, type TranscriptItem } from "../lib/transcript";
import { useSessionTranscript } from "../lib/useProviders";
import type { ProviderInfo, SessionInfo } from "../lib/types";
import { SendIcon, StopIcon, SubagentIcon } from "./icons";
import { statusDot, statusLabel } from "./ProvidersPanel";

const PREVIEW_CHARS = 4000;

interface Props {
  ready: boolean;
  active: boolean;
  sessionId: string;
  /** Latest info from the sessions list (null until listed). */
  session: SessionInfo | null;
  parent: SessionInfo | null;
  providers: ProviderInfo[];
  onOpenSession: (id: string) => void;
}

function compactJson(value: unknown, max = 160): string {
  const text = JSON.stringify(value ?? {});
  return text.length > max ? `${text.slice(0, max)}…` : text;
}

function prettyJson(value: unknown): string {
  const text = JSON.stringify(value, null, 2) ?? "null";
  return text.length > PREVIEW_CHARS ? `${text.slice(0, PREVIEW_CHARS)}\n… (truncado)` : text;
}

function Item({
  item,
  providerName,
  onOpenSession,
}: {
  item: TranscriptItem;
  providerName: string;
  onOpenSession: (id: string) => void;
}) {
  switch (item.kind) {
    case "user":
      return (
        <div className="msg user">
          <div className="msg-author">
            Você <span className="meta">{formatTime(item.at)}</span>
          </div>
          <div className="msg-text">{item.text}</div>
        </div>
      );
    case "assistant":
      return (
        <div className="msg assistant">
          <div className="msg-author">{providerName}</div>
          <div className="msg-text">{item.text}</div>
        </div>
      );
    case "reasoning":
      return (
        <details className="msg reasoning">
          <summary>Raciocínio</summary>
          <div className="msg-text">{item.text}</div>
        </details>
      );
    case "tool": {
      const result = item.result;
      const state = !result ? "pending" : result.ok ? "ok" : "err";
      return (
        <div className={`tool-call ${state}`}>
          <div className="row">
            <span className={`dot ${state === "pending" ? "ok pulse" : state === "ok" ? "ok" : "err"}`} />
            <span className="mono tool-name">{item.tool}</span>
            <span className="mono meta grow ellipsis" title={compactJson(item.args, 2000)}>
              {item.args === null ? "" : compactJson(item.args)}
            </span>
            <span className="meta">
              {!result ? "executando…" : `${result.ok ? "ok" : "falhou"} · ${formatDuration(result.durationMs)}`}
            </span>
          </div>
          {result?.error && (
            <div className="tool-error mono">
              {result.error.kind}: {result.error.message}
            </div>
          )}
          {result?.ok && (
            <details>
              <summary className="meta">saída</summary>
              <pre className="output">{prettyJson(result.output)}</pre>
            </details>
          )}
        </div>
      );
    }
    case "notice":
      return <div className={item.level === "error" ? "inline-error" : "inline-notice"}>{item.message}</div>;
    case "turnEnd": {
      const label =
        item.status === "completed" ? "✓ concluído" : item.status === "cancelled" ? "■ cancelado" : "✗ falhou";
      return (
        <div className={`turn-end ${item.status}`}>
          <span>{label}</span>
          <span className="meta">
            {formatDuration(item.durationMs)} · {formatUsage(item.usage)}
            {item.toolCalls > 0 && ` · ${item.toolCalls} ${item.toolCalls === 1 ? "ferramenta" : "ferramentas"}`}
          </span>
          {item.error && <div className="tool-error">{item.error}</div>}
        </div>
      );
    }
    case "subagent":
      return (
        <button className="subagent-link" onClick={() => onOpenSession(item.childId)}>
          <SubagentIcon /> Subagente criado: <strong>{item.title}</strong> <span className="meta">({item.provider})</span>
        </button>
      );
  }
}

export function SessionView({ ready, active, sessionId, session, parent, providers, onOpenSession }: Props) {
  const { transcript, error: syncError } = useSessionTranscript(sessionId, ready);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [spawnOpen, setSpawnOpen] = useState(false);
  const [spawnProvider, setSpawnProvider] = useState("");
  const [spawnTitle, setSpawnTitle] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  /** Follow new output while the user is at the bottom. */
  const stickRef = useRef(true);
  /** Scroll events caused by our own scrolling (they arrive late, after more
   * output may have been appended) must not unstick the view. */
  const autoScrollRef = useRef(false);

  const status = session?.status ?? transcript.status ?? "idle";
  const running = status === "running";
  const closed = status === "closed";
  const provider = providers.find((p) => p.id === session?.provider);
  const providerName = provider?.name ?? session?.provider ?? "Provider";

  useEffect(() => {
    const el = scrollRef.current;
    if (el && stickRef.current && el.scrollTop < el.scrollHeight - el.clientHeight) {
      autoScrollRef.current = true;
      el.scrollTop = el.scrollHeight;
    }
  }, [transcript.lastSeq, active]);

  const run = async (action: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const send = (e?: FormEvent) => {
    e?.preventDefault();
    const text = input.trim();
    if (!text || running || closed) return;
    stickRef.current = true;
    void run(async () => {
      await sessionApi.send(sessionId, text);
      setInput("");
    });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      send();
    }
  };

  const spawn = (e: FormEvent) => {
    e.preventDefault();
    void run(async () => {
      const child = await sessionApi.spawn(sessionId, {
        provider: spawnProvider || undefined,
        title: spawnTitle.trim() || undefined,
      });
      setSpawnOpen(false);
      setSpawnTitle("");
      onOpenSession(child.id);
    });
  };

  return (
    <div className="editor session-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className={`dot ${statusDot(status)}`} title={statusLabel(status)} />
        <span className="profile-title ellipsis">{session?.title ?? "Sessão"}</span>
        <span className="badge">{providerName}</span>
        {session?.model && <span className="meta mono">{session.model}</span>}
        <span className="meta grow ellipsis" title={session?.nativeRef ?? undefined}>
          {session ? `${statusLabel(status)} · ${session.turns} ${session.turns === 1 ? "turno" : "turnos"}` : ""}
          {session && session.turns > 0 && ` · ${formatUsage(session.usage)}`}
        </span>
        <button
          className="button small"
          disabled={!ready || busy || closed}
          title="Criar subagente (spawnAgent)"
          onClick={() => setSpawnOpen((open) => !open)}
        >
          <SubagentIcon /> Subagente
        </button>
        {running && (
          <button
            className="button small danger"
            disabled={busy}
            onClick={() => void run(() => sessionApi.cancel(sessionId))}
          >
            <StopIcon /> Cancelar
          </button>
        )}
        {closed ? (
          <button
            className="button small"
            disabled={!ready || busy}
            onClick={() => void run(() => sessionApi.resume(sessionId))}
          >
            Retomar
          </button>
        ) : (
          <button
            className="button small"
            disabled={!ready || busy}
            title={running ? "Cancela o turno em andamento e encerra" : "Encerrar sessão"}
            onClick={() => void run(() => sessionApi.close(sessionId))}
          >
            Encerrar
          </button>
        )}
      </div>
      {parent && (
        <div className="session-parent meta">
          Subagente de{" "}
          <button className="link" onClick={() => onOpenSession(parent.id)}>
            {parent.title}
          </button>
        </div>
      )}
      {spawnOpen && (
        <form className="spawn-form row" onSubmit={spawn}>
          <select value={spawnProvider} onChange={(e) => setSpawnProvider(e.target.value)}>
            <option value="">{providerName} (mesmo provider)</option>
            {providers
              .filter((p) => p.id !== session?.provider)
              .map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
          </select>
          <input
            className="grow"
            placeholder="Título do subagente (opcional)"
            value={spawnTitle}
            onChange={(e) => setSpawnTitle(e.target.value)}
          />
          <button className="button small primary" type="submit" disabled={busy}>
            Criar
          </button>
          <button className="button small" type="button" onClick={() => setSpawnOpen(false)}>
            Cancelar
          </button>
        </form>
      )}
      {(error || syncError) && <div className="inline-error">{error ?? syncError}</div>}
      <div
        className="transcript"
        ref={scrollRef}
        onScroll={(e) => {
          if (autoScrollRef.current) {
            autoScrollRef.current = false;
            return;
          }
          const el = e.currentTarget;
          stickRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        {transcript.items.length === 0 && (
          <div className="empty">
            Sessão pronta. {provider?.id === "echo" && "Digite /help para ver os comandos do provider Echo."}
          </div>
        )}
        {transcript.items.map((item) => (
          <Item key={item.key} item={item} providerName={providerName} onOpenSession={onOpenSession} />
        ))}
        {running && <div className="meta typing">{providerName} está trabalhando…</div>}
      </div>
      <form className="composer" onSubmit={send}>
        <textarea
          rows={3}
          value={input}
          disabled={!ready || closed}
          placeholder={
            closed
              ? "Sessão encerrada — use Retomar para continuar."
              : `Mensagem para ${providerName} (Enter envia, Shift+Enter quebra linha)`
          }
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={onKeyDown}
        />
        <button
          className="button primary"
          type="submit"
          disabled={!ready || busy || running || closed || !input.trim()}
        >
          <SendIcon /> Enviar
        </button>
      </form>
    </div>
  );
}
