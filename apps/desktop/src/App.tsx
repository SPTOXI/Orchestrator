import { type MouseEvent as ReactMouseEvent, type ReactNode, useCallback, useEffect, useState } from "react";
import { CommandPanel } from "./components/CommandPanel";
import { ContextBar } from "./components/ContextBar";
import { DiffView } from "./components/DiffView";
import { DiscoveryView } from "./components/DiscoveryView";
import { FileEditor } from "./components/FileEditor";
import { GitPanel } from "./components/GitPanel";
import { HistoryPanel } from "./components/HistoryPanel";
import {
  AgentsIcon,
  CloseIcon,
  FolderIcon,
  GitIcon,
  HistoryIcon,
  MemoryIcon,
  ProvidersIcon,
  TasksIcon,
  TerminalIcon,
} from "./components/icons";
import { PhasePlaceholder } from "./components/PhasePlaceholder";
import { ProcessesPanel } from "./components/ProcessesPanel";
import { ProfileView } from "./components/ProfileView";
import { ProjectPanel, RecentList } from "./components/ProjectPanel";
import { SessionsPanel } from "./components/SessionsPanel";
import { StatusBar } from "./components/StatusBar";
import { TerminalPanel } from "./components/TerminalPanel";
import { baseName } from "./lib/format";
import { addRecent, loadRecent, removeRecent, saveRecent, type RecentProject } from "./lib/recent";
import { appApi, errorMessage, isTauri, projectApi, shellApi } from "./lib/runtime";
import type { AppInfo, ProjectProfile, ShellList } from "./lib/types";
import { useGitStatus } from "./lib/useGitStatus";
import { useRuntimeSessions } from "./lib/useRuntimeSessions";

type PanelId = "project" | "providers" | "tasks" | "agents" | "terminal" | "git" | "memory" | "history";
type BottomTab = "terminal" | "processes" | "command";

/** Main-area tabs. */
type Tab =
  | { id: string; kind: "file"; path: string }
  | { id: string; kind: "diff"; repo: string; file: string; staged: boolean }
  | { id: "profile"; kind: "profile" }
  | { id: "discovery"; kind: "discovery" };

const ACTIVITIES: Array<{ id: PanelId; label: string; icon: () => ReactNode }> = [
  { id: "project", label: "PROJECT", icon: FolderIcon },
  { id: "providers", label: "AI PROVIDERS", icon: ProvidersIcon },
  { id: "tasks", label: "TASKS", icon: TasksIcon },
  { id: "agents", label: "AGENTS", icon: AgentsIcon },
  { id: "terminal", label: "TERMINAL", icon: TerminalIcon },
  { id: "git", label: "GIT", icon: GitIcon },
  { id: "memory", label: "MEMORY", icon: MemoryIcon },
  { id: "history", label: "HISTORY", icon: HistoryIcon },
];

const PROJECT_KEY = "orchestrator.project";

function loadStoredProject(): string | null {
  try {
    return localStorage.getItem(PROJECT_KEY);
  } catch {
    return null;
  }
}

function storeProject(path: string | null) {
  try {
    if (path) localStorage.setItem(PROJECT_KEY, path);
    else localStorage.removeItem(PROJECT_KEY);
  } catch {
    // Storage unavailable: the project is just not reopened next time.
  }
}

function tabTitle(tab: Tab): string {
  switch (tab.kind) {
    case "file":
      return baseName(tab.path);
    case "diff":
      return `${baseName(tab.file)} (${tab.staged ? "staged" : "diff"})`;
    case "profile":
      return "Perfil do projeto";
    case "discovery":
      return "Procurar projetos";
  }
}

