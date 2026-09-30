// Pull request tab (ADR-0017). With a number: the pull request, its CI,
// reviews and comments, a box to comment and the merge. Without: the form
// of a new pull request from the current branch.

import { useCallback, useEffect, useState } from "react";
import { formatTime } from "../lib/format";
import {
  CHECK_LABELS,
  MERGE_LABELS,
  PULL_STATE_LABELS,
  REVIEW_LABELS,
  fromTask,
  mergeHint,
  pushState,
  titleFromBranch,
} from "../lib/github";
import { errorMessage, gitApi, githubApi } from "../lib/runtime";
import type { GitHubStatus, MergeMethod, PullDetail, TaskView } from "../lib/types";

interface Props {
  active: boolean;
  ready: boolean;
  /** Project folder. */
  repo: string | undefined;
  number: number | null;
  nonce: number;
  tasks: TaskView[];
  onCreated: (number: number) => void;
  onChanged: () => void;
}

export function PullRequestView(props: Props) {
  return props.number === null ? <NewPull {...props} /> : <PullDetails {...props} number={props.number} />;
}

function NewPull({ active, ready, repo, nonce, tasks, onCreated, onChanged }: Props) {
  const [status, setStatus] = useState<GitHubStatus | null>(null);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [base, setBase] = useState("");
  const [draft, setDraft] = useState(false);
  const [taskId, setTaskId] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const next = await githubApi.status(repo);
      setStatus(next);
      setTitle((current) => current || titleFromBranch(next.branch));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [repo]);

  useEffect(() => {
    void load();
  }, [load, nonce]);

  const push = status ? pushState(status) : null;

  const run = async (label: string, work: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    try {
      await work();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const chooseTask = (id: string) => {
    setTaskId(id);
    const task = tasks.find((t) => t.id === id);
    if (!task) return;
    const filled = fromTask(task);
    setTitle(filled.title);
    setBody(filled.body);
  };

  const sendBranch = () =>
    run("push", async () => {
      await gitApi.push({
        path: repo,
        remote: status?.upstream ? undefined : (status?.remote ?? "origin"),
        branch: status?.upstream ? undefined : (status?.branch ?? undefined),
        setUpstream: !status?.upstream,
      });
      onChanged();
      await load();
    });

  const create = () =>
    run("create", async () => {
      const pull = await githubApi.createPull({
        path: repo,
        title: title.trim(),
        body,
        base: base.trim() || undefined,
        draft,
      });
      onChanged();
      onCreated(pull.number);
    });

  const offered = tasks.filter((t) => t.status !== "CANCELLED");

  return (
    <div className="editor pr-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">Novo pull request</span>
        <span className="meta grow ellipsis">
          {status?.repo ? `${status.repo.fullName} · ${status.branch ?? "?"} → ${base.trim() || status.repo.defaultBranch}` : ""}
        </span>
      </div>
      {error && <div className="inline-error">{error}</div>}
      {status && !status.authenticated && <div className="inline-notice">{status.accountError}</div>}
      {status?.repoError && <div className="inline-notice">{status.repoError}</div>}
      <div className="task-body">
        {push && !push.ready && (
          <div className="inline-notice row wrap">
            <span className="grow">{push.message}</span>
            {status?.branch && (
              <button className="button small" disabled={!ready || busy !== null} onClick={() => void sendBranch()}>
                {busy === "push" ? "Enviando…" : "Enviar (push)"}
              </button>
            )}
          </div>
        )}
        {offered.length > 0 && (
          <label className="field">
            <span>A partir de uma task (preenche título e descrição)</span>
            <select value={taskId} onChange={(e) => chooseTask(e.target.value)}>
              <option value="">—</option>
              {offered.map((task) => (
                <option key={task.id} value={task.id}>
                  {task.title}
                </option>
              ))}
            </select>
          </label>
        )}
        <label className="field">
          <span>Título</span>
          <input value={title} onChange={(e) => setTitle(e.target.value)} />
        </label>
        <label className="field">
          <span>Descrição</span>
          <textarea rows={10} value={body} onChange={(e) => setBody(e.target.value)} />
        </label>
        <div className="row wrap">
          <label className="field inline-field">
            <span>Base</span>
            <input
              className="mono"
              placeholder={status?.repo?.defaultBranch ?? "main"}
              value={base}
              onChange={(e) => setBase(e.target.value)}
            />
          </label>
          <label className="check">
            <input type="checkbox" checked={draft} onChange={(e) => setDraft(e.target.checked)} /> rascunho
          </label>
          <span className="grow" />
          <button
            className="button primary"
            disabled={!ready || busy !== null || !title.trim() || !push?.ready || !status?.repo}
            onClick={() => void create()}
          >
            {busy === "create" ? "Abrindo…" : "Abrir pull request"}
          </button>
        </div>
      </div>
    </div>
  );
}

function PullDetails({ active, ready, repo, number, nonce, onChanged }: Props & { number: number }) {
  const [pull, setPull] = useState<PullDetail | null>(null);
  const [comment, setComment] = useState("");
  const [method, setMethod] = useState<MergeMethod>("squash");
  const [deleteBranch, setDeleteBranch] = useState(true);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    setBusy((b) => b ?? "load");
    try {
      setPull(await githubApi.pull(number, repo));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy((b) => (b === "load" ? null : b));
    }
  }, [number, repo]);

  useEffect(() => {
    void load();
  }, [load, nonce]);

  const run = async (label: string, work: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    setNotice(null);
    try {
      await work();
      await load();
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const send = () =>
    run("comment", async () => {
      await githubApi.comment(number, comment, repo);
      setComment("");
    });

  const merge = () =>
    run("merge", async () => {
      setConfirming(false);
      const result = await githubApi.merge({ path: repo, number, method, deleteBranch });
      setNotice(
        `Merge feito (${result.sha.slice(0, 7)}).${
          result.branchDeleted ? " Branch apagada no GitHub." : result.branchError ? ` ${result.branchError}` : ""
        }`,
      );
    });

  const hint = pull ? mergeHint(pull.mergeableState, pull.mergeable) : null;

  return (
    <div className="editor pr-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">PR #{number}</span>
        {pull && (
          <span className="meta grow ellipsis">
            <span className={`pr-state ${pull.state}`}>{PULL_STATE_LABELS[pull.state]}</span>
            {pull.draft && " · rascunho"} · {pull.head} → {pull.base} · {pull.author}
          </span>
        )}
        {!pull && <span className="grow" />}
        <button className="button small" disabled={busy !== null} onClick={() => void load()}>
          {busy === "load" ? "…" : "Atualizar"}
        </button>
        {pull && (
          <button className="button small" onClick={() => void githubApi.openUrl(pull.url)}>
            Abrir no GitHub
          </button>
        )}
      </div>
      {error && <div className="inline-error">{error}</div>}
      {notice && <div className="inline-notice ok">{notice}</div>}
      {pull && (
        <div className="task-body">
          <section>
            <h2 className="pr-title">{pull.title}</h2>
            <div className="meta">
              {pull.commits} {pull.commits === 1 ? "commit" : "commits"} · {pull.changedFiles}{" "}
              {pull.changedFiles === 1 ? "arquivo" : "arquivos"} · +{pull.additions} −{pull.deletions} · aberto{" "}
              {formatTime(pull.createdAt)}
            </div>
            {pull.body.trim() ? <div className="pr-body">{pull.body}</div> : <p className="meta">Sem descrição.</p>}
          </section>

          <section className="task-step">
            <h3>
              <span className={`check-mark ${pull.checks.state}`}>{CHECK_LABELS[pull.checks.state]}</span>
              {pull.checks.total > 0 && (
                <span className="meta">
                  {" "}
                  {pull.checks.passed} ok · {pull.checks.failed} falharam · {pull.checks.pending} rodando
                </span>
              )}
            </h3>
            <ul className="plain-list">
              {pull.checks.items.map((item, i) => (
                <li key={`${item.kind}:${item.name}:${i}`} className="list-item static">
                  <span className={`check-mark ${item.state === "skipped" ? "none" : item.state}`}>
                    {item.state === "success" ? "✓" : item.state === "failure" ? "✗" : item.state === "pending" ? "●" : "–"}
                  </span>
                  <span className="grow ellipsis">
                    {item.name}
                    {item.description && <span className="meta"> — {item.description}</span>}
                  </span>
                  {item.url && (
                    <button className="link" onClick={() => void githubApi.openUrl(item.url!)}>
                      detalhes
                    </button>
                  )}
                </li>
              ))}
            </ul>
          </section>

          <section className="task-step">
            <h3>Revisões</h3>
            {pull.reviews.length === 0 && <p className="meta">Ninguém revisou ainda.</p>}
            <ul className="plain-list">
              {pull.reviews.map((review) => (
                <li key={review.author}>
                  <div>
                    <strong>{review.author}</strong>{" "}
                    <span className={`review-state ${review.state}`}>{REVIEW_LABELS[review.state]}</span>
                    {review.submittedAt && <span className="meta"> · {formatTime(review.submittedAt)}</span>}
                  </div>
                  {review.body && <div className="pr-comment">{review.body}</div>}
                </li>
              ))}
            </ul>
          </section>

          <section className="task-step">
            <h3>Comentários</h3>
            {pull.comments.length === 0 && <p className="meta">Nenhum comentário.</p>}
            <ul className="plain-list">
              {pull.comments.map((c, i) => (
                <li key={`${c.url}:${i}`}>
                  <div>
                    <strong>{c.author}</strong> <span className="meta">{formatTime(c.createdAt)}</span>
                  </div>
                  <div className="pr-comment">{c.body}</div>
                </li>
              ))}
            </ul>
            <textarea
              rows={3}
              placeholder="Comentar neste pull request"
              value={comment}
              onChange={(e) => setComment(e.target.value)}
            />
            <div className="row end">
              <button
                className="button small"
                disabled={!ready || busy !== null || !comment.trim()}
                onClick={() => void send()}
              >
                {busy === "comment" ? "Enviando…" : "Comentar"}
              </button>
            </div>
          </section>

          {pull.state === "open" && (
            <section className="task-step">
              <h3>Merge</h3>
              {hint && <p className="meta">{hint}</p>}
              <div className="row wrap">
                <select value={method} onChange={(e) => setMethod(e.target.value as MergeMethod)}>
                  {(Object.keys(MERGE_LABELS) as MergeMethod[]).map((m) => (
                    <option key={m} value={m}>
                      {MERGE_LABELS[m]}
                    </option>
                  ))}
                </select>
                <label className="check">
                  <input type="checkbox" checked={deleteBranch} onChange={(e) => setDeleteBranch(e.target.checked)} />{" "}
                  apagar a branch no GitHub
                </label>
                <span className="grow" />
                {confirming ? (
                  <>
                    <span className="meta">
                      {MERGE_LABELS[method]} de {pull.head} em {pull.base}?
                    </span>
                    <button className="button small primary" disabled={!ready || busy !== null} onClick={() => void merge()}>
                      {busy === "merge" ? "Fazendo merge…" : "Confirmar"}
                    </button>
                    <button className="button small" onClick={() => setConfirming(false)}>
                      Cancelar
                    </button>
                  </>
                ) : (
                  <button className="button small primary" disabled={!ready || busy !== null} onClick={() => setConfirming(true)}>
                    Fazer merge
                  </button>
                )}
              </div>
            </section>
          )}
        </div>
      )}
    </div>
  );
}
