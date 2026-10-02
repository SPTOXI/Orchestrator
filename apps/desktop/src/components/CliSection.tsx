// "Assinaturas (CLI)" (ADR-0021): use the AI subscriptions the user
// already pays for — Claude Pro/Max, ChatGPT, a Google account — through
// their official CLIs instead of API keys. The CLI does the login; the
// Orchestrator's tools reach it over MCP, so every action still goes
// through the autonomy gate, the locks and the history.

import { useCallback, useEffect, useState } from "react";
import { cliApi, errorMessage } from "../lib/runtime";
import { terminalRequests } from "../lib/terminalRequests";
import type { CliSettings, CliStatus } from "../lib/types";

interface Props {
  ready: boolean;
  active: boolean;
  /** Providers changed (a CLI was turned on or off). */
  onChanged: () => void;
}

function Login({ status }: { status: CliStatus }) {
  if (!status.program) return <span className="badge">não instalado</span>;
  if (status.loggedIn === true) return <span className="badge ok">conectado</span>;
  if (status.loggedIn === false) return <span className="badge waiting">sem login</span>;
  return <span className="badge">login não verificado</span>;
}

function CliCard({ status, onSave }: { status: CliStatus; onSave: (s: CliSettings) => Promise<void> }) {
  const [models, setModels] = useState(status.settings.models.join(", "));
  const [program, setProgram] = useState(status.settings.program ?? "");
  const [extra, setExtra] = useState(status.settings.extraArgs.join(" "));
  const [open, setOpen] = useState(false);
  const s = status.settings;
  const save = (patch: Partial<CliSettings>) => void onSave({ ...s, ...patch });
  const offered = s.models.length > 0 ? s.models : status.suggestedModels;

  return (
    <li className="cli-card">
      <div className="row">
        <span className={`dot ${status.program && status.loggedIn !== false ? "ok" : "off"}`} />
        <div className="grow">
          <div className="title">
            <strong>{status.name}</strong> <Login status={status} />
          </div>
          <div className="meta">
            Assinatura: {status.subscription}
            {status.version && ` · versão ${status.version}`}
            {status.detail && ` · ${status.detail}`}
          </div>
        </div>
        <label className="check-line" title="Mostrar esta IA em AI Providers e nas sessões">
          <input
            type="checkbox"
            checked={s.enabled}
            disabled={!status.program}
            onChange={(e) => save({ enabled: e.target.checked })}
          />
          <span>Usar no Orchestrator</span>
        </label>
      </div>
      <div className="row cli-actions">
        {!status.program ? (
          <>
            <button className="button small primary" onClick={() => terminalRequests.run(status.installCommand)}>
              Instalar
            </button>
            <span className="meta mono">{status.installCommand}</span>
          </>
        ) : (
          <>
            <button
              className={`button small ${status.loggedIn === false ? "primary" : ""}`}
              title={status.loginHint}
              onClick={() => terminalRequests.run(status.loginCommand)}
            >
              {status.loggedIn ? "Entrar com outra conta" : "Entrar"}
            </button>
            <span className="meta">{status.loginHint}</span>
          </>
        )}
        <button className="link" onClick={() => setOpen(!open)}>
          {open ? "Fechar opções" : "Opções"}
        </button>
      </div>
      {!status.program && (
        <p className="meta">
          A instalação usa o npm, que vem com o Node.js (nodejs.org). Depois de instalar, clique em "Verificar de novo".
        </p>
      )}
      {open && (
        <div className="cli-options">
          <label className="form-row">
            <span>Modelos (separados por vírgula)</span>
            <input
              className="mono"
              placeholder={status.suggestedModels.join(", ") || "vazio: o padrão da CLI"}
              value={models}
              onChange={(e) => setModels(e.target.value)}
              onBlur={() =>
                save({
                  models: models
                    .split(",")
                    .map((m) => m.trim())
                    .filter(Boolean),
                })
              }
            />
          </label>
          {offered.length > 0 && (
            <label className="form-row">
              <span>Modelo padrão</span>
              <select value={s.defaultModel ?? ""} onChange={(e) => save({ defaultModel: e.target.value || null })}>
                <option value="">{offered[0]}</option>
                {offered.slice(1).map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            </label>
          )}
          <label className="form-row">
            <span>Caminho do programa</span>
            <input
              className="mono"
              placeholder={status.program ?? "(procurado no PATH)"}
              value={program}
              onChange={(e) => setProgram(e.target.value)}
              onBlur={() => save({ program: program.trim() || null })}
            />
          </label>
          <label className="form-row">
            <span>Argumentos extras</span>
            <input
              className="mono"
              placeholder="(opcional)"
              value={extra}
              onChange={(e) => setExtra(e.target.value)}
              onBlur={() => save({ extraArgs: extra.split(/\s+/).filter(Boolean) })}
            />
          </label>
          <label className="check-line">
            <input type="checkbox" checked={s.ownTools} onChange={(e) => save({ ownTools: e.target.checked })} />
            <span>
              Deixar a CLI usar também as ferramentas próprias dela (fora das regras de autonomia, das travas e do
              histórico do Orchestrator)
            </span>
          </label>
        </div>
      )}
    </li>
  );
}

export function CliSection({ ready, active, onChanged }: Props) {
  const [list, setList] = useState<CliStatus[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  const load = useCallback(async () => {
    setChecking(true);
    try {
      setList(await cliApi.list());
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    if (ready && active) void load();
  }, [ready, active, load]);

  return (
    <div className="task-body cli-section">
      <section>
        <h3>Assinaturas (CLI)</h3>
        <p className="meta">
          Use as assinaturas que você já paga, em vez de chaves de API: <strong>Claude Pro/Max</strong> pelo Claude
          Code, <strong>ChatGPT Plus/Pro</strong> pelo Codex e a <strong>conta Google</strong> pelo Gemini CLI. O login
          é feito pela própria CLI, no navegador, uma vez. Cada sessão roda a CLI oficial com as ferramentas do
          Orchestrator (arquivos, comandos, git, GitHub, internet, memória, skills e servidores MCP), entregues por MCP;
          as ferramentas próprias da CLI ficam desligadas, então tudo passa pelas regras de autonomia, pelas travas e
          pelo histórico, como nas APIs. O limite de uso é o da sua assinatura e não há custo por token.
        </p>
        <p className="meta">
          O Google Antigravity é um editor, sem uma CLI que outros programas possam usar; a conta Google funciona aqui
          pelo Gemini CLI.
        </p>
        <button className="button small" disabled={checking} onClick={() => void load()}>
          {checking ? "Verificando…" : "Verificar de novo"}
        </button>
      </section>
      {error && <div className="inline-error">{error}</div>}
      <section className="task-step">
        <ul className="list plain">
          {list?.map((status) => (
            <CliCard
              key={status.id}
              status={status}
              onSave={async (settings) => {
                try {
                  setList(await cliApi.save(status.kind, settings));
                  onChanged();
                } catch (e) {
                  setError(errorMessage(e));
                }
              }}
            />
          ))}
        </ul>
        {!list && !error && <div className="meta">verificando as CLIs…</div>}
      </section>
    </div>
  );
}
