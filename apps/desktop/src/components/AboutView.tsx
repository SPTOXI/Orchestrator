// "Sobre e atualizações" (ADR-0019): what this copy is, and its updates.
// The app looks for updates and tells; installing is always the user's.

import { useState } from "react";
import { githubApi } from "../lib/runtime";
import type { AppInfo } from "../lib/types";
import { bundleLabel, checkedText, progressText } from "../lib/updates";
import type { Updates } from "../lib/useUpdates";

interface Props {
  /** Opens "Dados e backups". */
  onOpenData?: () => void;
  active: boolean;
  ready: boolean;
  info: AppInfo | null;
  updates: Updates;
  /** Agents running or queued in the open project: they stop first. */
  liveAgents: number;
}

/** The releases page, from the manifest URL of a GitHub release. */
function releasesPage(endpoint: string | null): string | null {
  const match = endpoint?.match(/^(https:\/\/github\.com\/[^/]+\/[^/]+)\/releases\//);
  return match ? `${match[1]}/releases` : null;
}

export function AboutView({ active, ready, info, updates, liveAgents, onOpenData }: Props) {
  const { status, progress, busy, error, checked, backedUp } = updates;
  const [confirming, setConfirming] = useState(false);
  const available = status?.available ?? null;
  const releases = releasesPage(status?.endpoint ?? null);
  const packaged = status?.bundle === "deb" || status?.bundle === "rpm";

  const install = async () => {
    setConfirming(false);
    await updates.install();
  };

  return (
    <div className="editor about-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">Sobre e atualizações</span>
        <span className="meta grow ellipsis">Orchestrator {status?.version ?? info?.version ?? ""}</span>
      </div>
      {error && <div className="inline-error">{error}</div>}
      {status?.warning && <div className="inline-notice">{status.warning}</div>}
      <div className="task-body">
        <section>
          <h3>Este Orchestrator</h3>
          <table className="kv-table">
            <tbody>
              <tr>
                <th>Versão</th>
                <td>
                  {status?.version ?? info?.version ?? "…"}
                  {status?.commit && <span className="meta mono"> · {status.commit.slice(0, 10)}</span>}
                </td>
              </tr>
              <tr>
                <th>Sistema</th>
                <td>
                  {status?.os ?? info?.os}/{status?.arch ?? info?.arch}
                </td>
              </tr>
              <tr>
                <th>Instalação</th>
                <td>{bundleLabel(status?.bundle ?? null)}</td>
              </tr>
              {info && (
                <tr>
                  <th>Dados</th>
                  <td className="mono ellipsis" title={info.dataDir}>
                    {info.dataDir}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </section>

        <section className="task-step">
          <h3>Atualizações</h3>
          {status && !status.configured ? (
            <p className="meta">{status.notConfigured}</p>
          ) : (
            <>
              <div className="row">
                <button
                  className="button small"
                  disabled={!ready || busy !== null || status?.phase === "downloading" || status?.phase === "installed"}
                  onClick={() => void updates.check()}
                >
                  {busy === "check" || status?.phase === "checking" ? "Procurando…" : "Procurar atualizações"}
                </button>
                <span className="meta">Última procura: {checkedText(status?.lastCheck ?? null)}</span>
              </div>
              <label className="check">
                <input
                  type="checkbox"
                  checked={status?.autoCheck ?? true}
                  disabled={!ready || !status}
                  onChange={(e) => void updates.setAutoCheck(e.target.checked)}
                />{" "}
                Procurar sozinho ao abrir e a cada 6 horas (só avisa; instalar é sempre com você)
              </label>
              {status?.lastError && !error && <div className="inline-error">{status.lastError}</div>}

              {status?.phase === "installed" ? (
                <div className="inline-notice ok">
                  <p>
                    A versão {available?.version ?? ""} foi instalada. Ela abre quando o Orchestrator reiniciar com
                    tudo o que você já tinha: projetos, histórico, memória, tasks, conexões e configurações; as sessões
                    voltam encerradas, com a conversa ("Retomar" continua).
                    {backedUp && ` Antes de instalar foi feito o backup "${backedUp}".`}
                  </p>
                  <button className="button small primary" onClick={() => void updates.restart()}>
                    Reiniciar agora
                  </button>
                </div>
              ) : available ? (
                <div className="update-offer">
                  <p>
                    <strong>Versão {available.version} disponível</strong>
                    {available.date && (
                      <span className="meta"> · {new Date(available.date).toLocaleDateString("pt-BR")}</span>
                    )}
                    <span className="meta"> (você tem a {available.currentVersion})</span>
                  </p>
                  {available.notes && <pre className="update-notes">{available.notes}</pre>}
                  {status?.phase === "downloading" || busy === "install" ? (
                    <div className="update-progress">
                      <progress
                        max={progress?.total ?? undefined}
                        value={progress?.total ? progress.downloaded : undefined}
                      />
                      <span className="meta">
                        {progress && progress.downloaded > 0
                          ? `Baixando ${progressText(progress.downloaded, progress.total)}`
                          : backedUp
                            ? "Backup dos seus dados feito; baixando…"
                            : "Fazendo o backup dos seus dados…"}
                      </span>
                    </div>
                  ) : confirming ? (
                    <div className="inline-notice">
                      <p>
                        O Orchestrator faz um backup dos seus dados, baixa a versão {available.version}, confere a
                        assinatura e instala. Nada do que você construiu se perde: projetos, histórico, memória, tasks,
                        conexões, regras, skills e configurações continuam na versão nova.
                        {liveAgents > 0 &&
                          ` ${liveAgents === 1 ? "O agente em execução para" : `Os ${liveAgents} agentes em execução param`} antes, com handoff.`}
                        {status?.os === "windows" && " O app fecha e o instalador abre a versão nova."}
                        {packaged && " O sistema pode pedir a senha de administrador."}
                      </p>
                      <div className="row">
                        <button className="button small primary" disabled={!ready} onClick={() => void install()}>
                          Baixar e instalar
                        </button>
                        <button className="button small" onClick={() => setConfirming(false)}>
                          Cancelar
                        </button>
                      </div>
                    </div>
                  ) : (
                    <button
                      className="button small primary"
                      disabled={!ready || busy !== null}
                      onClick={() => setConfirming(true)}
                    >
                      Baixar e instalar…
                    </button>
                  )}
                </div>
              ) : (
                checked === null && <div className="inline-notice ok">Você está na versão mais recente.</div>
              )}
            </>
          )}
          <p className="meta">
            Antes de cada atualização é feito um backup de todos os seus dados.{" "}
            {onOpenData && (
              <button className="link" onClick={onOpenData}>
                Ver dados e backups
              </button>
            )}
          </p>
          {releases && (
            <p className="meta">
              Os instaladores de cada versão também ficam na{" "}
              <button className="link" onClick={() => void githubApi.openUrl(releases)}>
                página de releases
              </button>
              .
            </p>
          )}
        </section>
      </div>
    </div>
  );
}
