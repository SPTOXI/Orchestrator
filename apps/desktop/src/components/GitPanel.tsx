// GIT panel: branch, changes, commit, pull/push and recent commits.

import { type FormEvent, type KeyboardEvent, useCallback, useEffect, useState } from "react";
import { formatTime } from "../lib/format";
import { errorMessage, gitApi } from "../lib/runtime";
import type { ChangeKind, FileChange, GitBranch, GitCommit } from "../lib/types";
import type { GitState } from "../lib/useGitStatus";
import { PlusIcon, RefreshIcon } from "./icons";

const LETTER: Record<ChangeKind, string> = {
  modified: "M",
  added: "A",
  deleted: "D",
  renamed: "R",
  copied: "C",
  typeChanged: "T",
  untracked: "U",
  conflicted: "!",
};

const LABEL: Record<ChangeKind, string> = {
  modified: "modificado",
  added: "novo",
  deleted: "removido",
  renamed: "renomeado",
  copied: "copiado",
  typeChanged: "tipo alterado",
  untracked: "não rastreado",
  conflicted: "conflito",
};

interface Props {
  ready: boolean;
  projectPath: string | null;
  git: GitState;
  onOpenDiff: (file: string, staged: boolean) => void;
  onOpenFile: (absolutePath: string) => void;
}

function joinRepo(root: string, file: string): string {
  const sep = root.includes("\\") && !root.includes("/") ? "\\" : "/";
  return `${root.replace(/[\\/]+$/, "")}${sep}${sep === "\\" ? file.replace(/\//g, "\\") : file}`;
}

