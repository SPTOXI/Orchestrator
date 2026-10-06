// AGENTS panel (sidebar): who is executing the project's tasks right now,
// what they hold and the limits around them (ADR-0015); pause and resume,
// for one agent or for every AI (ADR-0016); what they cost, the queue and
// the daily budget (ADR-0018).

import { useEffect, useState } from "react";
import { MODE_LABELS } from "../lib/autonomy";
import {
  AGENT_STATUS_LABELS,
  agentProgress,
  groupAgents,
  isLive,
  locksOf,
} from "../lib/agents";
import { formatUsd } from "../lib/format";
import { errorMessage } from "../lib/runtime";
import type { AgentSettings, AgentView, BudgetView, FileLock } from "../lib/types";

interface Props {
  ready: boolean;
  agents: AgentView[];
  locks: FileLock[];
  settings: AgentSettings | null;
  /** Today's spending against the daily budget (ADR-0018). */
  budget: BudgetView | null;
  /** Providers that can run agents, for the per-provider limit. */
  providers: { id: string; name: string }[];
  onOpenCost: () => void;
  error: string | null;
  projectName: string | null;
  activeAgentId: string | null;
  onOpenBoard: () => void;
  onOpenTask: (taskId: string) => void;
  onStop: (id: string) => Promise<void>;
  onStopAll: () => Promise<void>;
  onSaveSettings: (settings: AgentSettings) => Promise<void>;
  /** Every AI is paused (ADR-0016). */
  pausedAll: boolean;
  onPause: (id: string) => Promise<void>;
  onResume: (id: string) => Promise<void>;
  onPauseAll: () => Promise<void>;
  onResumeAll: () => Promise<void>;
  onOpenAutonomy: () => void;
}

