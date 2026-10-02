// AI PROVIDERS panel: the user's API connections as registered providers
// (ADR-0009, ADR-0010), their availability and capabilities, the active
// provider and the sessions of the open project.

import { useEffect, useState } from "react";
import { activityLabel, type SessionActivity } from "../lib/activity";
import { formatPrices } from "../lib/connections";
import { memberLabel, MODE_LABELS } from "../lib/council";
import { baseName } from "../lib/format";
import { formatTokens } from "../lib/transcript";
import type { Connections } from "../lib/useConnections";
import type { Council } from "../lib/useCouncil";
import type { ProviderHealth, Providers } from "../lib/useProviders";
import type { ConnectionView, ProviderCapabilities, ProviderInfo, SessionInfo, SessionStatus } from "../lib/types";
import { usePersistentToggle } from "../lib/usePersistentToggle";
import { Elapsed } from "./Activity";
import { Collapsible } from "./Collapsible";
import { ChevronIcon, EditIcon, PlusIcon, RefreshIcon } from "./icons";

interface Props {
  ready: boolean;
  providers: Providers;
  connections: Connections;
  /** Open project; sessions are listed per project. */
  projectPath: string | null;
  activeSessionId: string | null;
  starting: boolean;
  onNewSession: (provider: string, model: string | null) => void;
  onOpenSession: (id: string) => void;
  /** Opens the connection editor; null = new connection. */
  onEditConnection: (id: string | null) => void;
  council: Council;
  onOpenCouncil: () => void;
  /** Opens "Nova sessão com o Conselho". */
  onOpenRoute: () => void;
  /** Turns in progress, by session (lib/activity.ts). */
  activity?: Record<string, SessionActivity>;
  /** Tool calls waiting for the user's authorization. */
  waitingCalls?: ReadonlySet<string>;
}

const CAPABILITY_LABELS: Array<[keyof ProviderCapabilities, string]> = [
  ["streaming", "streaming"],
  ["toolCalls", "ferramentas"],
  ["resume", "retomada"],
  ["cancel", "cancelamento"],
  ["nativeSubagents", "subagentes nativos"],
  ["reasoning", "raciocínio"],
  ["tokenUsage", "uso de tokens"],
  ["cost", "custo"],
  ["completion", "conselho"],
];

export function statusDot(status: SessionStatus): string {
  return status === "running" ? "ok pulse" : status === "idle" ? "ok" : "off";
}

export function statusLabel(status: SessionStatus): string {
  return status === "running" ? "em execução" : status === "idle" ? "pronta" : "encerrada";
}

const TOOL_MODES = { native: "nativas", prompt: "por prompt", none: "desligadas" } as const;

function HealthLine({ health }: { health: ProviderHealth | undefined }) {
  if (!health || "checking" in health) return <div className="meta">verificando…</div>;
  if ("error" in health) return <div className="meta err-text">{health.error}</div>;
  const { status } = health;
  const parts = [
    status.available ? "disponível" : "indisponível",
    status.version ? `v${status.version}` : null,
    status.authenticated === true ? "autenticado" : status.authenticated === false ? "sem autenticação" : null,
  ].filter(Boolean);
  return (
    <div className={`meta ${status.available ? "" : "err-text"}`} title={status.detail ?? undefined}>
      {parts.join(" · ")}
    </div>
  );
}

