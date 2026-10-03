// PROJECT panel: the projects open side by side (ADR-0023), the open
// project's summary and the projects related to it, open/discover actions,
// recent projects and the project's file tree.

import { type FormEvent, useState } from "react";
import type { RecentProject } from "../lib/recent";
import type { GitStatusWithRemotes, Project, ProjectLink, ProjectProfile } from "../lib/types";
import { runningIn, samePath } from "../lib/workspace";
import { Explorer } from "./Explorer";
import { CloseIcon, EditIcon, FolderIcon, PlusIcon } from "./icons";

interface Props {
  ready: boolean;
  profile: ProjectProfile | null;
  gitStatus: GitStatusWithRemotes | null;
  recent: RecentProject[];
  opening: boolean;
  /** Projects open side by side, in sidebar order. */
  openProjects: Project[];
  /** Sessions with a turn running, per project folder. */
  running: ReadonlyMap<string, number>;
  /** Projects related to the open one, and the ones that can be. */
  links: ProjectLink[];
  linkCandidates: Project[];
  linkError: string | null;
  onPickFolder: () => void;
  onOpenProject: (path: string) => void;
  onCloseProject: (project: Project) => void;
  onRemoveRecent: (path: string) => void;
  onShowProfile: () => void;
  onShowDiscovery: () => void;
  onOpenFile: (path: string) => void;
  onLink: (other: string, note: string) => Promise<boolean>;
  onUnlink: (other: string) => void;
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

/** The projects open side by side: one click switches, AIs keep working in
 * the others. */
function OpenProjects({
  projects,
  active,
  running,
  opening,
  onOpen,
  onClose,
}: {
  projects: Project[];
  active: string | null;
  running: ReadonlyMap<string, number>;
  opening: boolean;
  onOpen: (path: string) => void;
  onClose: (project: Project) => void;
}) {
  if (projects.length === 0) return null;
  return (
    <>
      <div className="section-title">Abertos ({projects.length})</div>
      <ul className="list open-projects">
        {projects.map((project) => {
          const current = samePath(project.path, active);
          const working = runningIn(running, project.path);
          return (
            <li
              key={project.id}
              className={`list-item open-project ${current ? "selected" : ""}`}
              title={project.path}
              aria-current={current ? "true" : undefined}
              onClick={() => !current && !opening && onOpen(project.path)}
            >
              <FolderIcon />
              <span className="grow title">{project.name}</span>
              {working > 0 && (
                <span
                  className="open-project-work"
                  title={`${working === 1 ? "1 IA trabalhando" : `${working} IAs trabalhando`} neste projeto`}
                >
                  <span className="spinner small" />
                  {working}
                </span>
              )}
              <button
                className="icon-button small"
                title={
                  working > 0
                    ? "Fechar na barra lateral (as IAs deste projeto continuam trabalhando)"
                    : "Fechar na barra lateral (nada do projeto é apagado)"
                }
                aria-label={`Fechar ${project.name}`}
                onClick={(e) => {
                  e.stopPropagation();
                  onClose(project);
                }}
              >
                <CloseIcon />
              </button>
            </li>
          );
        })}
      </ul>
    </>
  );
}

/** Projects that work with this one: their AIs consult each other. */
function RelatedProjects({
  links,
  candidates,
  error,
  onOpen,
  onLink,
  onUnlink,
}: {
  links: ProjectLink[];
  candidates: Project[];
  error: string | null;
  onOpen: (path: string) => void;
  onLink: (other: string, note: string) => Promise<boolean>;
  onUnlink: (other: string) => void;
}) {
  /** `null`: closed; `""`: a new link; an id: editing that link's note. */
  const [editing, setEditing] = useState<string | null>(null);
  const [other, setOther] = useState("");
  const [note, setNote] = useState("");
  const [saving, setSaving] = useState(false);

  const start = (link?: ProjectLink) => {
    setEditing(link ? link.project.id : "");
    setOther(link ? link.project.id : (candidates[0]?.id ?? ""));
    setNote(link?.note ?? "");
  };

  const save = async (e: FormEvent) => {
    e.preventDefault();
    if (!other) return;
    setSaving(true);
    const ok = await onLink(other, note);
    setSaving(false);
    if (ok) setEditing(null);
  };

  const form = (
    <form className="related-form" onSubmit={(e) => void save(e)}>
      {editing === "" ? (
        candidates.length > 0 ? (
          <select value={other} onChange={(e) => setOther(e.target.value)} aria-label="Projeto relacionado">
            {candidates.map((project) => (
              <option key={project.id} value={project.id}>
                {project.name} — {project.path}
              </option>
            ))}
          </select>
        ) : (
          <div className="meta">
            Abra o outro projeto antes (Abrir pasta…): só projetos já abertos alguma vez podem ser relacionados.
          </div>
        )
      ) : null}
      <input
        value={note}
        maxLength={500}
        autoFocus={editing !== ""}
        placeholder="Como se relacionam? ex.: o app consome a API deste projeto"
        onChange={(e) => setNote(e.target.value)}
      />
      <div className="row tight">
        <button className="button small primary" type="submit" disabled={saving || !other}>
          {editing === "" ? "Relacionar" : "Salvar"}
        </button>
        <button className="button small" type="button" onClick={() => setEditing(null)}>
          Cancelar
        </button>
      </div>
    </form>
  );

  return (
    <div className="related">
      <div className="section-title row">
        <span className="grow">Relacionados</span>
        {editing === null && (
          <button
            className="icon-button small"
            title="Relacionar a outro projeto: as IAs de um consultam as do outro"
            aria-label="Relacionar projeto"
            onClick={() => start()}
          >
            <PlusIcon />
          </button>
        )}
      </div>
      {error && <div className="inline-error small">{error}</div>}
      {links.length === 0 && editing === null && (
        <div className="meta related-empty">
          Projetos que trabalham juntos (uma API e o app que a usa, por exemplo): relacionados, a IA de um pergunta
          à IA do outro e deixa tasks nele.{" "}
          <button className="link" onClick={() => start()}>
            Relacionar…
          </button>
        </div>
      )}
      {editing === "" && form}
      <ul className="list">
        {links.map((link) => (
          <li key={link.project.id} className="related-item">
            <div className="list-item" title={link.project.path} onClick={() => onOpen(link.project.path)}>
              <FolderIcon />
              <div className="grow">
                <div className="title">{link.project.name}</div>
                {link.note && <div className="meta related-note">{link.note}</div>}
              </div>
              <button
                className="icon-button small"
                title="Mudar a descrição da relação"
                aria-label={`Editar a relação com ${link.project.name}`}
                onClick={(e) => {
                  e.stopPropagation();
                  start(link);
                }}
              >
                <EditIcon />
              </button>
              <button
                className="icon-button small"
                title="Desfazer a relação (nada dos projetos é apagado)"
                aria-label={`Desfazer a relação com ${link.project.name}`}
                onClick={(e) => {
                  e.stopPropagation();
                  onUnlink(link.project.id);
                }}
              >
                <CloseIcon />
              </button>
            </div>
            {editing === link.project.id && form}
          </li>
        ))}
      </ul>
    </div>
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
          <button
            className="button small"
            disabled={!ready || opening}
            onClick={onPickFolder}
            title="Abrir mais um projeto: os abertos ficam na lista, um clique troca entre eles"
          >
            Abrir pasta…
          </button>
          <button className="button small" disabled={!ready} onClick={onShowDiscovery}>
            Procurar
          </button>
        </div>
      </div>

      <OpenProjects
        projects={props.openProjects}
        active={profile?.path ?? null}
        running={props.running}
        opening={opening}
        onOpen={onOpenProject}
        onClose={props.onCloseProject}
      />

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

      {profile && (
        <RelatedProjects
          key={`related:${profile.path}`}
          links={props.links}
          candidates={props.linkCandidates}
          error={props.linkError}
          onOpen={onOpenProject}
          onLink={props.onLink}
          onUnlink={props.onUnlink}
        />
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
        <Explorer key={`files:${profile.path}`} ready={ready} workspace={profile.path} onOpenFile={onOpenFile} />
      ) : (
        <div className="scroll">
          {props.recent.length > 0 && <div className="section-title">Recentes</div>}
          <RecentList recent={recent} onOpenProject={onOpenProject} onRemoveRecent={props.onRemoveRecent} />
        </div>
      )}
    </div>
  );
}
