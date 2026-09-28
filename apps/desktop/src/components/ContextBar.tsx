// Top bar: the execution context that must always be visible (section 24).

import type { ProcessInfo, TerminalInfo } from "../lib/types";

interface Chip {
  label: string;
  value: string;
  hint: string;
  pending?: boolean;
}

interface Props {
  terminals: TerminalInfo[];
  processes: ProcessInfo[];
}

export function ContextBar({ terminals, processes }: Props) {
  const openTerminals = terminals.filter((t) => t.alive).length;
  const running = processes.filter((p) => p.status === "running").length;
  const chips: Chip[] = [
    { label: "Provider", value: "—", hint: "Providers de IA: Fases 3–5", pending: true },
    { label: "Task", value: "—", hint: "Task Manager: Fase 8", pending: true },
    { label: "Agent", value: "—", hint: "Agent Manager: Fase 8", pending: true },
    { label: "Autonomia", value: "—", hint: "Assistido / Autônomo / Acesso Irrestrito: Fase 9", pending: true },
    { label: "Branch", value: "—", hint: "Git: Fase 2", pending: true },
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
