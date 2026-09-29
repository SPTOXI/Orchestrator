// AI PROVIDERS panel: registered providers (Provider Registry), their
// availability and capabilities, the active provider and the sessions of the
// open project (ADR-0009).

import { baseName } from "../lib/format";
import { formatTokens } from "../lib/transcript";
import type { ProviderHealth, Providers } from "../lib/useProviders";
import type { ProviderCapabilities, ProviderInfo, SessionInfo, SessionStatus } from "../lib/types";
import { PlusIcon, RefreshIcon } from "./icons";

interface Props {
  ready: boolean;
  providers: Providers;
  /** Open project; sessions are listed per project. */
  projectPath: string | null;
  activeSessionId: string | null;
  starting: boolean;
  onNewSession: () => void;
  onOpenSession: (id: string) => void;
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
];

export function statusDot(status: SessionStatus): string {
  return status === "running" ? "ok pulse" : status === "idle" ? "ok" : "off";
}

export function statusLabel(status: SessionStatus): string {
  return status === "running" ? "em execução" : status === "idle" ? "pronta" : "encerrada";
}

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
  onSelect,
  onInspect,
}: {
  provider: ProviderInfo;
  health: ProviderHealth | undefined;
  onSelect: () => void;
  onInspect: () => void;
}) {
  const available = health && "status" in health ? health.status.available : null;
  const caps = provider.capabilities;
  return (
    <li className={`provider-card ${provider.active ? "active" : ""}`}>
      <div className="row">
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
      </div>
      <div className="meta mono">
        {provider.id} · {provider.vendor}
        {caps.defaultModel ? ` · ${caps.defaultModel}` : ""}
      </div>
      <HealthLine health={health} />
      <div className="chip-list">
        {CAPABILITY_LABELS.filter(([key]) => caps[key] === true).map(([key, label]) => (
          <span key={key} className="tag small">
            {label}
          </span>
        ))}
      </div>
    </li>
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

export function ProvidersPanel({
  ready,
  providers,
  projectPath,
  activeSessionId,
  starting,
  onNewSession,
  onOpenSession,
}: Props) {
  const list = providers.view?.providers ?? [];
  const active = list.find((p) => p.active) ?? null;
  const sessions = projectPath ? providers.sessions.filter((s) => s.projectPath === projectPath) : providers.sessions;
  const others = providers.sessions.length - sessions.length;

  return (
    <div className="panel">
      <div className="panel-header">
        <span>AI Providers</span>
        <button
          className="icon-button small"
          title="Atualizar"
          disabled={!ready}
          onClick={() => void providers.refresh()}
        >
          <RefreshIcon />
        </button>
      </div>
      {providers.error && <div className="inline-error">{providers.error}</div>}
      <div className="scroll">
        {providers.view && list.length === 0 && (
          <div className="placeholder">
            <p>Nenhum provider registrado neste build.</p>
            <ul>
              <li>Fase 4 — OpenAI / Codex</li>
              <li>Fase 5 — Claude Code</li>
            </ul>
            <p className="meta">
              O provider de desenvolvimento <code>echo</code> aparece em builds de desenvolvimento ou com{" "}
              <code>ORCHESTRATOR_ECHO_PROVIDER=1</code>.
            </p>
          </div>
        )}
        <ul className="list provider-list">
          {list.map((provider) => (
            <ProviderCard
              key={provider.id}
              provider={provider}
              health={providers.health[provider.id]}
              onSelect={() => void providers.select(provider.id)}
              onInspect={() => void providers.inspect(provider.id)}
            />
          ))}
        </ul>

        <div className="section-title row">
          <span className="grow">
            Sessões {sessions.length > 0 && `(${sessions.length})`}
          </span>
          <button
            className="button small primary"
            disabled={!ready || !active || !projectPath || starting}
            title={
              !active
                ? "Nenhum provider registrado"
                : !projectPath
                  ? "Abra um projeto: sessões pertencem ao projeto"
                  : `Nova sessão com ${active.name}`
            }
            onClick={onNewSession}
          >
            <PlusIcon /> Nova sessão
          </button>
        </div>
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
                  {session.provider} · {session.turns} {session.turns === 1 ? "turno" : "turnos"}
                  {session.turns > 0 && ` · ${formatTokens(session.usage)}`}
                  {!projectPath && ` · ${baseName(session.projectPath)}`}
                </div>
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
