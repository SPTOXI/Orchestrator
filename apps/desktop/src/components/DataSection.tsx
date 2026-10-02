// "Dados e backups" (ADR-0022): an update never costs what was built with
// the version it replaces. Where the data lives, the backups the app makes
// on its own (before an update, a new version, a database migration) and
// the user's own, and restoring one.

import { useCallback, useEffect, useState } from "react";
import { backupMeta, isWarning } from "../lib/backups";
import { backupApi, errorMessage } from "../lib/runtime";
import type { BackupStatus } from "../lib/types";

interface Props {
  ready: boolean;
  active: boolean;
  /** Agents running or queued: they stop, with a handoff, before a restore. */
  liveAgents: number;
}

export function DataSection({ ready, active, liveAgents }: Props) {
  const [status, setStatus] = useState<BackupStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<{ id: string; action: "restore" | "remove" } | null>(null);

  const load = useCallback(async () => {
    try {
      setStatus(await backupApi.status());
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (ready && active) void load();
  }, [ready, active, load]);

  const create = async () => {
    setBusy("create");
    setError(null);
    setNotice(null);
    try {
      setStatus(await backupApi.create(label.trim() || null));
      setLabel("");
      setNotice("Backup feito.");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const remove = async (id: string) => {
    setConfirm(null);
    try {
      setStatus(await backupApi.remove(id));
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const restore = async (id: string) => {
    setConfirm(null);
    setBusy(id);
    setError(null);
    try {
      // The app restarts; nothing comes back on success.
      await backupApi.restore(id);
    } catch (e) {
      setError(errorMessage(e));
      setBusy(null);
    }
  };

  const backups = status?.backups ?? [];

  return (
    <div className="task-body data-section">
      {error && <div className="inline-error">{error}</div>}
      {notice && <div className="inline-notice ok">{notice}</div>}
      {status && status.notices.length > 0 && (
        <section>
          <h3>Nesta abertura</h3>
          {status.notices.map((text) => (
            <div key={text} className={`inline-notice${isWarning(text) ? " warn" : ""}`}>
              {text}
            </div>
          ))}
        </section>
      )}
      <section>
        <h3>Seus dados nas atualizações</h3>
        <p className="meta">
          Atualizar o Orchestrator não apaga nada do que você construiu: sessões e conversas, histórico, memória dos
          projetos, decisões, tasks, conexões de API, assinaturas, regras, skills, servidores MCP e configurações ficam
          na pasta de dados, que os instaladores não tocam. Os seus projetos nunca são alterados por uma atualização, e
          as chaves de API e os segredos ficam no cofre do sistema, que também continua.
        </p>
        <p className="meta">Mesmo assim, o Orchestrator guarda uma cópia de tudo:</p>
        <ul className="points meta">
          <li>antes de instalar uma atualização — sem o backup, a atualização não é instalada;</li>
          <li>ao abrir uma versão nova que você instalou por fora (instalador baixado);</li>
          <li>
            antes de atualizar o formato do banco de dados — sem a cópia, o banco não é alterado e o app abre sem
            mexer nele;
          </li>
          <li>quando você pede, abaixo.</li>
        </ul>
        <p className="meta">
          Configurações que uma versão não consegue ler não são apagadas: o arquivo original fica guardado ao lado, com
          o nome terminado em <span className="mono">.unreadable-…</span>, e o aviso aparece aqui. Os arquivos são
          gravados de um jeito que uma queda ou o fechamento para atualizar nunca deixa um pela metade.
        </p>
        {status && (
          <table className="kv-table">
            <tbody>
              <tr>
                <th>Pasta de dados</th>
                <td className="mono ellipsis" title={status.dataDir}>
                  {status.dataDir}
                </td>
              </tr>
              <tr>
                <th>Backups</th>
                <td className="mono ellipsis" title={status.backupsDir}>
                  {status.backupsDir}
                </td>
              </tr>
            </tbody>
          </table>
        )}
      </section>

      <section className="task-step">
        <h3>Backups</h3>
        <p className="meta">
          Os {status?.keepAutomatic ?? 5} backups automáticos mais recentes ficam guardados; os que você faz ficam até
          você apagar. As chaves e os segredos do cofre não entram nos backups (continuam no cofre).
        </p>
        <div className="row">
          <input
            className="grow"
            placeholder="Nome do backup (opcional), ex.: antes de mudar as regras"
            value={label}
            maxLength={120}
            onChange={(e) => setLabel(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && busy === null) void create();
            }}
          />
          <button className="button small primary" disabled={!ready || busy !== null} onClick={() => void create()}>
            {busy === "create" ? "Fazendo backup…" : "Fazer backup agora"}
          </button>
        </div>
        <ul className="list plain backup-list">
          {backups.map((backup) => (
            <li key={backup.id} className="backup-item">
              <div className="row">
                <div className="grow">
                  <div className="title">
                    <strong>{backup.label}</strong>
                  </div>
                  <div className="meta">{backupMeta(backup)}</div>
                </div>
                <button
                  className="button small"
                  disabled={!ready || busy !== null}
                  onClick={() => setConfirm({ id: backup.id, action: "restore" })}
                >
                  {busy === backup.id ? "Reiniciando…" : "Restaurar…"}
                </button>
                {backup.reason === "manual" && (
                  <button
                    className="link"
                    disabled={busy !== null}
                    onClick={() => setConfirm({ id: backup.id, action: "remove" })}
                  >
                    Apagar
                  </button>
                )}
              </div>
              {confirm?.id === backup.id && confirm.action === "restore" && (
                <div className="inline-notice">
                  <p>
                    Os dados voltam a ser os deste backup. O Orchestrator reinicia para trocar os arquivos; o estado de
                    agora vira um backup antes ("antes de uma restauração"), então dá para desfazer.
                    {liveAgents > 0 &&
                      ` ${liveAgents === 1 ? "O agente em execução para" : `Os ${liveAgents} agentes em execução param`} antes, com handoff.`}
                  </p>
                  <div className="row">
                    <button className="button small primary" onClick={() => void restore(backup.id)}>
                      Restaurar e reiniciar
                    </button>
                    <button className="button small" onClick={() => setConfirm(null)}>
                      Cancelar
                    </button>
                  </div>
                </div>
              )}
              {confirm?.id === backup.id && confirm.action === "remove" && (
                <div className="inline-notice">
                  <p>Apagar este backup? Não dá para desfazer.</p>
                  <div className="row">
                    <button className="button small danger" onClick={() => void remove(backup.id)}>
                      Apagar
                    </button>
                    <button className="button small" onClick={() => setConfirm(null)}>
                      Cancelar
                    </button>
                  </div>
                </div>
              )}
            </li>
          ))}
        </ul>
        {status && backups.length === 0 && (
          <div className="meta">
            Nenhum backup ainda. O primeiro automático é feito antes da próxima atualização.
          </div>
        )}
      </section>
    </div>
  );
}
