import { describe, expect, it } from "vitest";
import { fromTask, mergeHint, pushState, titleFromBranch, tokenSourceLabel } from "./github";

describe("github", () => {
  it("names where the token comes from", () => {
    expect(tokenSourceLabel(null)).toBe("nenhum token");
    expect(tokenSourceLabel("vault")).toBe("token salvo no cofre do sistema");
    expect(tokenSourceLabel("env:GH_TOKEN")).toBe("variável de ambiente GH_TOKEN");
    expect(tokenSourceLabel("gh")).toContain("gh auth login");
  });

  it("says what stands before a pull request", () => {
    expect(pushState({ branch: null, upstream: null, ahead: 0 }).ready).toBe(false);
    expect(pushState({ branch: "feat", upstream: null, ahead: 0 }).message).toContain("ainda não está no GitHub");
    expect(pushState({ branch: "feat", upstream: "origin/feat", ahead: 2 }).message).toContain("2 commits");
    expect(pushState({ branch: "feat", upstream: "origin/feat", ahead: 1 }).message).toContain("1 commit não");
    expect(pushState({ branch: "feat", upstream: "origin/feat", ahead: 0 })).toEqual({ ready: true, message: null });
  });

  it("explains why a merge may not happen", () => {
    expect(mergeHint("clean", true)).toBeNull();
    expect(mergeHint(null, null)).toContain("calculando");
    expect(mergeHint("dirty", false)).toContain("conflito");
    expect(mergeHint("blocked", false)).toContain("proteção");
    expect(mergeHint("unknown", false)).toContain("não permite");
  });

  it("suggests titles and descriptions", () => {
    expect(titleFromBranch("feat/retentativas-no_gateway")).toBe("Retentativas no gateway");
    expect(titleFromBranch("main")).toBe("Main");
    expect(titleFromBranch(null)).toBe("");
    const pr = fromTask({
      title: " Aplicar retentativas ",
      description: "Três tentativas com espera.",
      result: "Feito e testado.",
      files: ["src/gateway.ts"],
    });
    expect(pr.title).toBe("Aplicar retentativas");
    expect(pr.body).toBe(
      "Três tentativas com espera.\n\n## Resultado\n\nFeito e testado.\n\n## Arquivos\n\n- `src/gateway.ts`",
    );
    expect(fromTask({ title: "x", description: "", result: "", files: [] }).body).toBe("");
  });
});
