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
      {info && (
        <>
          <span title="Shell padrão">shell: {info.defaultShell}</span>
          <span>
            {info.os}/{info.arch}
          </span>
          <span title={`Auditoria: ${info.auditLog}`}>v{info.version}</span>
        </>
      )}
    </footer>
  );
}
