// Bottom status bar.

import type { AppInfo } from "../lib/types";

interface Props {
  ready: boolean;
  info: AppInfo | null;
  workspace: string;
  branch: string | null;
}

export function StatusBar({ ready, info, workspace, branch }: Props) {
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