export function AgentsPanel({
  ready,
  agents,
  locks,
  settings,
  budget,
  providers,
  onOpenCost,
  error,
  projectName,
  activeAgentId,
  onOpenBoard,
  onOpenTask,
  onStop,
  onStopAll,
  onSaveSettings,
  pausedAll,
  onPause,
  onResume,
  onPauseAll,
  onResumeAll,
  onOpenAutonomy,
}: Props) {
  const [busy, setBusy] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const groups = groupAgents(agents);
  const live = agents.filter(isLive).length;

  const run = async (label: string, action: () => Promise<void>) => {
    setBusy(label);
    setFailure(null);
    try {
      await action();
    } catch (err) {
      setFailure(errorMessage(err));
    } finally {
      setBusy(null);
    }
  };

  const save = (label: string, next: AgentSettings) => void run(label, () => onSaveSettings(next));
  const change = (field: "maxParallel" | "maxTurns" | "maxSubagents", value: number) => {
    if (!settings) return;
    save(field, { ...settings, [field]: value });
  };
  const providerLimit = (provider: string, value: number | null) => {
    if (!settings) return;
    const limits = { ...settings.providerLimits };
    if (value === null) delete limits[provider];
    else limits[provider] = Math.max(1, Math.round(value));
    save(`limit:${provider}`, { ...settings, providerLimits: limits });
  };

  return (
    <div className="panel">
      <div className="panel-header">
        <span>Agents</span>
        {projectName && <span className="meta">{projectName}</span>}
      </div>
      {error && <div className="inline-error">{error}</div>}
      {failure && <div className="inline-error">{failure}</div>}
      <div className="scroll">
        {!projectName ? (
          <div className="placeholder">
            <p>Agentes executam as tasks do projeto.</p>
            <p className="meta">Abra um projeto para pôr um agente para trabalhar.</p>
          </div>
        ) : (
          <>
            <div className="pad row wrap">
              <button className="button" disabled={!ready} onClick={onOpenBoard}>
                Agent Board
              </button>
              <button
                className={`button${pausedAll ? " primary" : ""}`}
                disabled={!ready || busy !== null}
                onClick={() => void run("pauseAll", pausedAll ? onResumeAll : onPauseAll)}
                title={
                  pausedAll
                    ? "As IAs voltam a agir; a fila anda"
                    : "Toda chamada de ferramenta de qualquer IA espera; nenhum agente começa turno novo"
                }
              >
                {busy === "pauseAll" ? "…" : pausedAll ? "Retomar IAs" : "Pausar IAs"}
              </button>
              <button
                className="button danger"
                disabled={!ready || live === 0 || busy !== null}
                onClick={() => void run("stopAll", onStopAll)}
                title="Encerra todos os agentes deste projeto, inclusive os da fila"
              >
                {busy === "stopAll" ? "Parando…" : "Parar todos"}
              </button>
            </div>
            {budget && (
              <div className={`pad budget-line${budget.exhausted ? " exhausted" : ""}`}>
                <div className="budget-amount" title="Gasto das IAs neste projeto desde a meia-noite: sessões, agentes e Conselho">
                  Hoje: {formatUsd(budget.spentTodayUsd)}
                  {budget.budgetUsd !== null && ` de ${formatUsd(budget.budgetUsd).replace("US$ ", "")}`}
                  {budget.unpriced > 0 && (
                    <span className="meta" title="Chamadas de modelos sem preço configurado: o gasto real é maior">
                      {" "}
                      · {budget.unpriced} sem preço
                    </span>
                  )}
                </div>
                <button className="link meta" onClick={onOpenCost}>
                  Tokens e custo
                </button>
                {budget.exhausted && (
                  <div className="inline-notice">
                    O orçamento diário acabou: nenhum agente começa, e os que estão rodando param no fim do turno.
                    Suas sessões continuam.
                  </div>
                )}
              </div>
            )}
            {agents.length === 0 ? (
              <div className="placeholder">
                <p className="meta">
                  Nenhum agente ainda. Abra uma task e use "Executar com um agente": ele abre a
                  sessão, trabalha até chamar <code>agent.finish</code> e deixa a task em revisão.
                </p>
              </div>
            ) : (
              groups.map((group) => (
                <section key={group.status} className="task-group">
                  <h4 className={`agent-status ${group.status.toLowerCase()}`}>
                    {AGENT_STATUS_LABELS[group.status]}
                    <span className="meta"> {group.agents.length}</span>
                  </h4>
                  <ul className="plain-list">
                    {group.agents.map((agent) => {
                      const held = locksOf(locks, agent.id);
                      return (
                        <li
                          key={agent.id}
                          className={`list-item${agent.id === activeAgentId ? " selected" : ""}`}
                          onClick={() => onOpenTask(agent.task)}
                          title={agent.error ?? agent.result ?? agent.taskTitle}
                        >
                          <div className="grow">
                            <div className="title ellipsis">{agent.taskTitle}</div>
                            <div className="meta task-meta">
                              <span className={agent.approval ? "waiting-approval" : "ellipsis"}>
                                {[
                                  agent.provider,
                                  agent.autonomy ? MODE_LABELS[agent.autonomy] : null,
                                  agentProgress(agent),
                                ]
                                  .filter(Boolean)
                                  .join(" · ")}
                              </span>
                            </div>
                            {held.length > 0 && (
                              <div className="meta ellipsis" title={held.map((l) => l.path).join("\n")}>
                                🔒 {held.map((lock) => lock.path).join(", ")}
                              </div>
                            )}
                            {isLive(agent) && (
                              <div className="row tight agent-actions">
                                {agent.status === "RUNNING" && (
                                  <button
                                    className="button small"
                                    disabled={!ready || busy !== null}
                                    title={
                                      agent.paused
                                        ? "Volta a trabalhar"
                                        : "Para na próxima chamada de ferramenta ou no próximo turno, sem perder a vaga nem os arquivos"
                                    }
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      void run(`pause:${agent.id}`, () =>
                                        agent.paused ? onResume(agent.id) : onPause(agent.id),
                                      );
                                    }}
                                  >
                                    {busy === `pause:${agent.id}` ? "…" : agent.paused ? "Retomar" : "Pausar"}
                                  </button>
                                )}
                                <button
                                  className="button small danger"
                                  disabled={!ready || busy !== null}
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    void run(agent.id, () => onStop(agent.id));
                                  }}
                                >
                                  {busy === agent.id ? "…" : "Parar"}
                                </button>
                              </div>
                            )}
                          </div>
                        </li>
                      );
                    })}
                  </ul>
                </section>
              ))
            )}
            {settings && (
              <section className="task-group">
                <h4>Limites</h4>
                <div className="pad">
                  <label className="field">
                    <span>Agentes ao mesmo tempo</span>
                    <input
                      type="number"
                      min={1}
                      max={8}
                      value={settings.maxParallel}
                      disabled={!ready || busy !== null}
                      onChange={(e) => change("maxParallel", Number(e.target.value))}
                    />
                  </label>
                  <label className="field">
                    <span>Turnos por agente</span>
                    <input
                      type="number"
                      min={1}
                      max={50}
                      value={settings.maxTurns}
                      disabled={!ready || busy !== null}
                      onChange={(e) => change("maxTurns", Number(e.target.value))}
                    />
                  </label>
                  <label className="field" title="0 desliga a delegação: o agente faz tudo sozinho">
                    <span>Subagentes por agente</span>
                    <input
                      type="number"
                      min={0}
                      max={10}
                      value={settings.maxSubagents}
                      disabled={!ready || busy !== null}
                      onChange={(e) => change("maxSubagents", Number(e.target.value))}
                    />
                  </label>
                  <label className="field" title="O agente para (com handoff) antes do turno em que passaria deste gasto">
                    <span>Teto de custo por agente (US$)</span>
                    <MoneyField
                      value={settings.maxCostUsd}
                      placeholder="sem teto"
                      disabled={!ready || busy !== null}
                      onCommit={(value) => save("maxCostUsd", { ...settings, maxCostUsd: value })}
                    />
                  </label>
                  <label
                    className="field"
                    title="Gasto do projeto no dia (todas as IAs). Ao chegar nele, agentes não começam e os que rodam param; suas sessões continuam."
                  >
                    <span>Orçamento diário do projeto (US$)</span>
                    <MoneyField
                      value={settings.dailyBudgetUsd}
                      placeholder="sem orçamento"
                      disabled={!ready || busy !== null}
                      onCommit={(value) => save("dailyBudgetUsd", { ...settings, dailyBudgetUsd: value })}
                    />
                  </label>
                  {providers.length > 0 && (
                    <div className="field">
                      <span title="Agentes de um provider ao mesmo tempo — para os limites de taxa da conta">
                        Por provider (agentes ao mesmo tempo)
                      </span>
                      {providers.map((provider) => (
                        <label key={provider.id} className="row tight provider-limit">
                          <span className="grow ellipsis">{provider.name}</span>
                          <MoneyField
                            value={settings.providerLimits[provider.id] ?? null}
                            placeholder={`até ${settings.maxParallel}`}
                            integer
                            disabled={!ready || busy !== null}
                            onCommit={(value) => providerLimit(provider.id, value)}
                          />
                        </label>
                      ))}
                    </div>
                  )}
                  <p className="meta">
                    Os limites dizem quanto um agente roda (custo). O que ele pode fazer sem
                    perguntar é o modo de autonomia. A fila anda por prioridade da task.
                  </p>
                  <button className="button small" onClick={onOpenAutonomy}>
                    Autonomia
                  </button>
                </div>
              </section>
            )}
          </>
        )}
      </div>
    </div>
  );
}

/** A positive number committed on blur or Enter; empty = none. */
function MoneyField({
  value,
  placeholder,
  disabled,
  integer = false,
  onCommit,
}: {
  value: number | null;
  placeholder: string;
  disabled: boolean;
  integer?: boolean;
  onCommit: (value: number | null) => void;
}) {
  const shown = value === null ? "" : String(value).replace(".", ",");
  const [text, setText] = useState(shown);
  useEffect(() => setText(shown), [shown]);
  const commit = () => {
    const trimmed = text.trim().replace(",", ".");
    const next = trimmed === "" ? null : Number(trimmed);
    if (next !== null && (!Number.isFinite(next) || next <= 0 || (integer && !Number.isInteger(next)))) {
      setText(shown);
      return;
    }
    if (next !== value) onCommit(next);
  };
  return (
    <input
      type="text"
      inputMode={integer ? "numeric" : "decimal"}
      className="narrow"
      value={text}
      placeholder={placeholder}
      disabled={disabled}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
      }}
    />
  );
}
