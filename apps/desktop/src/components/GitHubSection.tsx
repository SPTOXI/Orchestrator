// GitHub section of the GIT panel (ADR-0017): account and repository, the
// pull request of the current branch with its CI, "Criar pull request",
// and the open pull requests and issues. It asks GitHub when it opens, when
// the branch moves and when the user presses "Atualizar"; nothing polls.

import { useCallback, useEffect, useState } from "react";
import { CHECK_LABELS, CHECK_MARKS, PULL_STATE_LABELS, pushState } from "../lib/github";
import { errorMessage, githubApi } from "../lib/runtime";
import type { GitHubStatus, IssueSummary, PullSummary } from "../lib/types";
import { RefreshIcon } from "./icons";

interface Props {
  repo: string | undefined;
  /** Changes when the branch, its upstream or what is left to push change. */
  branchKey: string;
  onOpenPull: (number: number | null) => void;
  onOpenSetup: () => void;
}

export function GitHubSection({ repo, branchKey, onOpenPull, onOpenSetup }: Props) {
  const [status, setStatus] = useState<GitHubStatus | null>(null);
  const [pulls, setPulls] = useState<PullSummary[]>([]);
  const [issues, setIssues] = useState<IssueSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const next = await githubApi.status(repo);
      setStatus(next);
      if (next.authenticated && next.repo) {
        const [openPulls, openIssues] = await Promise.all([
          githubApi.pulls({ path: repo, limit: 10 }),
          githubApi.issues({ path: repo, limit: 10 }),
        ]);
        setPulls(openPulls.pulls);
        setIssues(openIssues.issues);
      } else {
        setPulls([]);
        setIssues([]);
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }, [repo]);

  useEffect(() => {
    void load();
  }, [load, branchKey]);

  const push = status ? pushState(status) : null;
  const checks = status?.checks ?? null;

  return (
    <section className="github-section">
      <div className="section-title row">
        <span className="grow">GitHub</span>
        <button className="icon-button small" title="Atualizar" disabled={loading} onClick={() => void load()}>
          <RefreshIcon />
        </button>
      </div>
      <div className="pad stack">
        {error && <div className="inline-error">{error}</div>}
        {!status && !error && <div className="meta">{loading ? "Consultando o GitHub…" : ""}</div>}
        {status && !status.authenticated && (
          <>
            <div className="meta">{status.accountError}</div>
            <button className="button small primary" onClick={onOpenSetup}>
              Conectar ao GitHub
            </button>
          </>
        )}
        {status?.authenticated && (
          <>
            <div className="row tight">
              <span className="grow ellipsis" title={status.repo?.url ?? status.repoError ?? ""}>
                {status.repo ? (
                  <button className="link" onClick={() => void githubApi.openUrl(status.repo!.url)}>
                    {status.repo.fullName}
                  </button>
                ) : (
                  <span className="meta">sem repositório</span>
                )}
              </span>
              <button className="link meta" title="Conta e token" onClick={onOpenSetup}>
                @{status.account?.login}
              </button>
            </div>
            {status.repoError && <div className="meta">{status.repoError}</div>}
            {status.repo && status.pull && (
              <button
                className={`github-pull current check-${checks?.state ?? "none"}`}
                onClick={() => onOpenPull(status.pull!.number)}
                title={checks ? `${CHECK_LABELS[checks.state]} (${checks.passed}/${checks.total})` : undefined}
              >
                <span className="mono">#{status.pull.number}</span>
                <span className="grow ellipsis">{status.pull.title}</span>
                {status.pull.state !== "open" ? (
                  <span className={`pr-state ${status.pull.state}`}>{PULL_STATE_LABELS[status.pull.state]}</span>
                ) : (
                  checks && <span className={`check-mark ${checks.state}`}>{CHECK_MARKS[checks.state]}</span>
                )}
              </button>
            )}
            {status.repo &&
              status.pull?.state !== "open" &&
              status.branch &&
              status.branch !== status.repo.defaultBranch && (
              <>
                <button className="button small primary" onClick={() => onOpenPull(null)}>
                  Criar pull request
                </button>
                {push && !push.ready && <div className="meta">{push.message}</div>}
              </>
            )}
          </>
        )}
      </div>
      {status?.authenticated && status.repo && (
        <>
          <div className="section-title">Pull requests abertos ({pulls.length})</div>
          <ul className="list">
            {pulls.length === 0 && <li className="empty">Nenhum.</li>}
            {pulls.map((pull) => (
              <li
                key={pull.number}
                className="list-item"
                title={`${pull.head} → ${pull.base} · ${pull.author}`}
                onClick={() => onOpenPull(pull.number)}
              >
                <span className="mono meta">#{pull.number}</span>
                <span className="grow ellipsis">
                  {pull.title}
                  {pull.draft && <span className="meta"> · rascunho</span>}
                </span>
              </li>
            ))}
          </ul>
          <div className="section-title">Issues abertas ({issues.length})</div>
          <ul className="list">
            {issues.length === 0 && <li className="empty">Nenhuma.</li>}
            {issues.map((issue) => (
              <li
                key={issue.number}
                className="list-item"
                title={`${issue.author} · ${issue.commentCount} comentários — abrir no GitHub`}
                onClick={() => void githubApi.openUrl(issue.url)}
              >
                <span className="mono meta">#{issue.number}</span>
                <span className="grow ellipsis">{issue.title}</span>
                {issue.labels.slice(0, 2).map((label) => (
                  <span key={label} className="badge">
                    {label}
                  </span>
                ))}
              </li>
            ))}
          </ul>
        </>
      )}
    </section>
  );
}
