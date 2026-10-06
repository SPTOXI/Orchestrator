import { describe, expect, it } from "vitest";
import {
  autonomyChip,
  describeRule,
  fromDraft,
  grantLabel,
  moveItem,
  requester,
  sameRules,
  toDraft,
  trialArgs,
  waitedFor,
} from "./autonomy";
import type { AutonomyOverview, PolicyRule } from "./types";

function overview(extra: Partial<AutonomyOverview> = {}): AutonomyOverview {
  return {
    projectId: "p1",
    mode: "assisted",
    projectMode: null,
    defaultMode: "assisted",
    rules: [],
    assistedRules: [],
    defaultRules: [],
    pausedAll: false,
    pausedAgents: [],
    pending: 0,
    grants: [],
    warning: null,
    ...extra,
  };
}

describe("autonomy", () => {
  it("describes rules the way the engine does", () => {
    expect(describeRule({ tools: ["*"], where: "outside", decision: "ask" })).toBe(
      "fora do projeto → perguntar",
    );
    expect(describeRule({ tools: [], command: "rm *", decision: "ask" })).toBe(
      "comando `rm *` → perguntar",
    );
    expect(describeRule({ tools: [], access: "read", decision: "allow" })).toBe(
      "consultas → permitir",
    );
    expect(describeRule({ tools: ["git.push", "git.reset"], decision: "deny" })).toBe(
      "git.push, git.reset → negar",
    );
    expect(describeRule({ tools: [], decision: "allow" })).toBe("qualquer chamada → permitir");
  });

  it("edits rules as text and back", () => {
    const rule: PolicyRule = {
      tools: ["git.push", "git.reset"],
      access: null,
      where: "inside",
      command: null,
      path: "src/**",
      decision: "ask",
      note: "remoto",
    };
    const draft = toDraft(rule);
    expect(draft.tools).toBe("git.push, git.reset");
    expect(fromDraft(draft)).toEqual(rule);
    expect(fromDraft({ ...draft, tools: " git.push  git.pull,, ", command: "  " })).toMatchObject({
      tools: ["git.push", "git.pull"],
      command: null,
    });
    expect(sameRules([rule], [{ ...rule, note: " remoto " }])).toBe(true);
    expect(sameRules([rule], [{ ...rule, decision: "deny" }])).toBe(false);
    expect(sameRules([rule], [])).toBe(false);
  });

  it("moves rules within bounds", () => {
    expect(moveItem(["a", "b", "c"], 2, -1)).toEqual(["a", "c", "b"]);
    expect(moveItem(["a", "b", "c"], 0, -1)).toEqual(["a", "b", "c"]);
    expect(moveItem(["a", "b", "c"], 0, 2)).toEqual(["b", "c", "a"]);
  });

  it("the chip says the mode, the pause and what is waiting", () => {
    expect(autonomyChip(null, 0).value).toBe("—");
    expect(autonomyChip(overview(), 0)).toMatchObject({ value: "Assistido", warn: false });
    const busy = autonomyChip(overview({ mode: "unrestricted", pausedAll: true }), 2);
    expect(busy.value).toBe("Acesso Irrestrito · pausado · 2 pedidos");
    expect(busy.warn).toBe(true);
    expect(autonomyChip(overview({ projectMode: "autonomous", mode: "autonomous" }), 1).hint).toContain(
      "escolhido para este projeto",
    );
    expect(autonomyChip(overview({ projectId: null }), 0).hint).toContain("nenhum projeto aberto");
  });

  it("names waits, grants, requesters and trial arguments", () => {
    const now = new Date("2026-09-30T12:00:00Z").getTime();
    expect(waitedFor("2026-09-30T11:59:48Z", now)).toBe("há 12 s");
    expect(waitedFor("2026-09-30T11:57:00Z", now)).toBe("há 3 min");
    expect(
      grantLabel({
        id: "g",
        sessionId: "s",
        tool: "shell.execute",
        mode: "assisted",
        rule: 5,
        command: "npm test",
        agentTitle: null,
        grantedAt: "",
      }),
    ).toBe("shell.execute `npm test` (regra 5 do Assistido)");
    expect(requester({ agentTitle: "Testes", projectName: "loja" })).toBe('Agente "Testes" · loja');
    expect(requester({ agentTitle: null, projectName: null })).toBe("Sessão");
    expect(trialArgs("shell.execute", " npm test ")).toEqual({ command: "npm test" });
    expect(trialArgs("terminal.write", "ls")).toEqual({ data: "ls" });
    expect(trialArgs("filesystem.write", "src/a.ts")).toEqual({ path: "src/a.ts" });
    expect(trialArgs("git.push", "")).toEqual({});
  });
});
