// GitHub tab (ADR-0017): the token (saved in the OS vault, never shown
// again), where the token in use comes from, the account, and the server
// for GitHub Enterprise.

import { useCallback, useEffect, useState } from "react";
import { tokenSourceLabel } from "../lib/github";
import { errorMessage, githubApi } from "../lib/runtime";
import type { GitHubSetup, GitHubStatus } from "../lib/types";

interface Props {
  active: boolean;
  ready: boolean;
  repo: string | undefined;
  onChanged: () => void;
}

const TOKENS_URL = "https://github.com/settings/personal-access-tokens/new";

export function GitHubView({ active, ready, repo, onChanged }: Props) {
  const [setup, setSetup] = useState<GitHubSetup | null>(null);
  const [status, setStatus] = useState<GitHubStatus | null>(null);
  const [token, setToken] = useState("");
  const [host, setHost] = useState("");
  const [apiUrl, setApiUrl] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [nextSetup, nextStatus] = await Promise.all([githubApi.setup(), githubApi.status(repo)]);
      setSetup(nextSetup);
      setStatus(nextStatus);
      setHost(nextSetup.host);
      setApiUrl(nextSetup.apiUrl ?? "");
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [repo]);

  useEffect(() => {
    if (active) void load();
  }, [active, load]);

  const run = async (label: string, work: () => Promise<GitHubSetup>, done: string) => {
    setBusy(label);
    setError(null);
    setNotice(null);
    try {
      setSetup(await work());
      const nextStatus = await githubApi.status(repo);
      setStatus(nextStatus);
      if (nextStatus.authenticated) setNotice(`${done} Conectado como @${nextStatus.account?.login}.`);
      else if (label === "token") setError(`${done} Mas: ${nextStatus.accountError ?? "o GitHub não aceitou."}`);
      else setNotice(done);
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const saveToken = () =>
    run(
      "token",
      async () => {
        const next = await githubApi.saveToken(token);
        setToken("");
        return next;
      },
      "Token salvo no cofre.",
    );

  return (
    <div className="editor github-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">GitHub</span>
        <span className="meta grow ellipsis">
          {status?.authenticated
            ? `Conectado como @${status.account?.login} · ${tokenSourceLabel(status.tokenSource)}`
            : "Não conectado"}
        </span>
      </div>
      {error && <div className="inline-error">{error}</div>}
      {notice && <div className="inline-notice ok">{notice}</div>}
      {setup?.warning && <div className="inline-notice">{setup.warning}</div>}
      <div className="task-body">
        <section>
          <h3>Conta</h3>
          {status?.authenticated ? (
            <p>
              <strong>@{status.account?.login}</strong>
              {status.account?.name && ` (${status.account.name})`} — {tokenSourceLabel(status.tokenSource)}.
              {status.account && status.account.scopes.length > 0 && (
                <span className="meta"> Escopos: {status.account.scopes.join(", ")}.</span>
              )}
            </p>
          ) : (
            <p className="meta">{status?.accountError ?? "…"}</p>
          )}
          {status?.repo && (
            <p className="meta">
              Repositório deste projeto: {status.repo.fullName}
              {status.remote && ` (remoto ${status.remote})`}
              {status.repo.private ? " · privado" : " · público"}.
            </p>
          )}
          {status?.repoError && <p className="meta">{status.repoError}</p>}
        </section>

        <section className="task-step">
          <h3>Token</h3>
          <p className="meta">
            O Orchestrator usa, nesta ordem: o token salvo aqui (no {setup?.vault ?? "cofre do sistema"}), as
            variáveis <code>GH_TOKEN</code>/<code>GITHUB_TOKEN</code> e o GitHub CLI (<code>gh auth login</code>). O
            token nunca aparece de novo na tela, no histórico ou em arquivos.
          </p>
          <p className="meta">
            Crie um token <em>fine-grained</em> com acesso ao repositório e permissões de leitura e escrita em{" "}
            <strong>Contents</strong>, <strong>Pull requests</strong> e <strong>Issues</strong>, e de leitura em{" "}
            <strong>Checks</strong> e <strong>Commit statuses</strong> — ou um token clássico com <code>repo</code>.{" "}
            <button className="link" onClick={() => void githubApi.openUrl(TOKENS_URL)}>
              Criar token no GitHub
            </button>
          </p>
          <div className="row">
            <input
              type="password"
              className="mono grow"
              autoComplete="off"
              placeholder={setup?.vaultToken ? "Um token já está salvo — cole outro para trocar" : "github_pat_… ou ghp_…"}
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
            <button
              className="button small primary"
              disabled={!ready || busy !== null || !token.trim()}
              onClick={() => void saveToken()}
            >
              {busy === "token" ? "Salvando…" : "Salvar no cofre"}
            </button>
            {setup?.vaultToken && (
              <button
                className="button small danger"
                disabled={!ready || busy !== null}
                onClick={() => void run("clear", githubApi.clearToken, "Token removido do cofre.")}
              >
                Remover
              </button>
            )}
          </div>
          <ul className="plain-list meta">
            <li>Cofre: {setup?.vaultToken ? "token salvo" : "vazio"}</li>
            <li>Ambiente: {setup?.envToken ? `${setup.envToken} definida` : "GH_TOKEN e GITHUB_TOKEN vazias"}</li>
            <li>GitHub CLI: {setup?.ghInstalled ? "instalado" : "não encontrado"}</li>
          </ul>
        </section>

        <section className="task-step">
          <h3>Servidor</h3>
          <p className="meta">
            Para GitHub Enterprise, informe o host dos remotos e, se for diferente de{" "}
            <code>https://HOST/api/v3</code>, a URL da API. API em uso: <code>{setup?.apiBase}</code>
          </p>
          <div className="row wrap">
            <label className="field inline-field">
              <span>Host</span>
              <input className="mono" value={host} placeholder="github.com" onChange={(e) => setHost(e.target.value)} />
            </label>
            <label className="field inline-field grow">
              <span>API</span>
              <input
                className="mono"
                value={apiUrl}
                placeholder="(automática)"
                onChange={(e) => setApiUrl(e.target.value)}
              />
            </label>
            <button
              className="button small"
              disabled={!ready || busy !== null}
              onClick={() =>
                void run(
                  "settings",
                  () => githubApi.saveSettings({ host: host.trim() || "github.com", apiUrl: apiUrl.trim() || null }),
                  "Servidor salvo.",
                )
              }
            >
              Salvar
            </button>
          </div>
        </section>
      </div>
    </div>
  );
}
