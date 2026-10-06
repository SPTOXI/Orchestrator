// TASKS panel (sidebar): the work of the project by state (ADR-0014).
// Every relevant activity is a task; the details open in the main area.

import { type FormEvent, useState } from "react";
import { filterTasks, groupByStatus, TASK_PRIORITY_LABELS, TASK_STATUS_LABELS } from "../lib/tasks";
import type { TaskView } from "../lib/types";

interface Props {
  ready: boolean;
  tasks: TaskView[];
  error: string | null;
  projectName: string | null;
  activeTaskId: string | null;
  onOpen: (id: string) => void;
  onNew: () => void;
}

export function TasksPanel({ ready, tasks, error, projectName, activeTaskId, onOpen, onNew }: Props) {
  const [query, setQuery] = useState("");
  const groups = groupByStatus(filterTasks(tasks, query));
  const open = tasks.filter((task) => task.status !== "DONE" && task.status !== "CANCELLED").length;

  const submit = (event: FormEvent) => event.preventDefault();

  return (
    <div className="panel">
      <div className="panel-header">
        <span>Tasks</span>
        {projectName && <span className="meta">{projectName}</span>}
      </div>
      {error && <div className="inline-error">{error}</div>}
      <div className="scroll">
        {!projectName ? (
          <div className="placeholder">
            <p>As tasks pertencem ao projeto.</p>
            <p className="meta">Abra um projeto para planejar o trabalho nele.</p>
          </div>
        ) : (
          <>
            <div className="pad">
              <button className="button full primary" disabled={!ready} onClick={onNew}>
                + Nova task
              </button>
            </div>
            {tasks.length > 0 && (
              <form className="pad" onSubmit={submit}>
                <input
                  className="full"
                  value={query}
                  placeholder="Buscar nas tasks…"
                  onChange={(e) => setQuery(e.target.value)}
                />
              </form>
            )}
            {tasks.length === 0 ? (
              <div className="placeholder">
                <p className="meta">
                  Nenhuma task ainda. Uma task guarda o objetivo, o que falta e a IA escolhida, e vira
                  o contexto da sessão que trabalha nela.
                </p>
              </div>
            ) : (
              <>
                <div className="pad meta">
                  {open} {open === 1 ? "task aberta" : "tasks abertas"} de {tasks.length}
                </div>
                {groups.map((group) => (
                  <section key={group.status} className="task-group">
                    <h4 className={`task-status ${group.status.toLowerCase()}`}>
                      {TASK_STATUS_LABELS[group.status]}
                      <span className="meta"> {group.tasks.length}</span>
                    </h4>
                    <ul className="plain-list">
                      {group.tasks.map((task) => (
                        <li
                          key={task.id}
                          className={`list-item${task.id === activeTaskId ? " selected" : ""}`}
                          onClick={() => onOpen(task.id)}
                          title={task.description || task.title}
                        >
                          <div className="grow">
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
                              <span className="ellipsis">
                                {[
                                  task.subtasks.total > 0
                                    ? `${task.subtasks.done}/${task.subtasks.total} subtasks`
                                    : null,
                                  task.provider,
                                ]
                                  .filter(Boolean)
                                  .join(" · ")}
                              </span>
                            </div>
                          </div>
                        </li>
                      ))}
                    </ul>
                  </section>
                ))}
              </>
            )}
          </>
        )}
      </div>
    </div>
  );
}
