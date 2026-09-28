// Bottom panel: one-off commands through shell.execute.

import { type FormEvent, useState } from "react";
import { formatDuration, formatTime, stripAnsi } from "../lib/format";
import { errorMessage, shellApi } from "../lib/runtime";
import type { ExecuteOutput, ShellList } from "../lib/types";
import { PlayIcon } from "./icons";

interface Run {
  id: number;
  command: string;
  startedAt: string;
  result?: ExecuteOutput;
  error?: string;
}

interface Props {
  ready: boolean;
  shells: ShellList | null;
  workspace: string;
}

let nextRunId = 1;

function exitBadge(run: Run) {
  if (run.error) return <span className="badge err">erro</span>;
  if (!run.result) return <span className="badge">executando…</span>;
  if (run.result.timedOut) return <span className="badge warn">timeout</span>;
  const code = run.result.exitCode;
  return <span className={`badge ${code === 0 ? "ok" : "err"}`}>exit {code ?? "sinal"}</span>;
}

export function CommandPanel({ ready, shells, workspace }: Props) {
  const [command, setCommand] = useState("");
  const [shell, setShell] = useState("");
  const [cwd, setCwd] = useState("");
  const [timeoutSec, setTimeoutSec] = useState("600");
  const [runs, setRuns] = useState<Run[]>([]);

  const update = (id: number, patch: Partial<Run>) =>
    setRuns((all) => all.map((r) => (r.id === id ? { ...r, ...patch } : r)));

  const run = async (e: FormEvent) => {
    e.preventDefault();
    if (!command.trim()) return;
    const id = nextRunId++;
    setRuns((all) => [{ id, command, startedAt: new Date().toISOString() }, ...all].slice(0, 50));
    const seconds = Number(timeoutSec);
    try {
      const result = await shellApi.execute({
        command,
        shell: shell || undefined,
        cwd: cwd || workspace || undefined,
        timeoutMs: Number.isFinite(seconds) && seconds > 0 ? seconds * 1000 : undefined,
      });
      update(id, { result });
    } catch (err) {
      update(id, { error: errorMessage(err) });
    }
  };

  return (
    <div className="command-panel">
      <form className="row command-form" onSubmit={run}>
        <input
          className="mono grow"
          placeholder="Comando não interativo (ex.: npm test, git status, dir)"
          value={command}
          onChange={(e) => setCommand(e.target.value)}
          disabled={!ready}
        />
        <select value={shell} onChange={(e) => setShell(e.target.value)} disabled={!ready} title="Shell">
          <option value="">Shell padrão</option>
          {shells?.shells.map((s) => (
            <option key={s.id} value={s.id}>
              {s.name}
            </option>
          ))}
        </select>
        <input
          className="mono cwd-input"
          placeholder={`cwd: ${workspace || "workspace"}`}
          value={cwd}
          onChange={(e) => setCwd(e.target.value)}
          disabled={!ready}
        />
        <label className="timeout" title="Timeout em segundos (vazio = sem timeout)">
          timeout
          <input value={timeoutSec} onChange={(e) => setTimeoutSec(e.target.value)} disabled={!ready} />s
        </label>
        <button className="button primary" type="submit" disabled={!ready || !command.trim()}>
          <PlayIcon /> Executar
        </button>
      </form>
      <div className="runs">
        {runs.length === 0 && (
          <div className="empty">
            Executa comandos até o fim e mostra stdout, stderr e exit code. Para processos longos use a aba Processos.
          </div>
        )}
        {runs.map((r) => (
          <div key={r.id} className="run">
            <div className="run-header">
              <span className="mono grow">$ {r.command}</span>
              {exitBadge(r)}
              {r.result && <span className="meta">{formatDuration(r.result.durationMs)}</span>}
              <span className="meta">{formatTime(r.startedAt)}</span>
            </div>
            {r.result && <div className="meta mono">{r.result.shell} · {r.result.cwd}</div>}
            {r.error && <pre className="output err">{r.error}</pre>}
            {r.result?.stdout && (
              <pre className="output">
                {stripAnsi(r.result.stdout)}
                {r.result.stdoutTruncated && "\n[stdout truncado]"}
              </pre>
            )}
            {r.result?.stderr && (
              <pre className="output err">
                {stripAnsi(r.result.stderr)}
                {r.result.stderrTruncated && "\n[stderr truncado]"}
              </pre>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
