// Top bar: the execution context that must always be visible (section 24).

import type { GitStatusWithRemotes, ProcessInfo, TerminalInfo } from "../lib/types";

interface Chip {
  label: string;
  value: string;
  hint: string;
  pending?: boolean;
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
    { label: "Agent", value: "—", hint: "Agent Manager: Fase 8b", pending: true },
    { label: "Autonomia", value: "—", hint: "Assistido / Autônomo / Acesso Irrestrito: Fase 9", pending: true },
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
        {chips.map((chip) => (
          <div key={chip.label} className={`chip ${chip.pending ? "pending" : ""}`} title={chip.hint}>
            <span className="chip-label">{chip.label}</span>
            <span className="chip-value">{chip.value}</span>
          </div>
        ))}
      </div>
    </header>
  );
}
