// Bottom panel: real terminals (PTY) with tabs.

import type { Terminal } from "@xterm/xterm";
import { useCallback, useEffect, useRef, useState } from "react";
import { streamEvents } from "../lib/events";
import { baseName } from "../lib/format";
import { OutputSync } from "../lib/outputSync";
import { errorMessage, terminalApi } from "../lib/runtime";
import type { ShellList, TerminalInfo } from "../lib/types";
import { CloseIcon, PlusIcon } from "./icons";
import { XTermView } from "./XTermView";

function exitBanner(code: number | null): string {
  return `\r\n\x1b[2m[shell encerrado · código ${code ?? "?"}]\x1b[0m\r\n`;
}

/** Streams a terminal's buffered and live output into an xterm instance. */
function connectTerminal(id: string, term: Terminal): () => void {
  let disposed = false;
  const sync = new OutputSync((data) => term.write(data));
  const unsubscribe = streamEvents.subscribe((event) => {
    if (event.type === "terminalOutput" && event.terminalId === id) {
      sync.push(event.offset, event.data);
    } else if (event.type === "terminalExited" && event.terminalId === id && sync.position !== null) {
      term.write(exitBanner(event.exitCode));
    }
  });
  streamEvents
    .ready()
    .then(() => terminalApi.read(id))
    .then((snapshot) => {
      if (disposed) return;
      sync.snapshot(snapshot.data, snapshot.next);
      if (!snapshot.alive) term.write(exitBanner(snapshot.exitCode));
    })
    .catch((error) => {
      if (!disposed) term.write(`\r\n\x1b[31m[${errorMessage(error)}]\x1b[0m\r\n`);
    });
  return () => {
    disposed = true;
    unsubscribe();
  };
}

interface Props {
  ready: boolean;
  visible: boolean;
  terminals: TerminalInfo[];
  loaded: boolean;
  shells: ShellList | null;
  workspace: string;
}

export function TerminalPanel({ ready, visible, terminals, loaded, shells, workspace }: Props) {
  const [activeId, setActiveId] = useState<string | null>(null);
  const [shellChoice, setShellChoice] = useState<string>("");
  const [error, setError] = useState<string | null>(null);
  const autoCreated = useRef(false);
  // A terminal just created but not yet present in `terminals` (the shared
  // list refreshes asynchronously); keeps it selected meanwhile.
  const pendingId = useRef<string | null>(null);

  const create = useCallback(
    async (shell?: string) => {
      setError(null);
      try {
        const info = await terminalApi.create({ shell: shell || undefined, cwd: workspace || undefined });
        pendingId.current = info.id;
        setActiveId(info.id);
      } catch (e) {
        setError(errorMessage(e));
      }
    },
    [workspace],
  );

  // Open one terminal automatically the first time the app starts.
  useEffect(() => {
    if (!ready || !loaded || !workspace || autoCreated.current) return;
    autoCreated.current = true;
    if (terminals.length === 0) void create();
  }, [ready, loaded, workspace, terminals.length, create]);

  // Keep a valid active tab.
  useEffect(() => {
    if (activeId !== null && activeId === pendingId.current) {
      if (terminals.some((t) => t.id === activeId)) pendingId.current = null;
      return;
    }
    if (terminals.length === 0) {
      if (activeId !== null) setActiveId(null);
    } else if (!terminals.some((t) => t.id === activeId)) {
      setActiveId(terminals[terminals.length - 1]?.id ?? null);
    }
  }, [terminals, activeId]);

  const close = async (id: string) => {
    try {
      await terminalApi.close(id);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <div className="terminal-panel">
      <div className="subtabs">
        {terminals.map((t, index) => (
          <div
            key={t.id}
            className={`subtab ${t.id === activeId ? "active" : ""} ${t.alive ? "" : "exited"}`}
            onClick={() => setActiveId(t.id)}
            title={`${t.shell.name} — ${t.cwd}${t.pid ? ` (pid ${t.pid})` : ""}`}
          >
            <span className={`dot ${t.alive ? "ok" : "off"}`} />
            <span>
              {index + 1}: {t.shell.name} · {baseName(t.cwd) || t.cwd}
            </span>
            <button
              className="icon-button small"
              title="Fechar terminal"
              onClick={(e) => {
                e.stopPropagation();
                void close(t.id);
              }}
            >
              <CloseIcon />
            </button>
          </div>
        ))}
        <div className="subtabs-actions">
          <select
            value={shellChoice}
            onChange={(e) => setShellChoice(e.target.value)}
            title="Shell do novo terminal"
            disabled={!ready}
          >
            <option value="">Shell padrão{shells ? ` (${shells.default})` : ""}</option>
            {shells?.shells.map((s) => (
              <option key={s.id} value={s.id}>
                {s.name}
              </option>
            ))}
          </select>
          <button className="button" onClick={() => void create(shellChoice)} disabled={!ready}>
            <PlusIcon /> Novo terminal
          </button>
        </div>
      </div>
      {error && <div className="inline-error">{error}</div>}
      <div className="terminal-body">
        {terminals.length === 0 && (
          <div className="empty">{ready ? "Nenhum terminal aberto." : "Runtime indisponível."}</div>
        )}
        {terminals.map((t) => (
          <XTermView
            key={t.id}
            sourceKey={t.id}
            active={visible && t.id === activeId}
            connect={(term) => connectTerminal(t.id, term)}
            onInput={(data) => void terminalApi.input(t.id, data).catch(() => undefined)}
            onResize={(cols, rows) => void terminalApi.resize(t.id, cols, rows).catch(() => undefined)}
          />
        ))}
      </div>
    </div>
  );
}
