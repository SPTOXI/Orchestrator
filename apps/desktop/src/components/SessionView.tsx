// Main area: one provider session — transcript (live), input, cancel,
// close/resume and subagents (ADR-0009); the project context of the first
// message and the handoff to another AI (ADR-0013).

import { type FormEvent, type KeyboardEvent, useEffect, useRef, useState } from "react";
import type { SessionActivity } from "../lib/activity";
import { HANDOFF_PROMPT_PREFIX, SECTION_LABELS, summaryLine } from "../lib/context";
import { formatDuration, formatTime } from "../lib/format";
import { contextApi, errorMessage, sessionApi } from "../lib/runtime";
import { formatUsage, type TranscriptItem } from "../lib/transcript";
import { useSessionTranscript } from "../lib/useProviders";
import type { ContextOptions, ContextPack, ProviderInfo, SessionInfo } from "../lib/types";
import { askingProject } from "../lib/workspace";
import { ActivityLine } from "./Activity";
import type { ContextTabRequest } from "./ContextView";
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
  /** Title of another session (handoff links). */
  sessionTitle: (id: string) => string | null;
  onOpenSession: (id: string) => void;
  onOpenContext: (request: ContextTabRequest) => void;
  onOpenHandoff: (sessionId: string) => void;
  /** Tool calls waiting for the user's authorization (ADR-0016). */
  waitingCalls?: ReadonlySet<string>;
  /** What the AI is doing in the turn in progress, if any. */
  activity?: SessionActivity | null;
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
  sessionId,
  providerName,
  sessionTitle,
  onOpenSession,
  waiting,
  onRetry,
}: {
  item: TranscriptItem;
  sessionId: string;
  providerName: string;
  sessionTitle: (id: string) => string | null;
  onOpenSession: (id: string) => void;
  /** The call waits for the user's authorization. */
  waiting: boolean;
  /** Sends the failed turn's message again (only on the last turn). */
  onRetry?: () => void;
}) {
  switch (item.kind) {
    case "user":
      // The Orchestrator asked the AI for the narrative of a handoff.
      if (item.text.startsWith(HANDOFF_PROMPT_PREFIX)) {
        return (
          <div className="msg user orchestrator">
            <div className="msg-author">
              Orchestrator <span className="meta">pedido do resumo do handoff · {formatTime(item.at)}</span>
            </div>
            <details>
              <summary className="meta">instruções enviadas à IA</summary>
              <div className="msg-text">{item.text}</div>
            </details>
          </div>
        );
      }
      // The AI of a related project asks (ADR-0023).
      if (askingProject(item.text)) {
        return (
          <div className="msg user from-project">
            <div className="msg-author">
              IA do projeto {askingProject(item.text)} <span className="meta">pergunta · {formatTime(item.at)}</span>
            </div>
            <div className="msg-text">{item.text}</div>
          </div>
        );
      }
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
      // A question answered by a related project's AI (ADR-0023).
      const answered =
        result?.ok && item.tool === "projects.ask"
          ? (result.output as { project?: string; sessionId?: string } | null)
          : null;
      return (
        <div className={`tool-call ${state}`}>
          <div className="row">
            <span className={`dot ${state === "pending" ? "ok pulse" : state === "ok" ? "ok" : "err"}`} />
            <span className="mono tool-name">{item.tool}</span>
            <span className="mono meta grow ellipsis" title={compactJson(item.args, 2000)}>
              {item.args === null ? "" : compactJson(item.args)}
            </span>
            <span className="meta">
              {!result
                ? waiting
                  ? "esperando sua autorização"
                  : "executando…"
                : `${result.ok ? "ok" : "falhou"} · ${formatDuration(result.durationMs)}`}
            </span>
          </div>
          {result?.error && (
            <div className="tool-error mono">
              {result.error.kind}: {result.error.message}
            </div>
          )}
          {answered?.sessionId && (
            <div className="row tight project-answer">
              <span className="meta grow">respondida pela IA do projeto {answered.project}</span>
              <button className="link" onClick={() => onOpenSession(answered.sessionId as string)}>
                Abrir a conversa
              </button>
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
          {onRetry && (
            <button className="button small" onClick={onRetry}>
              Tentar de novo
            </button>
          )}
        </div>
      );
    }
    case "subagent":
      return (
        <button className="subagent-link" onClick={() => onOpenSession(item.childId)}>
          <SubagentIcon /> Subagente criado: <strong>{item.title}</strong> <span className="meta">({item.provider})</span>
        </button>
      );
    case "context":
      return (
        <details className="context-item">
          <summary>
            Contexto do projeto anexado · <span className="meta">{summaryLine(item.summary)}</span>
            {item.summary.handoffId && <span className="badge">handoff</span>}
          </summary>
          <ul className="plain-list">
            {item.summary.sections.map((section) => (
              <li key={section.kind}>
                <span className="grow">{SECTION_LABELS[section.kind] ?? section.title}</span>
                <span className="meta">
                  {section.items} {section.items === 1 ? "item" : "itens"} · ~{section.tokens} tokens
                </span>
              </li>
            ))}
          </ul>
          {item.summary.omitted.map((o) => (
            <div key={o} className="meta">
              Fora pelo orçamento: {o.replace(/ \(orçamento\)$/, "")}
            </div>
          ))}
        </details>
      );
    case "compacted":
      return (
        <details className="context-item compacted-item">
          <summary>
            Conversa compactada {item.automatic ? "automaticamente" : "a pedido"} ·{" "}
            <span className="meta">
              {item.messages} {item.messages === 1 ? "mensagem" : "mensagens"} · ~
              {item.beforeTokens.toLocaleString("pt-BR")} → ~{item.afterTokens.toLocaleString("pt-BR")} tokens
            </span>
          </summary>
          <div className="meta">
            Daqui em diante, a IA recebe este resumo no lugar da conversa anterior (que continua aqui na tela).
          </div>
          <pre className="output compacted-summary">{item.summary}</pre>
        </details>
      );
    case "handoff": {
      const incoming = item.toSession === sessionId;
      const other = incoming ? item.fromSession : item.toSession;
      const title = sessionTitle(other) ?? "outra sessão";
      return (
        <button className="subagent-link handoff-link" onClick={() => onOpenSession(other)}>
          {incoming ? "Assumiu o trabalho de " : "Trabalho passado para "}
          <strong>{title}</strong>
          {!incoming && <span className="meta"> ({item.provider})</span>}
          <span className="meta"> · handoff</span>
        </button>
      );
    }
  }
}

/** Before the first message: whether the project context goes with it,
 * and how big it would be for what is being typed. */
function ProjectContextBar({
  ready,
  session,
  draft,
  onOpenContext,
}: {
  ready: boolean;
  session: SessionInfo;
  draft: string;
  onOpenContext: (request: ContextTabRequest) => void;
}) {
  const [options, setOptions] = useState<ContextOptions | null>(null);
  const [auto, setAuto] = useState(true);
  const [pack, setPack] = useState<ContextPack | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!ready) return;
    Promise.all([sessionApi.context(session.id), contextApi.settings()])
      .then(([o, view]) => {
        setOptions(o);
        setAuto(view.settings.autoAttach);
      })
      .catch((e) => setError(errorMessage(e)));
  }, [ready, session.id]);

  const handoff = options?.handoffId ?? null;
  const enabled = handoff ? true : (options?.enabled ?? auto);

  useEffect(() => {
    if (!ready || !options || !enabled) {
      setPack(null);
      return;
    }
    const timer = setTimeout(() => {
      contextApi
        .preview({
          sessionId: session.id,
          projectPath: session.projectPath,
          task: draft,
          handoffId: handoff ?? undefined,
          budget: options.budget ?? undefined,
        })
        .then((p) => {
          setPack(p);
          setError(null);
        })
        .catch((e) => setError(errorMessage(e)));
    }, 500);
    return () => clearTimeout(timer);
  }, [ready, options, enabled, draft, handoff, session.id, session.projectPath]);

  const toggle = () => {
    if (!options || handoff) return;
    sessionApi
      .setContext(session.id, { ...options, enabled: !enabled })
      .then(setOptions)
      .catch((e) => setError(errorMessage(e)));
  };

  return (
    <div className="context-bar row">
      <label className="check" title="O contexto vai uma vez, com a primeira mensagem">
        <input type="checkbox" checked={enabled} disabled={!options || handoff !== null} onChange={toggle} />
        {handoff ? "Contexto do projeto com o HANDOFF" : "Anexar o contexto do projeto à primeira mensagem"}
      </label>
      <span className="meta grow ellipsis">
        {error ?? (!enabled ? "a IA recebe só a mensagem" : pack ? summaryLine(pack) : "calculando…")}
      </span>
      <button
        className="link"
        disabled={!enabled}
        onClick={() =>
          onOpenContext({
            sessionId: session.id,
            task: draft,
            projectPath: session.projectPath,
            handoffId: handoff ?? undefined,
          })
        }
      >
        Ver prévia
      </button>
    </div>
  );
}

