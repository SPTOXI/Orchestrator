import { describe, expect, it } from "vitest";
import {
  agentChip,
  agentOfTask,
  agentProgress,
  groupAgents,
  isLive,
  locksOf,
} from "./agents";
import type { AgentStatus, AgentView, FileLock } from "./types";

function agent(id: string, status: AgentStatus, extra: Partial<AgentView> = {}): AgentView {
  return {
    id,
    projectId: "p",
    task: `task-${id}`,
    title: id,
    provider: "nuvem",
    model: null,
    session: null,
    parentAgent: null,
    status,
    tools: [],
    context: null,
    turns: 0,
    maxTurns: 12,
    files: [],
    result: "",
    error: null,
    handoff: null,
    autonomy: null,
    createdAt: "2026-09-30T00:00:00Z",
    updatedAt: "2026-09-30T00:00:00Z",
    startedAt: null,
    finishedAt: null,
    waiting: null,
    taskTitle: id,
    taskStatus: "IN_PROGRESS",
    paused: false,
    approval: null,
    mode: "assisted",
    costUsd: null,
    queuePosition: null,
    ...extra,
  };
}

describe("agent helpers", () => {
  it("groups by state in panel order and drops the empty ones", () => {
    const groups = groupAgents([
      agent("concluido", "DONE"),
      agent("fila", "QUEUED"),
      agent("rodando", "RUNNING"),
      agent("outro-fila", "QUEUED"),
    ]);
    expect(groups.map((g) => g.status)).toEqual(["RUNNING", "QUEUED", "DONE"]);
    expect(groups[1]?.agents.map((a) => a.id)).toEqual(["fila", "outro-fila"]);
    // Only what still holds a slot is live.
    expect(isLive(agent("x", "QUEUED"))).toBe(true);
    expect(isLive(agent("x", "STOPPED"))).toBe(false);
  });

  it("says what an agent is doing, or why it is not", () => {
    expect(agentProgress(agent("x", "QUEUED", { waiting: "src/a.ts está com \"Outro\"" }))).toBe(
      'src/a.ts está com "Outro"',
    );
    // Nothing in the way: the state already says it is queued.
    expect(agentProgress(agent("x", "QUEUED"))).toBe("");
    expect(agentProgress(agent("x", "QUEUED", { queuePosition: 2, waiting: "1 agente em execução" }))).toBe(
      "2º na fila · 1 agente em execução",
    );
    expect(agentProgress(agent("x", "DONE", { turns: 4, costUsd: 0.025 }))).toBe("4 de 12 turnos · US$ 0,025");
    expect(agentProgress(agent("x", "RUNNING", { turns: 3 }))).toBe("3 de 12 turnos");
    expect(
      agentProgress(agent("x", "RUNNING", { turns: 3, files: ["a.ts", "b.ts"] })),
    ).toBe("3 de 12 turnos · 2 arquivos");
    // Waiting for the user, or paused (ADR-0016).
    expect(
      agentProgress(agent("x", "RUNNING", { turns: 1, approval: "Executar `npm test`" })),
    ).toBe("esperando sua autorização: Executar `npm test`");
    expect(agentProgress(agent("x", "RUNNING", { turns: 2, paused: true }))).toBe(
      "pausado · 2 de 12 turnos",
    );
    // Ended agents keep the count, not the files (they let them go).
    expect(agentProgress(agent("x", "FAILED", { turns: 12 }))).toBe("12 de 12 turnos");
  });

  it("finds the agent of a task, preferring the one still working", () => {
    const list = [
      agent("velho", "FAILED", { task: "t1" }),
      agent("atual", "RUNNING", { task: "t1" }),
      agent("outra", "RUNNING", { task: "t2" }),
    ];
    expect(agentOfTask(list, "t1")?.id).toBe("atual");
    expect(agentOfTask(list, "t3")).toBeNull();
    expect(agentOfTask(list, null)).toBeNull();
  });

  it("names the agent of the open session, else how the project is doing", () => {
    const list = [
      agent("na-sessao", "RUNNING", { session: "s1", title: "Aplicar retentativas" }),
      agent("fila", "QUEUED"),
    ];
    expect(agentChip(list, "s1")).toBe("Aplicar retentativas");
    expect(agentChip(list, "outra")).toBe("1 em execução · 1 na fila");
    expect(agentChip([agent("x", "DONE")], null)).toBe("—");
  });

  it("keeps each agent's locks apart", () => {
    const lock = (path: string, agentId: string): FileLock => ({
      projectId: "p",
      path,
      agentId,
      agentTitle: agentId,
      task: "t",
      at: "2026-09-30T00:00:00Z",
    });
    const locks = [lock("a.ts", "um"), lock("b.ts", "dois"), lock("c.ts", "um")];
    expect(locksOf(locks, "um").map((l) => l.path)).toEqual(["a.ts", "c.ts"]);
    expect(locksOf(locks, "tres")).toEqual([]);
  });
});