export function App() {
  const ready = isTauri();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [shells, setShells] = useState<ShellList | null>(null);
  const [profile, setProfile] = useState<ProjectProfile | null>(null);
  const [opening, setOpening] = useState(false);
  const [recent, setRecent] = useState<RecentProject[]>(() => loadRecent());
  const [panel, setPanel] = useState<PanelId>("project");
  const [bottomTab, setBottomTab] = useState<BottomTab>("terminal");
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeTab, setActiveTab] = useState<string | null>(null);
  const [dirtyFiles, setDirtyFiles] = useState<Set<string>>(new Set());
  const [confirmClose, setConfirmClose] = useState<string | null>(null);
  const [bottomHeight, setBottomHeight] = useState(300);
  const [startupError, setStartupError] = useState<string | null>(null);
  const sessions = useRuntimeSessions(ready);
  const git = useGitStatus(profile?.path ?? null, ready && profile !== null);

  /** Working directory for terminals, processes and commands. */
  const workspace = profile?.path ?? info?.baseDir ?? "";

  const showTab = useCallback((tab: Tab) => {
    setTabs((all) => (all.some((t) => t.id === tab.id) ? all : [...all, tab]));
    setActiveTab(tab.id);
  }, []);

  const openProject = useCallback(
    async (path: string, options: { quiet?: boolean } = {}) => {
      setOpening(true);
      setStartupError(null);
      try {
        const opened = await projectApi.open(path);
        setProfile(opened);
        storeProject(opened.path);
        setRecent((list) => {
          const next = addRecent(list, { path: opened.path, name: opened.name, openedAt: new Date().toISOString() });
          saveRecent(next);
          return next;
        });
        setPanel("project");
        return true;
      } catch (e) {
        if (!options.quiet) setStartupError(`Não foi possível abrir ${path}: ${errorMessage(e)}`);
        return false;
      } finally {
        setOpening(false);
      }
    },
    [],
  );

  useEffect(() => {
    if (!ready) return;
    appApi
      .info()
      .then(setInfo)
      .catch((e) => setStartupError(errorMessage(e)));
    shellApi
      .list()
      .then(setShells)
      .catch((e) => setStartupError(errorMessage(e)));
    const stored = loadStoredProject();
    if (stored) {
      void openProject(stored, { quiet: true }).then((ok) => {
        if (!ok) storeProject(null);
      });
    }
  }, [ready, openProject]);

  const pickFolder = async () => {
    try {
      const folder = await appApi.pickFolder();
      if (folder) await openProject(folder);
    } catch (e) {
      setStartupError(errorMessage(e));
    }
  };

  const refreshProfile = async () => {
    if (!profile) return;
    try {
      setProfile(await projectApi.profile(profile.path));
    } catch (e) {
      setStartupError(errorMessage(e));
    }
  };

  const forgetRecent = (path: string) =>
    setRecent((list) => {
      const next = removeRecent(list, path);
      saveRecent(next);
      return next;
    });

  const openFile = (path: string) => showTab({ id: `file:${path}`, kind: "file", path });
  const openDiff = (file: string, staged: boolean) => {
    const repo = git.status?.root ?? profile?.path;
    if (repo) showTab({ id: `diff:${staged ? "s" : "u"}:${file}`, kind: "diff", repo, file, staged });
  };

  const closeTab = (id: string) => {
    const tab = tabs.find((t) => t.id === id);
    const dirty = tab?.kind === "file" && dirtyFiles.has(tab.path);
    if (dirty && confirmClose !== id) {
      setConfirmClose(id);
      return;
    }
    setConfirmClose(null);
    const index = tabs.findIndex((t) => t.id === id);
    const next = tabs.filter((t) => t.id !== id);
    setTabs(next);
    if (activeTab === id) setActiveTab(next[Math.min(index, next.length - 1)]?.id ?? null);
    if (tab?.kind === "file") {
      setDirtyFiles((all) => {
        const rest = new Set(all);
        rest.delete(tab.path);
        return rest;
      });
    }
  };

  const onDirtyChange = useCallback((path: string, dirty: boolean) => {
    setDirtyFiles((prev) => {
      if (prev.has(path) === dirty) return prev;
      const next = new Set(prev);
      if (dirty) next.add(path);
      else next.delete(path);
      return next;
    });
  }, []);

  const startResize = (e: ReactMouseEvent) => {
    e.preventDefault();
    const onMove = (move: MouseEvent) => {
      const height = window.innerHeight - move.clientY - 24;
      setBottomHeight(Math.max(120, Math.min(height, window.innerHeight - 220)));
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const selectActivity = (id: PanelId) => {
    setPanel(id);
    if (id === "terminal") setBottomTab("terminal");
  };

  const gitVersion = `${git.status?.head ?? ""}:${git.status?.files.map((f) => `${f.path}${f.staged}${f.unstaged}`).join("|") ?? ""}`;

  const sidebar = (() => {
    switch (panel) {
      case "project":
        return (
          <ProjectPanel
            ready={ready}
            profile={profile}
            gitStatus={git.status}
            recent={recent}
            opening={opening}
            onPickFolder={() => void pickFolder()}
            onOpenProject={(path) => void openProject(path)}
            onRemoveRecent={forgetRecent}
            onShowProfile={() => showTab({ id: "profile", kind: "profile" })}
            onShowDiscovery={() => showTab({ id: "discovery", kind: "discovery" })}
            onOpenFile={openFile}
          />
        );
      case "providers":
        return (
          <PhasePlaceholder
            title="AI Providers"
            phase="Fases 3–5"
            description="Providers são intercambiáveis e nunca acessam o sistema diretamente: pedem tool_call e o Orchestrator executa pelo Tool Runtime."
            items={[
              "Fase 3 — interface AIProvider, Provider Registry, Provider Sessions",
              "Fase 4 — OpenAI / Codex",
              "Fase 5 — Claude Code",
              "Futuro — Gemini, modelos locais",
            ]}
          />
        );
      case "tasks":
        return (
          <PhasePlaceholder
            title="Tasks"
            phase="Fase 8"
            description="Toda atividade relevante será uma Task com dependências, prioridade, provider e agente."
            items={["Estados: TODO · IN_PROGRESS · BLOCKED · REVIEW · DONE · CANCELLED", "Agent Board (kanban)"]}
          />
        );
      case "agents":
        return (
          <PhasePlaceholder
            title="Agents"
            phase="Fase 8"
            description="Agentes são temporários; o conhecimento fica no projeto."
            items={["Agent Manager e subagentes", "File Lock Manager", "Handoff entre IAs (Fase 7)"]}
          />
        );
      case "terminal":
        return (
          <SessionsPanel
            terminals={sessions.terminals}
            processes={sessions.processes}
            shells={shells}
            onOpen={setBottomTab}
          />
        );
      case "git":
        return (
          <GitPanel
            ready={ready}
            projectPath={profile?.path ?? null}
            git={git}
            onOpenDiff={openDiff}
            onOpenFile={openFile}
          />
        );
      case "memory":
        return (
          <PhasePlaceholder
            title="Memory"
            phase="Fase 6"
            description="A memória pertence ao projeto, não ao provider."
            items={["L1 — Working Memory", "L2 — Project Memory", "L3 — Historical Memory", "Decisões"]}
          />
        );
      case "history":
        return <HistoryPanel ready={ready} auditLog={info?.auditLog ?? null} />;
    }
  })();

  const branchLabel = git.status
    ? (git.status.branch ?? (git.status.detached ? `HEAD ${git.status.head?.slice(0, 7) ?? ""}` : "(sem commits)"))
    : git.notRepo
      ? "sem Git"
      : null;

  return (
    <div className="app">
      <ContextBar
        projectName={profile?.name ?? null}
        branch={branchLabel}
        gitStatus={git.status}
        terminals={sessions.terminals}
        processes={sessions.processes}
      />
      <div className="workbench">
        <nav className="activity-bar">
          {ACTIVITIES.map(({ id, label, icon: IconComponent }) => (
            <button
              key={id}
              className={`activity ${panel === id ? "active" : ""}`}
              title={label}
              aria-label={label}
              onClick={() => selectActivity(id)}
            >
              <IconComponent />
              {id === "git" && git.status && git.status.files.length > 0 && (
                <span className="activity-badge">{git.status.files.length}</span>
              )}
            </button>
          ))}
        </nav>
        <aside className="sidebar">{sidebar}</aside>
        <main className="main" style={{ gridTemplateRows: `minmax(0, 1fr) 5px ${bottomHeight}px` }}>
          <section className="editor-area">
            {!ready && (
              <div className="banner">
                Frontend aberto fora do app desktop: o runtime não está disponível. Execute <code>pnpm dev</code> na
                raiz do repositório.
              </div>
            )}
            {startupError && <div className="inline-error">{startupError}</div>}
            {tabs.length > 0 && (
              <div className="tabs">
                {tabs.map((tab) => {
                  const dirty = tab.kind === "file" && dirtyFiles.has(tab.path);
                  return (
                    <div
                      key={tab.id}
                      className={`tab ${tab.id === activeTab ? "active" : ""}`}
                      onClick={() => setActiveTab(tab.id)}
                      title={tab.kind === "file" ? tab.path : tab.kind === "diff" ? `${tab.repo} — ${tab.file}` : undefined}
                    >
                      <span>{tabTitle(tab)}</span>
                      {confirmClose === tab.id ? (
                        <span className="confirm">
                          descartar?
                          <button
                            className="link"
                            onClick={(e) => {
                              e.stopPropagation();
                              closeTab(tab.id);
                            }}
                          >
                            sim
                          </button>
                          <button
                            className="link"
                            onClick={(e) => {
                              e.stopPropagation();
                              setConfirmClose(null);
                            }}
                          >
                            não
                          </button>
                        </span>
                      ) : (
                        <button
                          className={`icon-button small ${dirty ? "dirty" : ""}`}
                          title={dirty ? "Alterações não salvas" : "Fechar"}
                          onClick={(e) => {
                            e.stopPropagation();
                            closeTab(tab.id);
                          }}
                        >
                          <CloseIcon />
                        </button>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
            {tabs.length === 0 && (
              <Welcome
                ready={ready}
                hasProject={profile !== null}
                recent={recent}
                onPickFolder={() => void pickFolder()}
                onDiscover={() => showTab({ id: "discovery", kind: "discovery" })}
                onOpenProject={(path) => void openProject(path)}
                onRemoveRecent={forgetRecent}
              />
            )}
            {tabs.map((tab) => {
              const active = tab.id === activeTab;
              switch (tab.kind) {
                case "file":
                  return <FileEditor key={tab.id} path={tab.path} active={active} onDirtyChange={onDirtyChange} />;
                case "diff":
                  return (
                    <DiffView
                      key={tab.id}
                      repo={tab.repo}
                      file={tab.file}
                      staged={tab.staged}
                      active={active}
                      version={gitVersion}
                    />
                  );
                case "profile":
                  return (
                    <ProfileView
                      key={tab.id}
                      profile={profile}
                      active={active}
                      onRefresh={() => void refreshProfile()}
                      onOpenFile={openFile}
                      onProcessStarted={() => setBottomTab("processes")}
                    />
                  );
                case "discovery":
                  return (
                    <DiscoveryView
                      key={tab.id}
                      active={active}
                      onOpenProject={(path) => {
                        void openProject(path).then((ok) => ok && showTab({ id: "profile", kind: "profile" }));
                      }}
                    />
                  );
              }
            })}
          </section>
          <div className="splitter" onMouseDown={startResize} title="Arraste para redimensionar" />
          <section className="bottom-panel">
            <div className="tabs bottom-tabs">
              {(
                [
                  ["terminal", "Terminal"],
                  ["processes", "Processos"],
                  ["command", "Comando"],
                ] as const
              ).map(([id, label]) => (
                <button
                  key={id}
                  className={`tab ${bottomTab === id ? "active" : ""}`}
                  onClick={() => setBottomTab(id)}
                >
                  {label}
                  {id === "processes" && sessions.processes.some((p) => p.status === "running") && (
                    <span className="count">{sessions.processes.filter((p) => p.status === "running").length}</span>
                  )}
                </button>
              ))}
            </div>
            <div className="bottom-body" hidden={bottomTab !== "terminal"}>
              <TerminalPanel
                ready={ready}
                visible={bottomTab === "terminal"}
                terminals={sessions.terminals}
                loaded={sessions.loaded}
                shells={shells}
                workspace={workspace}
              />
            </div>
            <div className="bottom-body" hidden={bottomTab !== "processes"}>
              <ProcessesPanel
                ready={ready}
                visible={bottomTab === "processes"}
                processes={sessions.processes}
                shells={shells}
                workspace={workspace}
              />
            </div>
            <div className="bottom-body" hidden={bottomTab !== "command"}>
              <CommandPanel ready={ready} shells={shells} workspace={workspace} />
            </div>
          </section>
        </main>
      </div>
      <StatusBar ready={ready} info={info} workspace={workspace} branch={branchLabel} />
    </div>
  );
}

interface WelcomeProps {
  ready: boolean;
  hasProject: boolean;
  recent: RecentProject[];
  onPickFolder: () => void;
  onDiscover: () => void;
  onOpenProject: (path: string) => void;
  onRemoveRecent: (path: string) => void;
}

function Welcome({ ready, hasProject, recent, onPickFolder, onDiscover, onOpenProject, onRemoveRecent }: WelcomeProps) {
  return (
    <div className="welcome">
      <h1>Orchestrator</h1>
      <p className="quote">A IA é substituível. O projeto é permanente.</p>
      {!hasProject && (
        <div className="welcome-actions">
          <button className="button primary" disabled={!ready} onClick={onPickFolder}>
            Abrir pasta…
          </button>
          <button className="button" disabled={!ready} onClick={onDiscover}>
            Procurar projetos
          </button>
        </div>
      )}
      <div className="welcome-grid">
        <div>
          {!hasProject && recent.length > 0 ? (
            <>
              <h2>Projetos recentes</h2>
              <RecentList recent={recent} onOpenProject={onOpenProject} onRemoveRecent={onRemoveRecent} />
            </>
          ) : (
            <>
              <h2>O que já funciona</h2>
              <ul>
                <li>
                  <strong>PROJECT</strong>: abrir e descobrir projetos, perfil (stack, runtimes, Docker, bancos, Git) e
                  arquivos.
                </li>
                <li>
                  <strong>GIT</strong>: branch, stage, diff, commit, pull, push, stash e histórico.
                </li>
                <li>
                  <strong>Terminal</strong>: shells reais (PowerShell, CMD, WSL, Bash…) em PTY.
                </li>
                <li>
                  <strong>Processos</strong> e <strong>Comando</strong>: execução com saída, exit code e encerramento da
                  árvore.
                </li>
                <li>
                  <strong>HISTORY</strong>: toda chamada de ferramenta é auditada.
                </li>
              </ul>
            </>
          )}
        </div>
        <div>
          <h2>Próximas fases</h2>
          <ul>
            <li>3–5 — AIProvider, OpenAI/Codex, Claude Code</li>
            <li>6–7 — SQLite, memória, Context Builder, Handoff</li>
            <li>8–9 — Tasks, agentes, File Locks, autonomia</li>
            <li>10–11 — GitHub, otimização de tokens</li>
          </ul>
        </div>
      </div>
    </div>
  );
}
