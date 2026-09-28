// PROJECT panel: open project summary, open/discover actions, recent
// projects and the project's file tree.

import { type FormEvent, useState } from "react";
import type { RecentProject } from "../lib/recent";
import type { GitStatusWithRemotes, ProjectProfile } from "../lib/types";
import { Explorer } from "./Explorer";
import { CloseIcon, FolderIcon } from "./icons";

interface Props {
  ready: boolean;
  profile: ProjectProfile | null;
  gitStatus: GitStatusWithRemotes | null;
  recent: RecentProject[];
  opening: boolean;
  onPickFolder: () => void;
  onOpenProject: (path: string) => void;
  onRemoveRecent: (path: string) => void;
  onShowProfile: () => void;
  onShowDiscovery: () => void;
  onOpenFile: (path: string) => void;
}

export function RecentList({
  recent,
  onOpenProject,
  onRemoveRecent,
}: Pick<Props, "recent" | "onOpenProject" | "onRemoveRecent">) {
  if (recent.length === 0) return null;
  return (
    <ul className="list">
      {recent.map((p) => (
        <li key={p.path} className="list-item" onClick={() => onOpenProject(p.path)} title={p.path}>
          <FolderIcon />
          <div className="grow">
            <div className="title">{p.name}</div>
            <div className="meta mono ellipsis">{p.path}</div>
          </div>
          <button
            className="icon-button small"
            title="Remover dos recentes"
            onClick={(e) => {
              e.stopPropagation();
              onRemoveRecent(p.path);
            }}
          >
            <CloseIcon />
          </button>
        </li>
      ))}
    </ul>
  );
}

export function ProjectPanel(props: Props) {
  const { ready, profile, gitStatus, recent, opening, onPickFolder, onOpenProject, onShowProfile, onShowDiscovery, onOpenFile } =
    props;
  const [pathInput, setPathInput] = useState("");

  const submitPath = (e: FormEvent) => {
    e.preventDefault();
    const path = pathInput.trim();
    if (path) {
      onOpenProject(path);
      setPathInput("");
    }
  };

  const branch = gitStatus?.branch ?? (gitStatus?.detached ? `HEAD ${gitStatus.head?.slice(0, 7)}` : null);
  const stack = profile ? [...profile.languages, ...profile.frameworks].slice(0, 5) : [];

  return (
    <div className="panel">
      <div className="panel-header">
        <span>Project</span>
        <div className="row tight">
          <button className="button small" disabled={!ready || opening} onClick={onPickFolder}>
            Abrir pasta…
          </button>
          <button className="button small" disabled={!ready} onClick={onShowDiscovery}>
            Procurar
          </button>
        </div>
      </div>

      {profile ? (
        <div className="project-card" onClick={onShowProfile} title="Ver PROJECT PROFILE">
          <div className="row">
            <strong className="grow ellipsis">{profile.name}</strong>
            {branch && <span className="badge mono">⎇ {branch}</span>}
          </div>
          <div className="meta mono ellipsis" title={profile.path}>
            {profile.path}
          </div>
          {stack.length > 0 && (
            <div className="chip-list">
              {stack.map((s) => (
                <span key={s} className="tag small">
                  {s}
                </span>
              ))}
              {profile.packageManagers[0] && <span className="tag small">{profile.packageManagers[0]}</span>}
            </div>
          )}
          <div className="meta link-like">Ver perfil completo →</div>
        </div>
      ) : (
        <div className="stack pad">
          <div className="meta">{ready ? "Nenhum projeto aberto." : "Runtime indisponível."}</div>
        </div>
      )}

      <form className="path-form" onSubmit={submitPath}>
        <input
          className="mono"
          value={pathInput}
          onChange={(e) => setPathInput(e.target.value)}
          placeholder="Abrir caminho (ex.: C:\Projetos\MeuSaaS)"
          disabled={!ready || opening}
        />
      </form>

      {profile ? (
        <Explorer ready={ready} workspace={profile.path} onOpenFile={onOpenFile} />
      ) : (
        <div className="scroll">
          {props.recent.length > 0 && <div className="section-title">Recentes</div>}
          <RecentList recent={recent} onOpenProject={onOpenProject} onRemoveRecent={props.onRemoveRecent} />
        </div>
      )}
    </div>
  );
}
