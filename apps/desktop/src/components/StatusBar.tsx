// Bottom status bar.

import { dataChip } from "../lib/backups";
import { formatUsd } from "../lib/format";
import type { AppInfo, BudgetView } from "../lib/types";
import { Elapsed } from "./Activity";

/** An AI at work (lib/activity.ts). */
export interface Working {
  sessionId: string;
  title: string;
  label: string;
  since: number;
}

interface Props {
  /** The AIs working now, the oldest first. */
  working?: Working[];
  onOpenSession?: (sessionId: string) => void;
  ready: boolean;
  info: AppInfo | null;
  workspace: string;
  branch: string | null;
  /** What the open project's AIs spent today (ADR-0018). */
  spentToday: BudgetView | null;
  onOpenCost: () => void;
  /** Something to do about updates ("Atualização 0.2.0"), if any (ADR-0019). */
  update: string | null;
  onOpenAbout: () => void;
  /** Opens "Dados e backups" (ADR-0022). */
  onOpenData: () => void;
}

export function StatusBar({
  ready,
  info,
  workspace,
  branch,
  spentToday,
  onOpenCost,
  update,
  onOpenAbout,
  onOpenData,
  working = [],
  onOpenSession,
}: Props) {
  const first = working[0];
  const data = dataChip(info?.dataNotices ?? []);
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
      {first && (
        <button
          className="status-link working"
          title={working.map((w) => `${w.title}: ${w.label}`).join("\n") + "\n— abre a sessão"}
          onClick={() => onOpenSession?.(first.sessionId)}
        >
          <span className="spinner small" aria-hidden />
          {working.length === 1 ? (
            <>
              {first.title}: {first.label} · <Elapsed since={first.since} />
            </>
          ) : (
            <>{working.length} IAs trabalhando</>
          )}
        </button>
      )}
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
      {update && (
        <button className="status-link update" title="Abre Sobre e atualizações" onClick={onOpenAbout}>
          ⬆ {update}
        </button>
      )}
      {data && (
        <button
          className={`status-link${data.warn ? " err" : ""}`}
          title={`${(info?.dataNotices ?? []).join("\n")}\n— abre Dados e backups`}
          onClick={onOpenData}
        >
          {data.text}
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
          <button
            className="status-link"
            title={`Banco: ${info.database} — abre Sobre e atualizações`}
            onClick={onOpenAbout}
          >
            v{info.version}
          </button>
        </>
      )}
    </footer>
  );
}
