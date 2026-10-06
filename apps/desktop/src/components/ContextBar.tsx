// Top bar: the execution context that must always be visible (section 24).

import { AGENT_STATUS_LABELS } from "../lib/agents";
import type { AgentStatus, GitStatusWithRemotes, ProcessInfo, TerminalInfo } from "../lib/types";

interface Chip {
  label: string;
  value: string;
  hint: string;
  pending?: boolean;
  /** Needs the user's attention (a request waiting, the AIs paused). */
  warn?: boolean;
  onClick?: () => void;
}

interface Props {
  projectName: string | null;
  /** Current branch label, or null without a project. */
  branch: string | null;
  gitStatus: GitStatusWithRemotes | null;
  terminals: TerminalInfo[];
  processes: ProcessInfo[];
  /** Active AI provider (null when none is registered). */
  providerName: string | null;
  /** Registered providers (null before loading). */
  providerCount: number | null;
  runningSessions: number;
  /** Task being worked on now, if any (ADR-0014). */
  task: { title: string; status: string } | null;
  /** How many tasks are open (not done, not cancelled). */
  openTasks: number;
  /** Agent of the open session, or the one on the current task (ADR-0015). */
  agent: { title: string; status: AgentStatus } | null;
  /** How the project's agents are doing, when none is in focus. */
  agentSummary: string;
  /** The autonomy mode, the pause and the requests waiting (ADR-0016). */
  autonomy: { value: string; hint: string; warn: boolean };
  onOpenAutonomy: () => void;
}

export function ContextBar({
  projectName,
  branch,
  gitStatus,
  terminals,
  processes,
  providerName,
  providerCount,
  runningSessions,
  task,
  openTasks,
  agent,
  agentSummary,
  autonomy,
  onOpenAutonomy,
}: Props) {
  const openTerminals = terminals.filter((t) => t.alive).length;
  const running = processes.filter((p) => p.status === "running").length;
  const chips: Chip[] = [
    {
      label: "Provider",
      value: providerName
        ? `${providerName}${runningSessions > 0 ? ` · ${runningSessions} ${runningSessions === 1 ? "sessão ativa" : "sessões ativas"}` : ""}`
        : "—",
      hint: providerName
        ? `Provider ativo de ${providerCount} registrado${providerCount === 1 ? "" : "s"} (painel AI PROVIDERS)`
        : "Nenhuma API cadastrada: adicione uma no painel AI PROVIDERS",
      pending: !providerName,
    },
    {
      label: "Task",
      value: task
        ? task.title
        : openTasks > 0
          ? `${openTasks} ${openTasks === 1 ? "aberta" : "abertas"}`
          : "—",
      hint: task
        ? `Task ${task.status.toLowerCase()} (painel TASKS)`
        : openTasks > 0
          ? "Nenhuma task em andamento; veja o painel TASKS"
          : "Nenhuma task ainda: crie uma no painel TASKS",
      pending: !task,
    },
    {
      label: "Agent",
      value: agent ? agent.title : agentSummary,
      hint: agent
        ? `Agente ${AGENT_STATUS_LABELS[agent.status].toLowerCase()} (painel AGENTS)`
        : agentSummary === "—"
          ? "Nenhum agente em execução: abra uma task e use \"Executar com um agente\""
          : "Agentes deste projeto (painel AGENTS)",
      pending: !agent && agentSummary === "—",
    },
    {
      label: "Autonomia",
      value: autonomy.value,
      hint: autonomy.hint,
      warn: autonomy.warn,
      onClick: onOpenAutonomy,
    },
    {
      label: "Branch",
      value: branch
        ? `${branch}${gitStatus && (gitStatus.ahead || gitStatus.behind) ? ` ↑${gitStatus.ahead} ↓${gitStatus.behind}` : ""}${gitStatus && !gitStatus.clean ? ` · ${gitStatus.files.length} ${gitStatus.files.length === 1 ? "alteração" : "alterações"}` : ""}`
        : "—",
      hint: gitStatus?.upstream ? `upstream: ${gitStatus.upstream}` : branch ? "Git local" : "Abra um projeto",
      pending: !branch,
    },
    {
      label: "Terminal",
      value: `${openTerminals} ${openTerminals === 1 ? "aberto" : "abertos"} · ${running} ${running === 1 ? "processo" : "processos"}`,
      hint: "Terminais e processos gerenciados pelo runtime",
    },
  ];
  return (
    <header className="context-bar">
      <div className="brand">
        <img src="/favicon.svg" alt="" width={18} height={18} />
        Orchestrator
        {projectName && <span className="brand-project">/ {projectName}</span>}
      </div>
      <div className="chips">
        {chips.map((chip) => {
          const className = `chip${chip.pending ? " pending" : ""}${chip.warn ? " warn" : ""}`;
          const content = (
            <>
              <span className="chip-label">{chip.label}</span>
              <span className="chip-value">{chip.value}</span>
            </>
          );
          return chip.onClick ? (
            <button key={chip.label} className={`${className} clickable`} title={chip.hint} onClick={chip.onClick}>
              {content}
            </button>
          ) : (
            <div key={chip.label} className={className} title={chip.hint}>
              {content}
            </div>
          );
        })}
      </div>
    </header>
  );
}