function ProviderCard({
  provider,
  health,
  connection,
  onSelect,
  onInspect,
  onEdit,
}: {
  provider: ProviderInfo;
  health: ProviderHealth | undefined;
  connection: ConnectionView | undefined;
  onSelect: () => void;
  onInspect: () => void;
  onEdit: (() => void) | null;
}) {
  const available = health && "status" in health ? health.status.available : null;
  const caps = provider.capabilities;
  const missingKey = connection && connection.key.source !== "none" && !connection.key.present;
  const [open, setOpen] = usePersistentToggle(`provider.${provider.id}`, false);
  return (
    <li className={`provider-card ${provider.active ? "active" : ""}`}>
      <div className="row">
        <button
          className="card-toggle"
          title={open ? "Recolher detalhes" : "Mostrar detalhes"}
          aria-expanded={open}
          onClick={() => setOpen(!open)}
        >
          <ChevronIcon open={open} />
        </button>
        <span className={`dot ${available === null ? "" : available ? "ok" : "err"}`} />
        <strong className="grow ellipsis" title={provider.description}>
          {provider.name}
        </strong>
        {provider.active ? (
          <span className="badge ok">ativo</span>
        ) : (
          <button className="button small" onClick={onSelect}>
            Usar
          </button>
        )}
        <button className="icon-button small" title="Verificar disponibilidade" onClick={onInspect}>
          <RefreshIcon />
        </button>
        {onEdit && (
          <button className="icon-button small" title="Editar conexão" onClick={onEdit}>
            <EditIcon />
          </button>
        )}
      </div>
      {!open && caps.defaultModel && <div className="meta mono ellipsis">{caps.defaultModel}</div>}
      {open && (
        <>
          <div className="meta mono ellipsis" title={connection?.connection.baseUrl}>
            {provider.id} · {provider.vendor}
            {caps.defaultModel ? ` · ${caps.defaultModel}` : ""}
          </div>
          <div className="meta">
            {caps.models.length} {caps.models.length === 1 ? "modelo" : "modelos"}
            {connection && ` · ferramentas ${TOOL_MODES[connection.connection.toolMode ?? (connection.connection.kind === "generic" ? "prompt" : "native")]}`}
          </div>
        </>
      )}
      {missingKey && (
        <div className="meta err-text">{connection.key.detail ?? "sem chave de API — edite a conexão"}</div>
      )}
      {(open || available === false) && <HealthLine health={health} />}
      {open && (
        <div className="chip-list">
          {CAPABILITY_LABELS.filter(([key]) => caps[key] === true).map(([key, label]) => (
            <span key={key} className="tag small">
              {label}
            </span>
          ))}
        </div>
      )}
    </li>
  );
}

/** Provider + model chooser for a new session. */
function NewSession({
  providers,
  disabled,
  title,
  starting,
  onStart,
  isConnection,
  onConfigure,
}: {
  providers: ProviderInfo[];
  disabled: boolean;
  title: string;
  starting: boolean;
  onStart: (provider: string, model: string | null) => void;
  /** Whether a provider is one of the user's API connections. */
  isConnection: (id: string) => boolean;
  /** Opens a connection's settings. */
  onConfigure: (id: string) => void;
}) {
  const active = providers.find((p) => p.active) ?? providers[0] ?? null;
  const [providerId, setProviderId] = useState<string | null>(null);
  const provider = providers.find((p) => p.id === providerId) ?? active;
  const [model, setModel] = useState<string | null>(null);
  const models = provider?.capabilities.models ?? [];
  const chosen = models.some((m) => m.id === model) ? model : (provider?.capabilities.defaultModel ?? models[0]?.id ?? null);
  /** An API connection with no model on: a session could not start. */
  const noModel = provider !== null && isConnection(provider.id) && chosen === null;

  // Follow the active provider until the user picks one here.
  useEffect(() => {
    if (providerId && !providers.some((p) => p.id === providerId)) setProviderId(null);
  }, [providers, providerId]);

  return (
    <>
      <div className="new-session">
        <select
          value={provider?.id ?? ""}
          disabled={providers.length === 0}
          onChange={(e) => {
            setProviderId(e.target.value);
            setModel(null);
          }}
          title="Provider da sessão"
        >
          {providers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        <select
          value={chosen ?? ""}
          disabled={models.length === 0}
          onChange={(e) => setModel(e.target.value)}
          title={(() => {
            const info = models.find((m) => m.id === chosen);
            return info ? [info.name, formatPrices(info), info.tags.join(", ")].filter(Boolean).join(" · ") : "Modelo";
          })()}
        >
          {models.length === 0 && <option value="">padrão</option>}
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.id}
            </option>
          ))}
        </select>
        <button
          className="button small primary"
          disabled={disabled || !provider || starting || noModel}
          title={noModel ? "Esta conexão não tem nenhum modelo ligado" : title}
          onClick={() => provider && onStart(provider.id, chosen)}
        >
          <PlusIcon /> {starting ? "Iniciando…" : "Nova sessão"}
        </button>
      </div>
      {noModel && provider && (
        <div className="inline-notice pad-x">
          <strong>{provider.name}</strong> não tem nenhum modelo ligado. Abra a conexão, clique em "Buscar modelos" e
          ligue pelo menos um.{" "}
          <button className="link" onClick={() => onConfigure(provider.id)}>
            Abrir a conexão
          </button>
        </div>
      )}
    </>
  );
}

