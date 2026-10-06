// Pure helpers for the TASKS panel and the task view (unit tested;
// ADR-0014). The engine owns the rules; here we only name and group.

import type { Task, TaskPriority, TaskStatus, TaskView } from "./types";

export const TASK_STATUS_LABELS: Record<TaskStatus, string> = {
  TODO: "A fazer",
  IN_PROGRESS: "Em andamento",
  BLOCKED: "Bloqueada",
  REVIEW: "Em revisão",
  DONE: "Concluída",
  CANCELLED: "Cancelada",
};

/** What the button that moves a task from `from` to `to` says. Going back
 * to TODO is reopening ended work, but parking work in flight. */
export function taskActionLabel(from: TaskStatus, to: TaskStatus): string {
  if (to === "TODO") return from === "DONE" || from === "CANCELLED" ? "Reabrir" : "Voltar para a fila";
  return {
    IN_PROGRESS: "Iniciar",
    BLOCKED: "Bloquear",
    REVIEW: "Enviar para revisão",
    DONE: "Concluir",
    CANCELLED: "Cancelar",
    TODO: "Reabrir",
  }[to];
}

export const TASK_PRIORITY_LABELS: Record<TaskPriority, string> = {
  LOW: "Baixa",
  NORMAL: "Normal",
  HIGH: "Alta",
  URGENT: "Urgente",
};

/** Panel order, the same the engine sorts by. */
export const TASK_STATUS_ORDER: TaskStatus[] = [
  "IN_PROGRESS",
  "REVIEW",
  "BLOCKED",
  "TODO",
  "DONE",
  "CANCELLED",
];

/** Tasks by state, in panel order; empty states are left out. */
export function groupByStatus(tasks: TaskView[]): Array<{ status: TaskStatus; tasks: TaskView[] }> {
  return TASK_STATUS_ORDER.map((status) => ({
    status,
    tasks: tasks.filter((task) => task.status === status),
  })).filter((group) => group.tasks.length > 0);
}

/** Title, description and files: what the router and the context read. */
export function taskText(task: Pick<Task, "title" | "description">): string {
  return [task.title, task.description].filter((part) => part.trim()).join(". ");
}

/** "espera 2 tasks", "2 de 5 subtasks", "" when there is nothing to say. */
export function taskWaitLine(task: TaskView): string {
  const parts: string[] = [];
  if (task.waitingFor.length > 0) {
    const count = task.waitingFor.length;
    parts.push(`espera ${count} ${count === 1 ? "task" : "tasks"}`);
  }
  if (task.subtasks.total > 0) {
    parts.push(`${task.subtasks.done} de ${task.subtasks.total} subtasks`);
  }
  return parts.join(" · ");
}

/** Case- and accent-insensitive filter over title and description. */
export function filterTasks(tasks: TaskView[], query: string): TaskView[] {
  const fold = (text: string) =>
    text
      .normalize("NFD")
      .replace(/[̀-ͯ]/g, "")
      .toLowerCase();
  const needle = fold(query.trim());
  if (!needle) return tasks;
  return tasks.filter((task) => fold(`${task.title} ${task.description}`).includes(needle));
}