export function GitPanel({ ready, projectPath, git, onOpenDiff, onOpenFile }: Props) {
  const { status, notRepo, error: statusError, refresh } = git;
  const [message, setMessage] = useState("");
  const [amend, setAmend] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [branches, setBranches] = useState<GitBranch[]>([]);
  const [commits, setCommits] = useState<GitCommit[]>([]);
  const [newBranch, setNewBranch] = useState<string | null>(null);

  const repo = status?.root ?? projectPath ?? undefined;
  const head = status?.head ?? null;
  const branch = status?.branch ?? null;

  const loadHistory = useCallback(async () => {
    if (!repo) return;
    try {
      const [nextBranches, nextCommits] = await Promise.all([
        gitApi.branch({ path: repo }),
        gitApi.log({ path: repo, limit: 15 }),
      ]);
      setBranches(nextBranches);
      setCommits(nextCommits);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [repo]);

  // Branches and commits change with HEAD, not with every file edit.
  useEffect(() => {
    if (ready && status) void loadHistory();
  }, [ready, loadHistory, head, branch, status?.root]);

  const run = async (label: string, action: () => Promise<unknown>, success?: string) => {
    setBusy(label);
    setError(null);
    setNotice(null);
    try {
      await action();
      if (success) setNotice(success);
      refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  if (!ready) {
    return (
      <div className="panel">
        <div className="panel-header">Git</div>
        <div className="empty">Runtime indisponível.</div>
      </div>
    );
  }
  if (!projectPath) {
    return (
      <div className="panel">
        <div className="panel-header">Git</div>
        <div className="empty">Abra um projeto para ver o Git.</div>
      </div>
    );
  }
  if (notRepo) {
    return (
      <div className="panel">
        <div className="panel-header">Git</div>
        <div className="empty">Esta pasta não é um repositório Git.</div>
      </div>
    );
  }

  const files = status?.files ?? [];
  const staged = files.filter((f) => f.staged && !f.conflicted);
  const conflicts = files.filter((f) => f.conflicted);
  const changes = files.filter((f) => f.unstaged && !f.conflicted);
  const localBranches = branches.filter((b) => !b.remote);
  const firstRemote = status?.remotes[0]?.name;

  const commit = (e?: FormEvent) => {
    e?.preventDefault();
    if (!message.trim() && !amend) return;
    void run(
      "commit",
      async () => {
        const result = await gitApi.commit({ path: repo, message, amend });
        setMessage("");
        setAmend(false);
        setNotice(`Commit ${result.shortHash}: ${result.subject}`);
      },
    );
  };

  const onMessageKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") commit();
  };

  const push = () =>
    run(
      "push",
      () =>
        status?.upstream || !firstRemote || !branch
          ? gitApi.push({ path: repo })
          : gitApi.push({ path: repo, remote: firstRemote, branch, setUpstream: true }),
      "Push concluído.",
    );

  const fileRow = (file: FileChange, stagedList: boolean) => {
    const kind = (stagedList ? file.staged : file.unstaged) ?? "modified";
    const absolute = joinRepo(status?.root ?? projectPath, file.path);
    const openable = kind !== "deleted";
    return (
      <li
        key={`${stagedList ? "s" : "u"}:${file.path}`}
        className="list-item git-file"
        title={`${file.path} — ${LABEL[kind]}`}
        onClick={() => (kind === "untracked" ? onOpenFile(absolute) : onOpenDiff(file.path, stagedList))}
      >
        <span className={`git-letter kind-${kind}`}>{LETTER[kind]}</span>
        <span className="grow ellipsis mono">{file.path}</span>
        {openable && kind !== "untracked" && (
          <button
            className="icon-button small"
            title="Abrir arquivo"
            onClick={(e) => {
              e.stopPropagation();
              onOpenFile(absolute);
            }}
          >
            ↗
          </button>
        )}
        <button
          className="icon-button small"
          title={stagedList ? "Remover do stage" : "Adicionar ao stage"}
          disabled={busy !== null}
          onClick={(e) => {
            e.stopPropagation();
            void run(stagedList ? "unstage" : "stage", () =>
              stagedList
                ? gitApi.reset({ path: repo, files: [file.path] })
                : gitApi.add({ path: repo, files: [file.path] }),
            );
          }}
        >
          {stagedList ? "−" : "+"}
        </button>
      </li>
    );
  };

  return (
    <div className="panel">
      <div className="panel-header">
        <span>Git</span>
        <div className="row tight">
          <button className="icon-button" title="Atualizar" onClick={refresh}>
            <RefreshIcon />
          </button>
        </div>
      </div>

      <div className="stack pad">
        <div className="row">
          <select
            value={branch ?? ""}
            disabled={busy !== null || !status}
            title="Trocar de branch"
            onChange={(e) => {
              const target = e.target.value;
              if (target === "__new__") setNewBranch("");
              else if (target && target !== branch)
                void run("checkout", () => gitApi.checkout({ path: repo, target }), `Branch ${target}.`);
            }}
          >
            {!branch && <option value="">{status?.detached ? `HEAD ${head?.slice(0, 7)}` : "(sem commits)"}</option>}
            {localBranches.map((b) => (
              <option key={b.name} value={b.name}>
                {b.name}
              </option>
            ))}
            <option value="__new__">+ Nova branch…</option>
          </select>
        </div>
        {newBranch !== null && (
          <form
            className="row"
            onSubmit={(e) => {
              e.preventDefault();
              const name = newBranch.trim();
              if (!name) return;
              setNewBranch(null);
              void run("checkout", () => gitApi.checkout({ path: repo, target: name, create: true }), `Branch ${name} criada.`);
            }}
          >
            <input autoFocus className="mono" placeholder="nome-da-branch" value={newBranch} onChange={(e) => setNewBranch(e.target.value)} />
            <button className="button small primary" type="submit">
              Criar
            </button>
            <button className="button small" type="button" onClick={() => setNewBranch(null)}>
              ✕
            </button>
          </form>
        )}
        <div className="meta">
          {status?.upstream ? (
            <>
              {status.upstream} · ↑{status.ahead} ↓{status.behind}
            </>
          ) : (
            "sem upstream"
          )}
          {status?.remotes[0] && (
            <div className="ellipsis mono" title={status.remotes[0].url}>
              {status.remotes[0].name}: {status.remotes[0].url}
            </div>
          )}
        </div>
        <div className="row tight">
          <button className="button small" disabled={busy !== null || !status?.upstream} onClick={() => void run("pull", () => gitApi.pull({ path: repo }), "Pull concluído.")}>
            ↓ Pull
          </button>
          <button className="button small" disabled={busy !== null || (!status?.upstream && !firstRemote)} onClick={() => void push()}>
            ↑ Push
          </button>
          <button
            className="button small"
            disabled={busy !== null || files.length === 0}
            title="Guardar alterações (stash)"
            onClick={() => void run("stash", () => gitApi.stash({ path: repo, action: "push", includeUntracked: true }), "Alterações guardadas no stash.")}
          >
            Stash
          </button>
          {busy && <span className="meta">{busy}…</span>}
        </div>
      </div>

      <form className="stack pad commit-box" onSubmit={commit}>
        <textarea
          className="mono"
          placeholder={`Mensagem do commit (Ctrl+Enter) — ${staged.length} no stage`}
          value={message}
          onChange={(e) => setMessage(e.target.value)}
          onKeyDown={onMessageKey}
          rows={3}
        />
        <div className="row">
          <label className="check">
            <input type="checkbox" checked={amend} onChange={(e) => setAmend(e.target.checked)} /> amend
          </label>
          <span className="spacer" />
          <button className="button small primary" type="submit" disabled={busy !== null || (!amend && (staged.length === 0 || !message.trim()))}>
            Commit
          </button>
        </div>
      </form>

      {statusError && <div className="inline-error">{statusError}</div>}
      {error && <div className="inline-error">{error}</div>}
      {notice && <div className="inline-notice ok">{notice}</div>}

      <div className="scroll">
        {conflicts.length > 0 && (
          <>
            <div className="section-title">Conflitos ({conflicts.length})</div>
            <ul className="list">{conflicts.map((f) => fileRow({ ...f, unstaged: "conflicted" }, false))}</ul>
          </>
        )}
        <div className="section-title row">
          <span className="grow">Staged ({staged.length})</span>
          {staged.length > 0 && (
            <button className="link" onClick={() => void run("unstage", () => gitApi.reset({ path: repo, files: staged.map((f) => f.path) }))}>
              remover todos
            </button>
          )}
        </div>
        <ul className="list">{staged.map((f) => fileRow(f, true))}</ul>
        <div className="section-title row">
          <span className="grow">Alterações ({changes.length})</span>
          {changes.length > 0 && (
            <button className="link" onClick={() => void run("stage", () => gitApi.add({ path: repo, all: true }))}>
              <PlusIcon /> adicionar todos
            </button>
          )}
        </div>
        <ul className="list">
          {changes.map((f) => fileRow(f, false))}
          {status?.clean && <li className="empty">Nada a commitar.</li>}
        </ul>
        <div className="section-title">Últimos commits</div>
        <ul className="list">
          {commits.length === 0 && <li className="empty">Nenhum commit.</li>}
          {commits.map((c) => (
            <li key={c.hash} className="list-item static commit" title={`${c.hash}\n${c.author} <${c.email}>`}>
              <span className="mono meta">{c.shortHash}</span>
              <div className="grow">
                <div className="ellipsis">{c.subject}</div>
                <div className="meta">
                  {c.author} · {formatTime(c.date)}
                </div>
              </div>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