export function SessionView({
  ready,
  active,
  sessionId,
  session,
  parent,
  providers,
  sessionTitle,
  onOpenSession,
  onOpenContext,
  onOpenHandoff,
  waitingCalls,
  activity = null,
}: Props) {
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
  /** Its provider summarizes its own conversation (ADR-0018). */
  const compacts = provider?.capabilities.compaction === true;
  const [compacting, setCompacting] = useState(false);
  /** Its connection was removed or disabled: turns would fail. */
  const orphan = session !== null && providers.length > 0 && !provider;

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

  /** The last turn failed: its message, to send again. */
  const lastEnd = [...transcript.items].reverse().find((i) => i.kind === "turnEnd");
  const retryText =
    lastEnd?.kind === "turnEnd" && lastEnd.status === "failed"
      ? transcript.items.find((i) => i.kind === "user" && i.turnId === lastEnd.turnId)
      : undefined;
  const retry =
    retryText?.kind === "user" && !running && !closed && !busy
      ? () => {
          stickRef.current = true;
          void run(() => sessionApi.send(sessionId, retryText.text));
        }
      : undefined;

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
        <button
          className="button small"
          disabled={!ready || running || !session || session.turns === 0}
          title="Passar este trabalho para outra IA, sem a conversa (HandoffPacket)"
          onClick={() => onOpenHandoff(sessionId)}
        >
          Handoff
        </button>
        {compacts && (
          <button
            className="button small"
            disabled={!ready || busy || compacting || running || closed || !session || session.turns === 0}
            title="A IA resume a conversa e o resumo passa a ir no lugar dela: menos tokens por mensagem (ADR-0018)"
            onClick={() => {
              // Not through `run`: Cancelar stays available while the AI
              // writes the summary.
              setCompacting(true);
              setError(null);
              stickRef.current = true;
              sessionApi
                .compact(sessionId)
                .catch((e: unknown) => setError(errorMessage(e)))
                .finally(() => setCompacting(false));
            }}
          >
            {compacting ? "Compactando…" : "Compactar"}
          </button>
        )}
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
          <Item
            key={item.key}
            item={item}
            sessionId={sessionId}
            providerName={providerName}
            sessionTitle={sessionTitle}
            onOpenSession={onOpenSession}
            waiting={item.kind === "tool" && (waitingCalls?.has(item.callId) ?? false)}
            onRetry={item === lastEnd ? retry : undefined}
          />
        ))}
        {running && !activity && <div className="meta typing">{providerName} está trabalhando…</div>}
      </div>
      {running && activity && (
        <ActivityLine
          activity={activity}
          providerName={providerName}
          waitingForUser={activity.callId !== null && (waitingCalls?.has(activity.callId) ?? false)}
        />
      )}
      {orphan && (
        <div className="inline-notice">
          O provider <code>{session?.provider}</code> não está mais registrado (conexão removida, renomeada ou
          desativada). Reative a conexão ou abra uma nova sessão.
        </div>
      )}
      {session && session.turns === 0 && !running && !closed && !transcript.items.some((i) => i.kind === "user") && (
        <ProjectContextBar ready={ready} session={session} draft={input} onOpenContext={onOpenContext} />
      )}
      <form className="composer" onSubmit={send}>
        <textarea
          rows={3}
          value={input}
          disabled={!ready || closed || orphan}
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
          disabled={!ready || busy || running || closed || orphan || !input.trim()}
        >
          <SendIcon /> Enviar
        </button>
      </form>
    </div>
  );
}
