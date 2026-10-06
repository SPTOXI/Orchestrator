// TERMINAL panel (sidebar): overview of terminals and managed processes.

import { baseName, formatTime } from "../lib/format";
import type { ProcessInfo, ShellList, TerminalInfo } from "../lib/types";

interface Props {
  terminals: TerminalInfo[];
  processes: ProcessInfo[];
  shells: ShellList | null;
  onOpen: (tab: "terminal" | "processes") => void;
}

export function SessionsPanel({ terminals, processes, shells, onOpen }: Props) {
  return (
    <div className="panel">
      <div className="panel-header">
        <span>Terminal</span>
      </div>
      <div className="section-title">Terminais ({terminals.length})</div>
      <ul className="list">
        {terminals.length === 0 && <li className="empty">Nenhum terminal aberto.</li>}
        {terminals.map((t) => (
          <li key={t.id} className="list-item" onClick={() => onOpen("terminal")}>
            <span className={`dot ${t.alive ? "ok" : "off"}`} />
            <div className="grow">
              <div className="title">{t.shell.name}</div>
              <div className="meta mono" title={t.cwd}>
                {baseName(t.cwd) || t.cwd} · pid {t.pid ?? "?"} · {t.cols}×{t.rows}
              </div>
            </div>
          </li>
        ))}
      </ul>
      <div className="section-title">Processos ({processes.filter((p) => p.status === "running").length} ativos)</div>
      <ul className="list">
        {processes.length === 0 && <li className="empty">Nenhum processo iniciado.</li>}
        {[...processes].reverse().map((p) => (
          <li key={p.id} className="list-item" onClick={() => onOpen("processes")}>
            <span className={`dot ${p.status === "running" ? "ok" : p.exitCode === 0 ? "off" : "err"}`} />
            <div className="grow">
              <div className="title mono">{p.name}</div>
              <div className="meta">
                {p.status} · {formatTime(p.startedAt)}
              </div>
            </div>
          </li>
        ))}
      </ul>
      <div className="section-title">Shells disponíveis</div>
      <ul className="list">
        {shells?.shells.map((s) => (
          <li key={s.id} className="list-item static" title={s.path}>
            <div className="grow">
              <div className="title">
                {s.name} {s.id === shells.default && <span className="badge">padrão</span>}
              </div>
              <div className="meta mono">{s.path}</div>
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}
