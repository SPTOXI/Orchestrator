// Main area: PROJECT PROFILE (project.profile) plus installed runtimes.

import { type ReactNode, useEffect, useState } from "react";
import { formatTime, joinPath } from "../lib/format";
import { errorMessage, packageApi, runtimeInfoApi } from "../lib/runtime";
import type { DockerRuntime, NodeRuntime, ProjectProfile, PythonRuntime } from "../lib/types";
import { PlayIcon, RefreshIcon } from "./icons";

interface Props {
  profile: ProjectProfile | null;
  active: boolean;
  onRefresh: () => void;
  onOpenFile: (path: string) => void;
  onProcessStarted: () => void;
}

function count(n: number, singular: string, plural: string): string {
  return `${n} ${n === 1 ? singular : plural}`;
}

function Chips({ items, empty = "—" }: { items: string[]; empty?: string }) {
  if (items.length === 0) return <span className="meta">{empty}</span>;
  return (
    <span className="chip-list">
      {items.map((item) => (
        <span key={item} className="tag">
          {item}
        </span>
      ))}
    </span>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="profile-row">
      <div className="profile-label">{label}</div>
      <div className="profile-value">{children}</div>
    </div>
  );
}

interface Installed {
  node?: NodeRuntime;
  python?: PythonRuntime;
  docker?: DockerRuntime;
}