/** Sessions ordered as a tree: roots newest first, subagents under their parent. */
function sessionTree(sessions: SessionInfo[]): Array<{ session: SessionInfo; depth: number }> {
  const ids = new Set(sessions.map((s) => s.id));
  const children = new Map<string, SessionInfo[]>();
  for (const s of sessions) {
    if (s.parentId && ids.has(s.parentId)) {
      children.set(s.parentId, [...(children.get(s.parentId) ?? []), s]);
    }
  }
  const out: Array<{ session: SessionInfo; depth: number }> = [];
  const visit = (session: SessionInfo, depth: number) => {
    out.push({ session, depth });
    // Subagents in creation order (the list is newest first).
    for (const child of [...(children.get(session.id) ?? [])].reverse()) visit(child, depth + 1);
  };
  for (const s of sessions) if (!s.parentId || !ids.has(s.parentId)) visit(s, 0);
  return out;
}

/** What a session's AI is doing now, with its clock. */
function Working({ activity, waitingCalls }: { activity: SessionActivity; waitingCalls?: ReadonlySet<string> }) {
  const waiting = activity.callId !== null && (waitingCalls?.has(activity.callId) ?? false);
  return (
    <div className={`meta ellipsis working-text${waiting ? " waiting" : ""}`}>
      {activityLabel(activity, waiting)} · <Elapsed since={activity.since} />
    </div>
  );
}

