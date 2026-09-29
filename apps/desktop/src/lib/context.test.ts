import { describe, expect, it } from "vitest";
import { emptyPacket, handoffStatusLabel, handoffTask, linesOf, PACKET_LISTS, summaryLine } from "./context";

describe("context helpers", () => {
  it("summarizes a context", () => {
    const section = { kind: "task" as const, title: "TASK", items: 1, tokens: 20 };
    expect(summaryLine({ tokens: 1234, budget: 1500, sections: [section, section], omitted: [] })).toBe(
      "~1.234 tokens de 1.500 · 2 seções",
    );
    expect(summaryLine({ tokens: 300, budget: 300, sections: [section], omitted: ["x"] })).toBe(
      "~300 tokens de 300 · 1 seção · 1 corte pelo orçamento",
    );
  });

  it("edits packet lists as lines", () => {
    expect(linesOf("- checkout\n\n  * criação de clientes \n• webhook\n   ")).toEqual([
      "checkout",
      "criação de clientes",
      "webhook",
    ]);
    expect(linesOf("")).toEqual([]);
    const packet = emptyPacket();
    expect(PACKET_LISTS.map((l) => l.key)).toEqual([
      "completed",
      "remaining",
      "files",
      "commands",
      "errors",
      "decisions",
      "tests",
    ]);
    expect(PACKET_LISTS.every((l) => Array.isArray(packet[l.key]))).toBe(true);
  });

  it("labels handoffs and builds their task", () => {
    expect(handoffStatusLabel({ status: "created" })).toBe("pendente");
    expect(handoffStatusLabel({ status: "accepted" })).toBe("assumido");
    expect(handoffTask({ goal: "Pagamentos Stripe", nextAction: "Validar a assinatura" })).toBe(
      "Pagamentos Stripe. Validar a assinatura",
    );
    expect(handoffTask({ goal: "Só o objetivo", nextAction: " " })).toBe("Só o objetivo");
  });
});
