// Bottom status bar.

import { formatUsd } from "../lib/format";
import type { AppInfo, BudgetView } from "../lib/types";

interface Props {
  ready: boolean;
  info: AppInfo | null;
  workspace: string;
  branch: string | null;
  /** What the open project's AIs spent today (ADR-0018). */
  spentToday: BudgetView | null;
  onOpenCost: () => void;
}

export function StatusBar({ ready, info, workspace, branch, spentToday, onOpenCost }: Props) {
  return (
    <footer className="status-bar">
      <span className={`status ${ready ? "ok" : "err"}`}>
        <span className={`dot ${ready ? "ok" : "err"}`} />
        {ready ? "Runtime conectado" : "Runtime indisponível"}
      </span>
      {branch && <span className="mono">⎇ {branch}</span>}
      <span className="mono ellipsis" title={workspace}>
        {workspace || "—"}
      </span>
      <span className="spacer" />
      {spentToday && (
        <button
          className={`status-link${spentToday.exhausted ? " err" : ""}`}
          title="Gasto das IAs neste projeto hoje — abre Tokens e custo"
          onClick={onOpenCost}
        >
          hoje: {formatUsd(spentToday.spentTodayUsd)}
          {spentToday.budgetUsd !== null && ` de ${formatUsd(spentToday.budgetUsd)}`}
          {spentToday.unpriced > 0 && " +?"}
        </button>
      )}
      {info?.databaseWarning && (
        <span className="status err" title={info.databaseWarning}>
          <span className="dot err" />
          Banco indisponível: nada será guardado ao fechar
        </span>
      )}
      {info && (
        <>
          <span title="Shell padrão">shell: {info.defaultShell}</span>
          <span>
            {info.os}/{info.arch}
          </span>
          <span title={`Banco: ${info.database}`}>v{info.version}</span>
        </>
      )}
    </footer>
  );
}
