// Agent Board (master document, section 25): the work in columns, with the
// agent, the provider, the state and the progress of each task (ADR-0015),
// and whether it is paused or waiting for the user (ADR-0016).

import { AGENT_STATUS_LABELS, BOARD_COLUMNS, agentOfTask, agentProgress } from "../lib/agents";
import { TASK_PRIORITY_LABELS } from "../lib/tasks";
import type { AgentView, TaskView } from "../lib/types";

interface Props {
  active: boolean;
  tasks: TaskView[];
  agents: AgentView[];
  onOpenTask: (id: string) => void;
  onOpenSession: (id: string) => void;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
}

/** The state an agent's card shows: a paused or waiting agent is still
 * running, but that is not what the user needs to see. */
function boardState(agent: AgentView): { label: string; className: string } {
  if (agent.status === "RUNNING" && agent.approval) return { label: "Esperando você", className: "waiting" };
  if (agent.status === "RUNNING" && agent.paused) return { label: "Pausado", className: "paused" };
  return { label: AGENT_STATUS_LABELS[agent.status], className: agent.status.toLowerCase() };
}

export function BoardView({ active, tasks, agents, onOpenTask, onOpenSession, onPause, onResume }: Props) {
  const columns = BOARD_COLUMNS.map((column) => ({
    ...column,
    tasks: tasks.filter((task) => task.status === column.status),
  }));
  const blocked = tasks.filter((task) => task.status === "BLOCKED");

  return (
    <div className="editor board-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">Agent Board</span>
        <span className="meta grow">
          O trabalho do projeto por estado, com quem está executando cada task.
        </span>
      </div>
      <div className="board">
        {columns.map((column) => (
          <section key={column.status} className="board-column">
            <h3 className={`task-status ${column.status.toLowerCase()}`}>
              {column.title}
              <span className="meta"> {column.tasks.length}</span>
            </h3>
            {column.tasks.length === 0 && <p className="meta pad">Nada aqui.</p>}
            {column.tasks.map((task) => {
              const agent = agentOfTask(agents, task.id);
              return (
                <article key={task.id} className="board-card" onClick={() => onOpenTask(task.id)}>
                  <div className="title ellipsis">{task.title}</div>
                  <div className="meta task-meta">
                    {task.priority !== "NORMAL" && (
                      <span className={`badge priority ${task.priority.toLowerCase()}`}>
                        {TASK_PRIORITY_LABELS[task.priority]}
                      </span>
                    )}
                    {task.waitingFor.length > 0 && (
                      <span className="badge waiting">espera {task.waitingFor.length}</span>
                    )}
                    {task.subtasks.total > 0 && (
                      <span>
                        {task.subtasks.done}/{task.subtasks.total} subtasks
                      </span>
                    )}
                  </div>
                  {agent ? (
                    <div className="board-agent">
                      <span className={`agent-status ${boardState(agent).className}`}>
                        {boardState(agent).label}
                      </span>
                      <span className="meta ellipsis">
                        {[agent.provider, agent.model, agentProgress(agent)]
                          .filter(Boolean)
                          .join(" · ")}
                      </span>
                      {agent.session && (
                        <button
                          className="subagent-link"
                          onClick={(e) => {
                            e.stopPropagation();
                            onOpenSession(agent.session as string);
                          }}
                        >
                          Ver a sessão
                        </button>
                      )}
                      {agent.status === "RUNNING" && (
                        <button
                          className="subagent-link"
                          onClick={(e) => {
                            e.stopPropagation();
                            if (agent.paused) onResume(agent.id);
                            else onPause(agent.id);
                          }}
                        >
                          {agent.paused ? "Retomar" : "Pausar"}
                        </button>
                      )}
                    </div>
                  ) : (
                    <div className="meta">
                      {task.provider ? `${task.provider} · sem agente` : "sem agente"}
                    </div>
                  )}
                </article>
              );
            })}
          </section>
        ))}
      </div>
      {blocked.length > 0 && (
        <p className="meta pad">
          {blocked.length} {blocked.length === 1 ? "task bloqueada" : "tasks bloqueadas"} pelo
          usuário {blocked.length === 1 ? "não aparece" : "não aparecem"} no quadro.
        </p>
      )}
    </div>
  );
}
