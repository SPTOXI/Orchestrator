// Pure helpers for the AGENTS panel and the Agent Board (unit tested;
// ADR-0015). The rules are the agent manager's; here we only name, group
// and count.

import { formatUsd } from "./format";
import type { AgentStatus, AgentView, FileLock, TaskStatus } from "./types";

export const AGENT_STATUS_LABELS: Record<AgentStatus, string> = {
  QUEUED: "Na fila",
  RUNNING: "Em execução",
  DONE: "Concluído",
  FAILED: "Falhou",
  STOPPED: "Parado",
};

/** Panel order, the same the manager sorts by. */
export const AGENT_STATUS_ORDER: AgentStatus[] = [
  "RUNNING",
  "QUEUED",
  "DONE",
  "FAILED",
  "STOPPED",
];

/** Queued or running: it still holds a slot and its files. */
export function isLive(agent: AgentView): boolean {
  return agent.status === "QUEUED" || agent.status === "RUNNING";
}

/** Agents by state, in panel order; empty states are left out. */
export function groupAgents(agents: AgentView[]): Array<{ status: AgentStatus; agents: AgentView[] }> {
  return AGENT_STATUS_ORDER.map((status) => ({
    status,
    agents: agents.filter((agent) => agent.status === status),
  })).filter((group) => group.agents.length > 0);
}

/** Columns of the Agent Board (master document, section 25). */
export const BOARD_COLUMNS: Array<{ status: TaskStatus; title: string }> = [
  { status: "TODO", title: "A fazer" },
  { status: "IN_PROGRESS", title: "Em andamento" },
  { status: "REVIEW", title: "Em revisão" },
];

/** What an agent is doing, in one line: turns, or why it is waiting. The
 * state itself is shown beside it, so a queued agent with nothing in its
 * way says nothing. */
export function agentProgress(agent: AgentView): string {
  if (agent.status === "QUEUED") {
    const place = agent.queuePosition ? `${agent.queuePosition}º na fila` : "";
    return [place, agent.waiting].filter(Boolean).join(" · ");
  }
  const cost = agent.costUsd !== null && agent.costUsd !== undefined ? ` · ${formatUsd(agent.costUsd)}` : "";
  const turns = `${agent.turns} de ${agent.maxTurns} turnos${cost}`;
  if (agent.status !== "RUNNING") return turns;
  // What it waits for comes first: it is what the user can act on.
  if (agent.approval) return `esperando sua autorização: ${agent.approval}`;
  const files = agent.files.length > 0
    ? ` · ${agent.files.length} ${agent.files.length === 1 ? "arquivo" : "arquivos"}`
    : "";
  return agent.paused ? `pausado · ${turns}${files}` : `${turns}${files}`;
}

/** The agent of a task: the one still working, else the newest one. */
export function agentOfTask(agents: AgentView[], taskId: string | null): AgentView | null {
  if (!taskId) return null;
  const mine = agents.filter((agent) => agent.task === taskId);
  return mine.find(isLive) ?? mine[0] ?? null;
}

/** What the AGENT chip shows: the agent of the open session, or how the
 * agents of the project are doing. */
export function agentChip(agents: AgentView[], sessionId: string | null): string {
  const mine = sessionId ? agents.find((agent) => agent.session === sessionId) : undefined;
  if (mine) return mine.title;
  const running = agents.filter((agent) => agent.status === "RUNNING").length;
  const queued = agents.filter((agent) => agent.status === "QUEUED").length;
  if (running === 0 && queued === 0) return "—";
  const parts = [];
  if (running > 0) parts.push(`${running} em execução`);
  if (queued > 0) parts.push(`${queued} na fila`);
  return parts.join(" · ");
}

/** Locks of one agent, for the panel. */
export function locksOf(locks: FileLock[], agentId: string): FileLock[] {
  return locks.filter((lock) => lock.agentId === agentId);
}
