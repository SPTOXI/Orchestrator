import { describe, expect, it } from "vitest";
import { filterTasks, groupByStatus, taskActionLabel, taskText, taskWaitLine } from "./tasks";
import type { TaskStatus, TaskView } from "./types";

function task(title: string, status: TaskStatus, extra: Partial<TaskView> = {}): TaskView {
  return {
    id: title,
    projectId: "p",
    title,
    description: "",
    status,
    priority: "NORMAL",
    provider: null,
    model: null,
    agent: null,
    parentTask: null,
    dependencies: [],
    files: [],
    sessions: [],
    result: "",
    createdAt: "2026-09-30T00:00:00Z",
    updatedAt: "2026-09-30T00:00:00Z",
    startedAt: null,
    finishedAt: null,
    waitingFor: [],
    subtasks: { done: 0, total: 0 },
    can: [],
    ...extra,
  };
}

describe("task helpers", () => {
  it("groups by state in panel order and drops the empty ones", () => {
    const groups = groupByStatus([
      task("Concluída", "DONE"),
      task("Fazendo", "IN_PROGRESS"),
      task("A fazer", "TODO"),
      task("Outra em andamento", "IN_PROGRESS"),
    ]);
    expect(groups.map((g) => g.status)).toEqual(["IN_PROGRESS", "TODO", "DONE"]);
    expect(groups[0]?.tasks.map((t) => t.title)).toEqual(["Fazendo", "Outra em andamento"]);
  });

  it("says what a task is waiting for", () => {
    expect(taskWaitLine(task("x", "TODO"))).toBe("");
    const waiting = task("x", "TODO", {
      waitingFor: [{ id: "a", title: "Banco", status: "TODO" }],
      subtasks: { done: 2, total: 5 },
    });
    expect(taskWaitLine(waiting)).toBe("espera 1 task · 2 de 5 subtasks");
  });

  it("filters without accents or case, over title and description", () => {
    const tasks = [
      task("Cobranças no gateway", "TODO"),
      task("Outra", "TODO", { description: "mexe na COBRANÇA recorrente" }),
      task("Sem relação", "TODO"),
    ];
    expect(filterTasks(tasks, "cobranc").map((t) => t.title)).toEqual([
      "Cobranças no gateway",
      "Outra",
    ]);
    expect(filterTasks(tasks, "  ")).toHaveLength(3);
  });

  it("names the move by where the task comes from", () => {
    // Ended work is reopened; work in flight goes back to the queue.
    expect(taskActionLabel("DONE", "TODO")).toBe("Reabrir");
    expect(taskActionLabel("CANCELLED", "TODO")).toBe("Reabrir");
    expect(taskActionLabel("IN_PROGRESS", "TODO")).toBe("Voltar para a fila");
    expect(taskActionLabel("TODO", "IN_PROGRESS")).toBe("Iniciar");
    expect(taskActionLabel("REVIEW", "DONE")).toBe("Concluir");
  });

  it("reads a task as the router and the context read it", () => {
    expect(taskText({ title: "Aplicar retentativas", description: "Com backoff." })).toBe(
      "Aplicar retentativas. Com backoff.",
    );
    expect(taskText({ title: "Só o título", description: "  " })).toBe("Só o título");
  });
});