export function ProfileView({ profile, active, onRefresh, onOpenFile, onProcessStarted }: Props) {
  const [installed, setInstalled] = useState<Installed | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // Messages and runtime checks belong to the project they were made for.
  const projectPath = profile?.path;
  useEffect(() => {
    setInstalled(null);
    setError(null);
    setNotice(null);
  }, [projectPath]);

  if (!profile) {
    return (
      <div className="editor" hidden={!active}>
        <div className="empty">Nenhum projeto aberto.</div>
      </div>
    );
  }

  const git = profile.git;
  const checkRuntimes = async () => {
    setChecking(true);
    setError(null);
    try {
      const [node, python, docker] = await Promise.all([
        runtimeInfoApi.node(),
        runtimeInfoApi.python(),
        runtimeInfoApi.docker(),
      ]);
      setInstalled({ node, python, docker });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setChecking(false);
    }
  };

  const runScript = async (script: string) => {
    setError(null);
    try {
      const out = await packageApi.run({ path: profile.path, script, background: true });
      setNotice(`Iniciado: ${out.command} (aba Processos)`);
      onProcessStarted();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const scripts = Object.entries(profile.scripts);

  return (
    <div className="editor" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">{profile.name}</span>
        <span className="mono path" title={profile.path}>
          {profile.path}
        </span>
        <span className="meta">detectado às {formatTime(profile.detectedAt)}</span>
        <button className="icon-button" title="Detectar novamente" onClick={onRefresh}>
          <RefreshIcon />
        </button>
      </div>
      {error && <div className="inline-error">{error}</div>}
      {notice && <div className="inline-notice ok">{notice}</div>}
      <div className="profile">
        <section>
          <h3>Stack</h3>
          <Row label="Linguagem">
            <Chips items={profile.languages} />
          </Row>
          <Row label="Framework">
            <Chips items={profile.frameworks} />
          </Row>
          <Row label="Package manager">
            <Chips items={profile.packageManagers} />
          </Row>
          <Row label="Runtime">
            <Chips items={profile.runtimes.map((r) => (r.version ? `${r.name} ${r.version}` : r.name))} />
          </Row>
          <Row label="Ferramentas">
            <Chips items={profile.tools} />
          </Row>
          <Row label="Banco">
            <Chips items={profile.databases} />
          </Row>
          <Row label="Docker">
            {profile.docker.dockerfiles.length + profile.docker.composeFiles.length === 0 ? (
              <span className="meta">—</span>
            ) : (
              <Chips items={[...profile.docker.dockerfiles, ...profile.docker.composeFiles, ...profile.docker.images]} />
            )}
          </Row>
          {profile.monorepo && (
            <Row label="Estrutura">
              <span className="tag">monorepo</span>
            </Row>
          )}
        </section>

        <section>
          <h3>Git</h3>
          {git ? (
            <>
              <Row label="Git root">
                <span className="mono">{git.root}</span>
              </Row>
              <Row label="Branch">
                <span className="mono">{git.branch ?? `HEAD ${git.head ?? ""} (detached)`}</span>
                {git.upstream && (
                  <span className="meta">
                    {" "}
                    → {git.upstream} · ↑{git.ahead} ↓{git.behind}
                  </span>
                )}
              </Row>
              <Row label="Remote">
                {git.remotes.length === 0 ? (
                  <span className="meta">—</span>
                ) : (
                  git.remotes.map((r) => (
                    <div key={r.name} className="mono">
                      {r.name}: {r.url}
                    </div>
                  ))
                )}
              </Row>
              <Row label="Status">
                {git.clean ? (
                  <span className="badge ok">limpo</span>
                ) : (
                  <span className="chip-list">
                    {git.staged > 0 && <span className="tag">{git.staged} no stage</span>}
                    {git.modified > 0 && <span className="tag">{count(git.modified, "modificado", "modificados")}</span>}
                    {git.untracked > 0 && <span className="tag">{count(git.untracked, "novo", "novos")}</span>}
                    {git.deleted > 0 && <span className="tag">{count(git.deleted, "removido", "removidos")}</span>}
                    {git.conflicted > 0 && <span className="tag err">{count(git.conflicted, "conflito", "conflitos")}</span>}
                  </span>
                )}
              </Row>
            </>
          ) : (
            <div className="meta">Não é um repositório Git (ou Git indisponível).</div>
          )}
        </section>

        {scripts.length > 0 && (
          <section>
            <h3>Scripts</h3>
            {scripts.map(([name, command]) => (
              <div key={name} className="profile-row">
                <div className="profile-label mono">{name}</div>
                <div className="profile-value row">
                  <span className="mono grow ellipsis" title={command}>
                    {command}
                  </span>
                  <button className="button small" title="Executar como processo" onClick={() => void runScript(name)}>
                    <PlayIcon /> Executar
                  </button>
                </div>
              </div>
            ))}
          </section>
        )}

        <section>
          <h3>Arquivos importantes</h3>
          {profile.importantFiles.length === 0 ? (
            <div className="meta">—</div>
          ) : (
            <div className="chip-list">
              {profile.importantFiles.map((file) => (
                <button key={file} className="tag link-tag" onClick={() => onOpenFile(joinPath(profile.path, file))}>
                  {file}
                </button>
              ))}
            </div>
          )}
        </section>

        <section>
          <h3 className="row">
            <span className="grow">Runtimes instalados</span>
            <button className="button small" onClick={() => void checkRuntimes()} disabled={checking}>
              {checking ? "Verificando…" : "Verificar"}
            </button>
          </h3>
          {installed ? (
            <>
              <Row label="Node.js">
                {installed.node?.available ? (
                  <>
                    <span className="mono">{installed.node.version}</span>{" "}
                    <span className="meta">
                      {Object.entries(installed.node.managers)
                        .filter(([, v]) => v)
                        .map(([k, v]) => `${k} ${v}`)
                        .join(" · ")}
                    </span>
                  </>
                ) : (
                  <span className="meta">não encontrado</span>
                )}
              </Row>
              <Row label="Python">
                {installed.python?.available ? (
                  <span className="mono">
                    {installed.python.version} ({installed.python.command})
                  </span>
                ) : (
                  <span className="meta">não encontrado</span>
                )}
              </Row>
              <Row label="Docker">
                {installed.docker?.available ? (
                  <span className="mono">
                    {installed.docker.version} · daemon {installed.docker.daemonRunning ? "ativo" : "parado"}
                    {installed.docker.compose ? ` · compose ${installed.docker.compose}` : ""}
                  </span>
                ) : (
                  <span className="meta">não encontrado</span>
                )}
              </Row>
            </>
          ) : (
            <div className="meta">Consulta node, python e docker no shell padrão.</div>
          )}
        </section>

        <details className="evidence">
          <summary>Evidências ({profile.markers.length})</summary>
          <ul>
            {profile.markers.map((m) => (
              <li key={m} className="mono">
                {m}
              </li>
            ))}
          </ul>
        </details>
      </div>
    </div>
  );
}