export function ProvidersPanel({
  ready,
  providers,
  connections,
  projectPath,
  activeSessionId,
  starting,
  onNewSession,
  onOpenSession,
  onEditConnection,
  council,
  onOpenCouncil,
  onOpenRoute,
  activity = {},
  waitingCalls,
}: Props) {
  const list = providers.view?.providers ?? [];
  const byId = new Map((connections.view?.connections ?? []).map((c) => [c.connection.id, c]));
  const disabled = (connections.view?.connections ?? []).filter((c) => !c.connection.enabled);
  const warnings = connections.view?.warnings ?? [];
  const sessions = projectPath ? providers.sessions.filter((s) => s.projectPath === projectPath) : providers.sessions;
  const others = providers.sessions.length - sessions.length;

  return (
    <div className="panel">
      <div className="panel-header">
        <span>AI Providers</span>
        <button
          className="icon-button small"
          title="Adicionar API"
          disabled={!ready}
          onClick={() => onEditConnection(null)}
        >
          <PlusIcon />
        </button>
        <button
          className="icon-button small"
          title="Atualizar"
          disabled={!ready}
          onClick={() => {
            void providers.refresh();
            void connections.refresh();
          }}
        >
          <RefreshIcon />
        </button>
      </div>
      {providers.error && <div className="inline-error">{providers.error}</div>}
      {connections.error && <div className="inline-error">{connections.error}</div>}
      {warnings.map((warning) => (
        <div key={warning} className="inline-error">
          {warning}
        </div>
      ))}
      <div className="scroll">
        {providers.view && list.length === 0 && (
          <div className="placeholder">
            <p>Nenhuma API cadastrada.</p>
            <p className="meta">
              Cadastre quantas APIs quiser — OpenAI e compatíveis (OpenRouter, Groq, Ollama…), Anthropic, Gemini ou
              qualquer outra descrita por um perfil genérico.
            </p>
            <button className="button primary" disabled={!ready} onClick={() => onEditConnection(null)}>
              <PlusIcon /> Adicionar API
            </button>
          </div>
        )}
        {list.length > 0 && (
          <Collapsible id="providers.list" title={`IAs (${list.length})`} summary={list.find((p) => p.active)?.name}>
            <ul className="list provider-list">
              {list.map((provider) => (
                <ProviderCard
                  key={provider.id}
                  provider={provider}
                  health={providers.health[provider.id]}
                  connection={byId.get(provider.id)}
                  onSelect={() => void providers.select(provider.id)}
                  onInspect={() => void providers.inspect(provider.id)}
                  onEdit={byId.has(provider.id) ? () => onEditConnection(provider.id) : null}
                />
              ))}
            </ul>
            <button className="link add-connection" disabled={!ready} onClick={() => onEditConnection(null)}>
              + Adicionar API
            </button>
          </Collapsible>
        )}
        {disabled.length > 0 && (
          <Collapsible id="providers.disabled" title={`Conexões desativadas (${disabled.length})`} defaultOpen={false}>
            <ul className="list">
              {disabled.map(({ connection }) => (
                <li key={connection.id} className="list-item" onClick={() => onEditConnection(connection.id)}>
                  <span className="dot off" />
                  <div className="grow">
                    <div className="title">{connection.name}</div>
                    <div className="meta mono ellipsis">{connection.id}</div>
                  </div>
                  <EditIcon />
                </li>
              ))}
            </ul>
          </Collapsible>
        )}

        {list.length > 0 && (
          <Collapsible
            id="providers.council"
            title="Conselho"
            summary={council.view ? MODE_LABELS[council.view.settings.mode] : null}
            actions={
              <button className="link" disabled={!ready} onClick={onOpenCouncil}>
                Configurar
              </button>
            }
          >
            <div className="council-summary">
              {council.error && <div className="meta err-text">{council.error}</div>}
              {council.view && (
                <div className="meta">
                  Modo <strong>{MODE_LABELS[council.view.settings.mode]}</strong>
                  {council.view.settings.members.length === 0
                    ? " · sem membros (só o roteador)"
                    : ` · ${council.view.settings.members.map((m) => memberLabel(m, list)).join("; ")}`}
                </div>
              )}
              <button
                className="button small primary"
                disabled={!ready}
                title="Descreva a tarefa: o roteador e o Conselho escolhem o modelo"
                onClick={onOpenRoute}
              >
                Nova sessão com o Conselho
              </button>
            </div>
          </Collapsible>
        )}

        <div className="section-title">Sessões {sessions.length > 0 && `(${sessions.length})`}</div>
        {list.length > 0 && (
          <NewSession
            providers={list}
            starting={starting}
            disabled={!ready || !projectPath}
            title={!projectPath ? "Abra um projeto: sessões pertencem ao projeto" : "Iniciar sessão com o provider e o modelo escolhidos"}
            onStart={onNewSession}
            isConnection={(id) => byId.has(id)}
            onConfigure={onEditConnection}
          />
        )}
        {!projectPath && <div className="meta pad">Abra um projeto para iniciar sessões.</div>}
        <ul className="list">
          {projectPath && sessions.length === 0 && <li className="empty">Nenhuma sessão neste projeto.</li>}
          {sessionTree(sessions).map(({ session, depth }) => (
            <li
              key={session.id}
              className={`list-item ${session.id === activeSessionId ? "selected" : ""}`}
              style={{ paddingLeft: 12 + depth * 16 }}
              onClick={() => onOpenSession(session.id)}
              title={`${statusLabel(session.status)} · ${session.nativeRef ?? ""}`}
            >
              <span className={`dot ${statusDot(session.status)}`} />
              <div className="grow">
                <div className="title">
                  {depth > 0 && "↳ "}
                  {session.title}
                </div>
                <div className="meta ellipsis">
                  {list.some((p) => p.id === session.provider) ? (
                    session.provider
                  ) : (
                    <span className="err-text" title="Conexão removida ou desativada">
                      {session.provider} (indisponível)
                    </span>
                  )}{" "}
                  · {session.turns} {session.turns === 1 ? "turno" : "turnos"}
                  {session.turns > 0 && ` · ${formatTokens(session.usage)}`}
                  {!projectPath && ` · ${baseName(session.projectPath)}`}
                </div>
                {activity[session.id] && <Working activity={activity[session.id]!} waitingCalls={waitingCalls} />}
              </div>
            </li>
          ))}
        </ul>
        {projectPath && others > 0 && (
          <div className="meta pad">
            {others} {others === 1 ? "sessão" : "sessões"} em outros projetos.
          </div>
        )}
      </div>
      <div className="panel-footer meta">Providers nunca acessam o sistema: pedem tool_call ao Orchestrator.</div>
    </div>
  );
}
