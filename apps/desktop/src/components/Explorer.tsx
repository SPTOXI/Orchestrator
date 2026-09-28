// PROJECT panel: workspace file tree (filesystem.list / write / move / delete).

import { type FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { auditEvents } from "../lib/events";
import { baseName, joinPath, parentPath } from "../lib/format";
import { errorMessage, fsApi } from "../lib/runtime";
import type { DirEntryInfo } from "../lib/types";
import { ChevronIcon, EditIcon, FileIcon, PlusIcon, RefreshIcon, TrashIcon, UpIcon } from "./icons";

interface DirState {
  entries?: DirEntryInfo[];
  error?: string;
  loading: boolean;
}

type Dialog =
  | { mode: "new"; dir: string }
  | { mode: "rename"; entry: DirEntryInfo }
  | { mode: "delete"; entry: DirEntryInfo };

interface Props {
  ready: boolean;
  workspace: string;
  onWorkspaceChange: (path: string) => void;
  onOpenFile: (path: string) => void;
}

export function Explorer({ ready, workspace, onWorkspaceChange, onOpenFile }: Props) {
  const [dirs, setDirs] = useState<Record<string, DirState>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [pathInput, setPathInput] = useState(workspace);
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [dialogValue, setDialogValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const visibleDirs = useRef<string[]>([]);

  const load = useCallback(async (dir: string) => {
    setDirs((d) => ({ ...d, [dir]: { ...d[dir], loading: true } }));
    try {
      const out = await fsApi.list(dir);
      setDirs((d) => ({ ...d, [dir]: { entries: out.entries, loading: false } }));
    } catch (e) {
      setDirs((d) => ({ ...d, [dir]: { error: errorMessage(e), loading: false } }));
    }
  }, []);

  useEffect(() => {
    setPathInput(workspace);
    setExpanded(new Set());
    setDirs({});
    if (ready && workspace) void load(workspace);
  }, [ready, workspace, load]);

  visibleDirs.current = [workspace, ...expanded];

  // Reload visible directories when any file changes (UI or, later, agents).
  useEffect(() => {
    let timer: number | undefined;
    const unsubscribe = auditEvents.subscribe((event) => {
      if (event.kind !== "FILE_CHANGED") return;
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        for (const dir of visibleDirs.current) if (dir) void load(dir);
      }, 150);
    });
    return () => {
      window.clearTimeout(timer);
      unsubscribe();
    };
  }, [load]);

  const toggle = (dir: string) => {
    const opening = !expanded.has(dir);
    setExpanded((prev) => {
      const next = new Set(prev);
      if (opening) next.add(dir);
      else next.delete(dir);
      return next;
    });
    if (opening && !dirs[dir]?.entries) void load(dir);
  };

  const openDialog = (next: Dialog) => {
    setError(null);
    setDialog(next);
    setDialogValue(next.mode === "rename" ? next.entry.name : "");
  };

  const submitDialog = async (e: FormEvent) => {
    e.preventDefault();
    if (!dialog) return;
    setError(null);
    try {
      if (dialog.mode === "new") {
        if (!dialogValue.trim()) return;
        const path = joinPath(dialog.dir, dialogValue.trim());
        await fsApi.write(path, "", { createDirs: true });
        onOpenFile(path);
      } else if (dialog.mode === "rename") {
        if (!dialogValue.trim() || dialogValue === dialog.entry.name) return setDialog(null);
        await fsApi.move(dialog.entry.path, joinPath(parentPath(dialog.entry.path), dialogValue.trim()));
      } else {
        await fsApi.remove(dialog.entry.path, dialog.entry.kind === "directory");
      }
      setDialog(null);
    } catch (err) {
      setError(errorMessage(err));
    }
  };

  const renderEntries = (dir: string, depth: number) => {
    const state = dirs[dir];
    if (!state || (state.loading && !state.entries)) {
      return <div className="tree-note" style={{ paddingLeft: 12 + depth * 14 }}>carregando…</div>;
    }
    if (state.error) {
      return <div className="tree-note err" style={{ paddingLeft: 12 + depth * 14 }}>{state.error}</div>;
    }
    if (state.entries?.length === 0) {
      return <div className="tree-note" style={{ paddingLeft: 12 + depth * 14 }}>(vazio)</div>;
    }
    return state.entries?.map((entry) => {
      const isDir = entry.kind === "directory";
      const open = isDir && expanded.has(entry.path);
      return (
        <div key={entry.path}>
          <div
            className={`tree-row ${entry.name.startsWith(".") ? "hidden-entry" : ""}`}
            style={{ paddingLeft: 8 + depth * 14 }}
            onClick={() => (isDir ? toggle(entry.path) : onOpenFile(entry.path))}
            title={entry.path}
          >
            <span className="tree-icon">{isDir ? <ChevronIcon open={open} /> : <FileIcon />}</span>
            <span className="tree-name">
              {entry.name}
              {entry.isSymlink && <span className="meta"> ↪</span>}
            </span>
            <span className="tree-actions">
              {isDir && (
                <button
                  className="icon-button small"
                  title="Novo arquivo aqui"
                  onClick={(e) => {
                    e.stopPropagation();
                    openDialog({ mode: "new", dir: entry.path });
                  }}
                >
                  <PlusIcon />
                </button>
              )}
              <button
                className="icon-button small"
                title="Renomear"
                onClick={(e) => {
                  e.stopPropagation();
                  openDialog({ mode: "rename", entry });
                }}
              >
                <EditIcon />
              </button>
              <button
                className="icon-button small"
                title="Excluir"
                onClick={(e) => {
                  e.stopPropagation();
                  openDialog({ mode: "delete", entry });
                }}
              >
                <TrashIcon />
              </button>
            </span>
          </div>
          {open && renderEntries(entry.path, depth + 1)}
        </div>
      );
    });
  };

  return (
    <div className="panel">
      <div className="panel-header">
        <span>Workspace</span>
        <div className="row tight">
          <button
            className="icon-button"
            title="Pasta acima"
            disabled={!ready}
            onClick={() => onWorkspaceChange(parentPath(workspace))}
          >
            <UpIcon />
          </button>
          <button
            className="icon-button"
            title="Novo arquivo na raiz"
            disabled={!ready}
            onClick={() => openDialog({ mode: "new", dir: workspace })}
          >
            <PlusIcon />
          </button>
          <button className="icon-button" title="Recarregar" disabled={!ready} onClick={() => void load(workspace)}>
            <RefreshIcon />
          </button>
        </div>
      </div>
      <form
        className="path-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (pathInput.trim()) onWorkspaceChange(pathInput.trim());
        }}
      >
        <input
          className="mono"
          value={pathInput}
          onChange={(e) => setPathInput(e.target.value)}
          placeholder="Caminho da pasta"
          title="Digite um caminho e pressione Enter"
          disabled={!ready}
        />
      </form>
      {dialog && (
        <form className="dialog" onSubmit={submitDialog}>
          {dialog.mode === "delete" ? (
            <div>
              Excluir <strong>{dialog.entry.name}</strong>
              {dialog.entry.kind === "directory" ? " e todo o seu conteúdo" : ""}?
            </div>
          ) : (
            <>
              <div className="meta">
                {dialog.mode === "new" ? `Novo arquivo em ${baseName(dialog.dir) || dialog.dir}` : "Novo nome"}
              </div>
              <input
                autoFocus
                className="mono"
                value={dialogValue}
                onChange={(e) => setDialogValue(e.target.value)}
                placeholder={dialog.mode === "new" ? "nome.ext ou sub/pasta/nome.ext" : ""}
              />
            </>
          )}
          <div className="row tight end">
            <button type="button" className="button small" onClick={() => setDialog(null)}>
              Cancelar
            </button>
            <button type="submit" className={`button small ${dialog.mode === "delete" ? "danger" : "primary"}`}>
              {dialog.mode === "delete" ? "Excluir" : "OK"}
            </button>
          </div>
        </form>
      )}
      {error && <div className="inline-error">{error}</div>}
      <div className="tree">{ready ? renderEntries(workspace, 0) : <div className="tree-note">Runtime indisponível.</div>}</div>
    </div>
  );
}
