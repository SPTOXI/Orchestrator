// Bottom panel: long-running processes (process.start / stop / list / read).

import type { Terminal } from "@xterm/xterm";
import { type FormEvent, useEffect, useState } from "react";
import { streamEvents } from "../lib/events";
import { formatTime } from "../lib/format";
import { OutputSync } from "../lib/outputSync";
import { errorMessage, processApi } from "../lib/runtime";
import type { ProcessInfo, ShellList } from "../lib/types";
import { PlayIcon, StopIcon } from "./icons";
import { XTermView } from "./XTermView";

function connectProcess(id: string, term: Terminal): () => void {
  let disposed = false;
  const sync = new OutputSync((data) => term.write(data));
  const unsubscribe = streamEvents.subscribe((event) => {
    if (event.type === "processOutput" && event.processId === id) {
      const rendered = event.stream === "stderr" ? `\x1b[31m${event.data}\x1b[0m` : event.data;
      sync.push(event.offset, event.data, rendered);
    } else if (event.type === "processExited" && event.processId === id && sync.position !== null) {
      term.write(
        `\n\x1b[2m[${event.stopped ? "encerrado pelo usuário" : "processo terminou"} · código ${event.exitCode ?? "?"}]\x1b[0m\n`,
      );
    }
  });
  streamEvents
    .ready()
    .then(() => processApi.read(id))
    .then((snapshot) => {
      if (disposed) return;
      if (snapshot.truncated) term.write("\x1b[2m[saída anterior descartada]\x1b[0m\n");
      sync.snapshot(snapshot.data, snapshot.next);
    })
    .catch((error) => {
      if (!disposed) term.write(`\n\x1b[31m[${errorMessage(error)}]\x1b[0m\n`);
    });
  return () => {
    disposed = true;
    unsubscribe();
  };
}

function statusLabel(p: ProcessInfo): string {
  switch (p.status) {
    case "running":
      return "em execução";
    case "stopped":
      return `encerrado (${p.exitCode ?? "sinal"})`;
    case "exited":
      return `terminou (${p.exitCode ?? "?"})`;
  }
}

interface Props {
  ready: boolean;
  visible: boolean;
  processes: ProcessInfo[];
  shells: ShellList | null;
  workspace: string;
}

export function ProcessesPanel({ ready, visible, processes, shells, workspace }: Props) {
  const [command, setCommand] = useState("");
  const [name, setName] = useState("");
  const [cwd, setCwd] = useState("");
  const [shell, setShell] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!selected && processes.length > 0) setSelected(processes[processes.length - 1]?.id ?? null);
  }, [processes, selected]);

  const start = async (e: FormEvent) => {
    e.preventDefault();
    if (!command.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const info = await processApi.start({
        command,
        name: name || undefined,
        cwd: cwd || workspace || undefined,
        shell: shell || undefined,
      });
      setSelected(info.id);
      setCommand("");
      setName("");
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const stop = async (id: string, force: boolean) => {
    setError(null);
    try {
      await processApi.stop(id, force);
    } catch (err) {
      setError(errorMessage(err));
    }
  };

  const current = processes.find((p) => p.id === selected) ?? null;

  return (
    <div className="split-panel">
      <div className="split-left">
        <form className="stack" onSubmit={start}>
          <input
            className="mono"
            placeholder="Comando (ex.: npm run dev)"
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            disabled={!ready}
          />
          <div className="row">
            <input placeholder="Nome (opcional)" value={name} onChange={(e) => setName(e.target.value)} disabled={!ready} />
            <select value={shell} onChange={(e) => setShell(e.target.value)} disabled={!ready} title="Shell">
              <option value="">Shell padrão</option>
              {shells?.shells.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </select>
          </div>
          <input
            className="mono"
            placeholder={`Diretório (padrão: ${workspace || "workspace"})`}
            value={cwd}
            onChange={(e) => setCwd(e.target.value)}
            disabled={!ready}
          />
          <button className="button primary" type="submit" disabled={!ready || busy || !command.trim()}>
            <PlayIcon /> Iniciar processo
          </button>
        </form>
        {error && <div className="inline-error">{error}</div>}
        <ul className="list">
          {processes.length === 0 && <li className="empty">Nenhum processo iniciado.</li>}
          {[...processes].reverse().map((p) => (
            <li
              key={p.id}
              className={`list-item ${p.id === selected ? "selected" : ""}`}
              onClick={() => setSelected(p.id)}
            >
              <span className={`dot ${p.status === "running" ? "ok" : p.exitCode === 0 ? "off" : "err"}`} />
              <div className="grow">
                <div className="title mono">{p.name}</div>
                <div className="meta">
                  {statusLabel(p)} · pid {p.pid ?? "?"} · {formatTime(p.startedAt)}
                </div>
              </div>
              {p.status === "running" && (
                <div className="row tight">
                  <button
                    className="button small"
                    title="Encerrar (SIGTERM; força após 5 s)"
                    onClick={(e) => {
                      e.stopPropagation();
                      void stop(p.id, false);
                    }}
                  >
                    <StopIcon /> Parar
                  </button>
                  <button
                    className="button small danger"
                    title="Matar imediatamente a árvore de processos"
                    onClick={(e) => {
                      e.stopPropagation();
                      void stop(p.id, true);
                    }}
                  >
                    Matar
                  </button>
                </div>
              )}
            </li>
          ))}
        </ul>
      </div>
      <div className="split-right">
        {current ? (
          <>
            <div className="output-header mono" title={current.cwd}>
              $ {current.command} <span className="meta">— {current.cwd}</span>
            </div>
            <div className="terminal-body">
              <XTermView
                key={current.id}
                sourceKey={current.id}
                active={visible}
                readOnly
                connect={(term) => connectProcess(current.id, term)}
              />
            </div>
          </>
        ) : (
          <div className="empty">Selecione um processo para ver a saída.</div>
        )}
      </div>
    </div>
  );
}
