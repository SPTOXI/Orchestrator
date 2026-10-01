import { type MouseEvent as ReactMouseEvent, type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { CommandPanel } from "./components/CommandPanel";
import { ConnectionEditor } from "./components/ConnectionEditor";
import { ContextBar } from "./components/ContextBar";
import { type ContextTabRequest, ContextView } from "./components/ContextView";
import { AboutView } from "./components/AboutView";
import { CostView } from "./components/CostView";
import { CouncilEditor } from "./components/CouncilEditor";
import { DiffView } from "./components/DiffView";
import { DiscoveryView } from "./components/DiscoveryView";
import { FileEditor } from "./components/FileEditor";
import { GitPanel } from "./components/GitPanel";
import { HandoffView } from "./components/HandoffView";
import { HistoryPanel } from "./components/HistoryPanel";
import { MemoryPanel } from "./components/MemoryPanel";
import { TasksPanel } from "./components/TasksPanel";
import { TaskView } from "./components/TaskView";
import { type MemorySection, MemoryView } from "./components/MemoryView";
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
import { AgentsPanel } from "./components/AgentsPanel";
import { BoardView } from "./components/BoardView";
import { ApprovalBar } from "./components/ApprovalBar";
import { AutonomyView } from "./components/AutonomyView";
import { GitHubView } from "./components/GitHubView";
import { PullRequestView } from "./components/PullRequestView";
import { ProcessesPanel } from "./components/ProcessesPanel";
import { ProfileView } from "./components/ProfileView";
import { ProjectPanel, RecentList } from "./components/ProjectPanel";
import { ProvidersPanel } from "./components/ProvidersPanel";
import { RouteView } from "./components/RouteView";
import { SessionsPanel } from "./components/SessionsPanel";
import { SessionView } from "./components/SessionView";
import { StatusBar } from "./components/StatusBar";
import { TerminalPanel } from "./components/TerminalPanel";
import { baseName, joinPath } from "./lib/format";
import type { RecentProject } from "./lib/recent";
import { agentApi, appApi, errorMessage, isTauri, projectApi, sessionApi, shellApi } from "./lib/runtime";
import type { AppInfo, ConnectionsView, Deliberation, ProjectProfile, SessionInfo, ShellList } from "./lib/types";
import { useConnections } from "./lib/useConnections";
import { useCouncil } from "./lib/useCouncil";
import { useMemory } from "./lib/useMemory";
import { agentChip, agentOfTask } from "./lib/agents";
import { useAgents } from "./lib/useAgents";
import { updateChip } from "./lib/updates";
import { useUpdates } from "./lib/useUpdates";
import { useAutonomy } from "./lib/useAutonomy";
import { autonomyChip } from "./lib/autonomy";
import { useTasks } from "./lib/useTasks";
import { useProjects } from "./lib/useProjects";
import { useGitStatus } from "./lib/useGitStatus";
import { useProviders } from "./lib/useProviders";
import { useRuntimeSessions } from "./lib/useRuntimeSessions";

type PanelId = "project" | "providers" | "tasks" | "agents" | "terminal" | "git" | "memory" | "history";
type BottomTab = "terminal" | "processes" | "command";

/** Main-area tabs. */
type Tab =
  | { id: string; kind: "file"; path: string }
  | { id: string; kind: "diff"; repo: string; file: string; staged: boolean }
  | { id: "profile"; kind: "profile" }
  | { id: "discovery"; kind: "discovery" }
  | { id: string; kind: "session"; sessionId: string }
  | { id: string; kind: "connection"; connectionId: string | null }
  | { id: "council"; kind: "council" }
  /** Pick a model with the router / Council; `deliberation` from the history. */
  | { id: "route"; kind: "route"; deliberation: Deliberation | null }
  /** Project memory; `nonce` changes when the sidebar asks again. */
  | { id: "memory"; kind: "memory"; section: MemorySection; query: string; nonce: number }
  | { id: "context"; kind: "context"; request: ContextTabRequest; nonce: number }
  | { id: string; kind: "handoff"; sessionId: string | null; handoffId: string | null }
  /** One task; `taskId` null is the form of a new one. */
  | { id: string; kind: "task"; taskId: string | null; nonce: number }
  /** Agent Board: the tasks of the project in columns (ADR-0015). */
  | { id: "board"; kind: "board" }
  /** Mode, requests, rules and pause (ADR-0016). */
  | { id: "autonomy"; kind: "autonomy" }
  /** A pull request; `number` null is the form of a new one (ADR-0017). */
  | { id: string; kind: "pr"; number: number | null; nonce: number }
  /** GitHub connection: token and server (ADR-0017). */
  | { id: "github"; kind: "github" }
  /** What the AIs spent (ADR-0018). */
  | { id: "cost"; kind: "cost" }
  /** This copy of the app and its updates (ADR-0019). */
  | { id: "about"; kind: "about" };

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

function tabTitle(tab: Tab, sessions: Map<string, SessionInfo>, connections: ConnectionsView | null): string {
  switch (tab.kind) {
    case "file":
      return baseName(tab.path);
    case "diff":
      return `${baseName(tab.file)} (${tab.staged ? "staged" : "diff"})`;
    case "profile":
      return "Perfil do projeto";
    case "discovery":
      return "Procurar projetos";
    case "session":
      return sessions.get(tab.sessionId)?.title ?? "Sessão";
    case "council":
      return "Conselho de IAs";
    case "route":
      return "Nova sessão · Conselho";
    case "memory":
      return "Memória do projeto";
    case "context":
      return "Contexto do projeto";
    case "handoff":
      return "Handoff";
    case "task":
      return tab.taskId ? "Task" : "Nova task";
    case "board":
      return "Agent Board";
    case "autonomy":
      return "Autonomia";
    case "pr":
      return tab.number === null ? "Novo pull request" : `PR #${tab.number}`;
    case "github":
      return "GitHub";
    case "cost":
      return "Tokens e custo";
    case "about":
      return "Sobre e atualizações";
    case "connection":
      if (!tab.connectionId) return "Nova API";
      return `API · ${connections?.connections.find((c) => c.connection.id === tab.connectionId)?.connection.name ?? tab.connectionId}`;
  }
}

export function App() {
  const ready = isTauri();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [shells, setShells] = useState<ShellList | null>(null);
  const [profile, setProfile] = useState<ProjectProfile | null>(null);
  const [opening, setOpening] = useState(false);
  const [panel, setPanel] = useState<PanelId>("project");
  const [bottomTab, setBottomTab] = useState<BottomTab>("terminal");
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeTab, setActiveTab] = useState<string | null>(null);
  const [dirtyFiles, setDirtyFiles] = useState<Set<string>>(new Set());
  const [confirmClose, setConfirmClose] = useState<string | null>(null);
  const [bottomHeight, setBottomHeight] = useState(300);
  const [startupError, setStartupError] = useState<string | null>(null);
  const [startingSession, setStartingSession] = useState(false);
  const sessions = useRuntimeSessions(ready);
  const git = useGitStatus(profile?.path ?? null, ready && profile !== null);
  const providers = useProviders(ready);
  const connections = useConnections(ready);
  const council = useCouncil(ready);
  const projects = useProjects(ready);
  const recent = projects.recent;
  const projectId = profile && projects.current?.path === profile.path ? projects.current.id : null;
  const memory = useMemory(ready, projectId);
  const tasks = useTasks(ready, projectId);
  const agents = useAgents(ready, projectId);
  const updates = useUpdates(ready);
  const autonomy = useAutonomy(ready, projectId);
  /** Changes after a GitHub action, so the GIT panel asks GitHub again. */
  const [githubNonce, setGithubNonce] = useState(0);
  /** Tool calls waiting for the user, so a transcript can say so. */
  const waitingCalls = useMemo(
    () => new Set(autonomy.pending.map((request) => request.callId)),
    [autonomy.pending],
  );
  const openSessionId = activeTab?.startsWith("session:") ? activeTab.slice("session:".length) : null;
  /** What the top bar shows as the current task: the one of the open
   * session, else the task being worked on now (ADR-0014). */
  const currentTask =
    tasks.list.find((task) => openSessionId !== null && task.sessions.includes(openSessionId)) ??
    tasks.list.find((task) => task.status === "IN_PROGRESS") ??
    null;
  /** The agent of the open session, else the one working on the current
   * task (ADR-0015). */
  const currentAgent =
    agents.list.find((agent) => agent.session !== null && agent.session === openSessionId) ??
    agentOfTask(agents.list, currentTask?.id ?? null);
  const sessionsById = new Map(providers.sessions.map((s) => [s.id, s]));
  const activeProvider = providers.view?.providers.find((p) => p.active) ?? null;
  const runningSessions = providers.sessions.filter((s) => s.status === "running").length;

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

  const forgetRecent = (path: string) => void projects.forget(path);

  const openFile = (path: string) => showTab({ id: `file:${path}`, kind: "file", path });
  const openSession = (sessionId: string) => showTab({ id: `session:${sessionId}`, kind: "session", sessionId });

  const newSession = async (provider: string, model: string | null) => {
    setStartingSession(true);
    try {
      const session = await sessionApi.start({ provider, model: model ?? undefined });
      openSession(session.id);
    } catch (e) {
      setStartupError(`Não foi possível iniciar a sessão: ${errorMessage(e)}`);
    } finally {
      setStartingSession(false);
    }
  };
  const openCouncil = () => showTab({ id: "council", kind: "council" });
  /** The context tab is reused; each request refreshes it. */
  const openContext = (request: ContextTabRequest) => {
    const tab = { id: "context" as const, kind: "context" as const, request, nonce: Date.now() };
    setTabs((all) => (all.some((t) => t.id === "context") ? all.map((t) => (t.id === "context" ? tab : t)) : [...all, tab]));
    setActiveTab("context");
  };
  /** The Agent Board is a single tab (section 25). */
  const openBoard = () => showTab({ id: "board", kind: "board" });
  /** The autonomy tab is a single tab (ADR-0016). */
  const openAutonomy = () => showTab({ id: "autonomy", kind: "autonomy" });
  /** One tab per pull request; the new-PR form has its own (ADR-0017). */
  const openPull = (number: number | null) =>
    showTab({ id: number === null ? "pr:new" : `pr:${number}`, kind: "pr", number, nonce: Date.now() });
  const openGitHub = () => showTab({ id: "github", kind: "github" });
  /** Tokens and cost (ADR-0018). */
  const openCost = () => showTab({ id: "cost", kind: "cost" });
  /** About and updates (ADR-0019). */
  const openAbout = () => showTab({ id: "about", kind: "about" });
  const providerName = (id: string) => providers.view?.providers.find((p) => p.id === id)?.name ?? id;
  const pauseAgent = async (id: string, paused: boolean) => {
    if (paused) await agentApi.resume(id);
    else await agentApi.pause(id);
    await agents.refresh();
  };
  /** One tab per task; the new-task form has its own. */
  const openTask = (taskId: string | null) => {
    const id = taskId ? `task:${taskId}` : "task:new";
    showTab({ id, kind: "task", taskId, nonce: Date.now() });
  };
  /** One handoff tab per source session (new) or saved handoff. */
  const openHandoff = (target: { sessionId?: string; handoffId?: string }) => {
    const id = target.handoffId ? `handoff:${target.handoffId}` : `handoff:new:${target.sessionId}`;
    showTab({ id, kind: "handoff", sessionId: target.sessionId ?? null, handoffId: target.handoffId ?? null });
  };
  /** The route tab is reused; opening a past deliberation shows it there. */
  /** The memory tab is reused; the sidebar picks its section. */
  const openMemory = (section: MemorySection, query = "") => {
    const tab = { id: "memory" as const, kind: "memory" as const, section, query, nonce: Date.now() };
    setTabs((all) => (all.some((t) => t.id === "memory") ? all.map((t) => (t.id === "memory" ? tab : t)) : [...all, tab]));
    setActiveTab("memory");
  };
  const openRoute = (deliberation: Deliberation | null = null) => {
    setTabs((all) =>
      all.some((t) => t.id === "route")
        ? deliberation
          ? all.map((t) => (t.id === "route" ? { ...t, deliberation } : t))
          : all
        : [...all, { id: "route", kind: "route", deliberation }],
    );
    setActiveTab("route");
  };
  const routedSession = (session: SessionInfo) => {
    void providers.refresh();
    openSession(session.id);
  };
  /** One editor tab per connection; each "Adicionar API" opens a new one. */
  const editConnection = (connectionId: string | null) => {
    const open = connectionId ? tabs.find((t) => t.kind === "connection" && t.connectionId === connectionId) : null;
    if (open) setActiveTab(open.id);
    else showTab({ id: `connection:${Date.now()}`, kind: "connection", connectionId });
  };
  /** After a save the tab follows the (possibly new or renamed) connection. */
  const connectionSaved = (tabId: string, connectionId: string) =>
    setTabs((all) => all.map((t) => (t.id === tabId && t.kind === "connection" ? { ...t, connectionId } : t)));
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
          <ProvidersPanel
            ready={ready}
            providers={providers}
            projectPath={profile?.path ?? null}
            activeSessionId={activeTab?.startsWith("session:") ? activeTab.slice("session:".length) : null}
            starting={startingSession}
            connections={connections}
            onNewSession={(provider, model) => void newSession(provider, model)}
            onOpenSession={openSession}
            onEditConnection={editConnection}
            council={council}
            onOpenCouncil={openCouncil}
            onOpenRoute={() => openRoute()}
          />
        );
      case "tasks":
        return (
          <TasksPanel
            ready={ready}
            tasks={tasks.list}
            error={tasks.error}
            projectName={profile?.name ?? null}
            activeTaskId={activeTab?.startsWith("task:") ? activeTab.slice("task:".length) : null}
            onOpen={openTask}
            onNew={() => openTask(null)}
          />
        );
      case "agents":
        return (
          <AgentsPanel
            ready={ready}
            agents={agents.list}
            locks={agents.locks}
            settings={agents.settings}
            budget={agents.budget}
            providers={(providers.view?.providers ?? [])
              .filter((p) => p.capabilities.toolCalls)
              .map((p) => ({ id: p.id, name: p.name }))}
            onOpenCost={openCost}
            error={agents.error}
            projectName={profile?.name ?? null}
            activeAgentId={currentAgent?.id ?? null}
            onOpenBoard={openBoard}
            onOpenTask={openTask}
            onStop={async (id) => {
              await agentApi.stop(id);
              await agents.refresh();
            }}
            onStopAll={async () => {
              await agentApi.stopAll(projectId ?? undefined);
              await agents.refresh();
            }}
            onSaveSettings={agents.saveSettings}
            pausedAll={autonomy.overview?.pausedAll ?? false}
            onPause={(id) => pauseAgent(id, false)}
            onResume={(id) => pauseAgent(id, true)}
            onPauseAll={async () => {
              await autonomy.pauseAll();
              await agents.refresh();
            }}
            onResumeAll={async () => {
              await autonomy.resumeAll();
              await agents.refresh();
            }}
            onOpenAutonomy={openAutonomy}
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
            onOpenPull={openPull}
            onOpenGitHub={openGitHub}
            githubNonce={githubNonce}
          />
        );
      case "memory":
        return (
          <MemoryPanel
            ready={ready}
            memory={memory}
            projectName={profile?.name ?? null}
            database={info?.database ?? null}
            onOpen={openMemory}
            onOpenSession={openSession}
          />
        );
      case "history":
        return <HistoryPanel ready={ready} database={info?.database ?? null} projectId={projectId} />;
    }
  })();

  const branchLabel = git.status
    ? (git.status.branch ?? (git.status.detached ? `HEAD ${git.status.head?.slice(0, 7) ?? ""}` : "(sem commits)"))
    : git.notRepo
      ? "sem Git"
      : null;

  return (
    <div className="app">
      <div className="top">
        <ContextBar
          projectName={profile?.name ?? null}
          branch={branchLabel}
          gitStatus={git.status}
          terminals={sessions.terminals}
          processes={sessions.processes}
          providerName={activeProvider?.name ?? null}
          providerCount={providers.view?.providers.length ?? null}
          runningSessions={runningSessions}
          task={currentTask ? { title: currentTask.title, status: currentTask.status } : null}
          openTasks={tasks.list.filter((t) => t.status !== "DONE" && t.status !== "CANCELLED").length}
          agent={currentAgent ? { title: currentAgent.taskTitle, status: currentAgent.status } : null}
          agentSummary={agentChip(agents.list, openSessionId)}
          autonomy={autonomyChip(autonomy.overview, autonomy.pending.length)}
          onOpenAutonomy={openAutonomy}
        />
        <ApprovalBar pending={autonomy.pending} onAnswer={autonomy.answer} onOpen={openAutonomy} />
      </div>
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
              {id === "providers" && runningSessions > 0 && <span className="activity-badge">{runningSessions}</span>}
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
                      <span>{tabTitle(tab, sessionsById, connections.view)}</span>
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
                case "session": {
                  const session = sessionsById.get(tab.sessionId) ?? null;
                  return (
                    <SessionView
                      key={tab.id}
                      ready={ready}
                      active={active}
                      sessionId={tab.sessionId}
                      session={session}
                      parent={session?.parentId ? (sessionsById.get(session.parentId) ?? null) : null}
                      providers={providers.view?.providers ?? []}
                      sessionTitle={(id) => sessionsById.get(id)?.title ?? null}
                      onOpenSession={openSession}
                      onOpenContext={openContext}
                      onOpenHandoff={(sessionId) => openHandoff({ sessionId })}
                      waitingCalls={waitingCalls}
                    />
                  );
                }
                case "connection":
                  return (
                    <ConnectionEditor
                      key={tab.id}
                      ready={ready}
                      active={active}
                      connectionId={tab.connectionId}
                      view={connections.view}
                      onSaved={(id) => connectionSaved(tab.id, id)}
                      onDeleted={() => closeTab(tab.id)}
                    />
                  );
                case "council":
                  return (
                    <CouncilEditor
                      key={tab.id}
                      ready={ready}
                      active={active}
                      council={council}
                      providers={providers.view?.providers ?? []}
                      onOpenDeliberation={openRoute}
                    />
                  );
                case "memory":
                  return (
                    <MemoryView
                      key={tab.id}
                      ready={ready}
                      active={active}
                      memory={memory}
                      projectId={projectId}
                      section={tab.section}
                      query={tab.query}
                      nonce={tab.nonce}
                      onOpenSession={openSession}
                      onOpenFile={(path) =>
                        openFile(/^([a-zA-Z]:)?[\\/]/.test(path) || !profile ? path : joinPath(profile.path, path))
                      }
                      onOpenHandoff={(handoffId) => openHandoff({ handoffId })}
                      onOpenTask={openTask}
                    />
                  );
                case "context":
                  return (
                    <ContextView key={tab.id} ready={ready} active={active} request={tab.request} nonce={tab.nonce} />
                  );
                case "task":
                  return (
                    <TaskView
                      key={tab.id}
                      ready={ready}
                      active={active}
                      taskId={tab.taskId}
                      nonce={tab.nonce}
                      tasks={tasks.list}
                      agents={agents.list}
                      projectMode={autonomy.overview?.mode ?? null}
                      providers={providers.view?.providers ?? []}
                      onAgentChanged={() => void agents.refresh()}
                      onSaved={(id) => {
                        void tasks.refresh();
                        if (!tab.taskId) {
                          closeTab(tab.id);
                          openTask(id);
                        }
                      }}
                      onOpenSession={openSession}
                      onOpenContext={(taskId) => openContext({ taskId })}
                    />
                  );
                case "board":
                  return (
                    <BoardView
                      key={tab.id}
                      active={active}
                      tasks={tasks.list}
                      agents={agents.list}
                      onOpenTask={openTask}
                      onOpenSession={openSession}
                      onPause={(id) => void pauseAgent(id, false)}
                      onResume={(id) => void pauseAgent(id, true)}
                    />
                  );
                case "pr":
                  return (
                    <PullRequestView
                      key={tab.id}
                      active={active}
                      ready={ready}
                      repo={profile?.path ?? undefined}
                      number={tab.number}
                      nonce={tab.nonce}
                      tasks={tasks.list}
                      onCreated={(number) => {
                        closeTab(tab.id);
                        openPull(number);
                      }}
                      onChanged={() => {
                        setGithubNonce(Date.now());
                        git.refresh();
                      }}
                    />
                  );
                case "github":
                  return (
                    <GitHubView
                      key={tab.id}
                      active={active}
                      ready={ready}
                      repo={profile?.path ?? undefined}
                      onChanged={() => setGithubNonce(Date.now())}
                    />
                  );
                case "about":
                  return (
                    <AboutView
                      key={tab.id}
                      active={active}
                      ready={ready}
                      info={info}
                      updates={updates}
                      liveAgents={agents.list.filter((a) => a.status === "RUNNING" || a.status === "QUEUED").length}
                    />
                  );
                case "cost":
                  return (
                    <CostView
                      key={tab.id}
                      active={active}
                      ready={ready}
                      projectName={projectId ? (profile?.name ?? null) : null}
                      providerName={providerName}
                      onOpenAgents={() => setPanel("agents")}
                    />
                  );
                case "autonomy":
                  return (
                    <AutonomyView
                      key={tab.id}
                      active={active}
                      ready={ready}
                      projectName={projectId ? (profile?.name ?? null) : null}
                      autonomy={autonomy}
                    />
                  );
                case "handoff":
                  return (
                    <HandoffView
                      key={tab.id}
                      ready={ready}
                      active={active}
                      sessionId={tab.sessionId}
                      handoffId={tab.handoffId}
                      sessions={sessionsById}
                      providers={providers.view?.providers ?? []}
                      onSaved={() => void memory.refresh()}
                      onOpenSession={(id) => {
                        void providers.refresh();
                        openSession(id);
                      }}
                      onOpenContext={openContext}
                    />
                  );
                case "route":
                  return (
                    <RouteView
                      key={tab.id}
                      ready={ready}
                      active={active}
                      council={council}
                      projectPath={profile?.path ?? null}
                      initial={tab.deliberation}
                      onSessionStarted={routedSession}
                      onOpenCouncil={openCouncil}
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
      <StatusBar
        ready={ready}
        info={info}
        workspace={workspace}
        branch={branchLabel}
        spentToday={agents.budget}
        onOpenCost={openCost}
        update={updateChip(updates.status)}
        onOpenAbout={openAbout}
      />
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
                  <strong>AI PROVIDERS</strong>: quantas APIs você quiser — OpenAI e compatíveis, Anthropic, Gemini
                  ou qualquer API descrita por perfil — com chave no cofre do sistema, modelos, preços e sessões com
                  streaming, ferramentas pelo Orchestrator, cancelamento, subagentes, tokens e custo.
                </li>
                <li>
                  <strong>Conselho de IAs</strong>: o roteador ordena os modelos para cada tarefa sem gastar tokens, e o
                  Conselho (1 a 5 IAs) decide o melhor — você aprova (Sugerir) ou ele aplica sozinho (Full).
                </li>
                <li>
                  <strong>MEMORY</strong>: banco local com memória do projeto (trabalho, projeto, decisões e busca), e
                  sessões e conversas que continuam depois de reiniciar o app.
                </li>
                <li>
                  <strong>Contexto e handoff</strong>: cada sessão recebe o essencial do projeto num orçamento de
                  tokens, as IAs consultam e registram memória, e uma IA passa o trabalho para outra sem a conversa.
                </li>
                <li>
                  <strong>TASKS</strong>: todo trabalho relevante é uma task, com estado, prioridade,
                  dependências e subtasks; a task vira o contexto da IA que trabalha nela.
                </li>
                <li>
                  <strong>AGENTS</strong>: um agente executa a task sozinho e a deixa em revisão; mais de um
                  ao mesmo tempo quando não há conflito, com travas por arquivo, subagentes e Agent Board.
                </li>
                <li>
                  <strong>Autonomia</strong>: você decide o que as IAs fazem sozinhas — Assistido (pede
                  autorização), Autônomo (segue as suas regras) ou Acesso Irrestrito — por projeto ou por
                  agente; e pode pausar as IAs a qualquer momento.
                </li>
                <li>
                  <strong>GitHub</strong>: pull requests com CI, revisões, comentários e merge, issues e fetch, pelo
                  painel GIT ou pelas IAs — com o token no cofre do sistema.
                </li>
                <li>
                  <strong>Tokens e custo</strong>: custo real com cache de prompt, conversas longas compactadas pela
                  própria IA, fila de agentes por prioridade com limite por provider, teto de custo por agente e
                  orçamento diário por projeto.
                </li>
                <li>
                  <strong>Instaladores e atualizações</strong>: instaladores para Windows, macOS e Linux, e o app avisa
                  quando há versão nova — você decide quando instalar (clique na versão, na barra de status).
                </li>
                <li>
                  <strong>HISTORY</strong>: toda chamada de ferramenta é auditada, inclusive as feitas por IAs.
                </li>
              </ul>
            </>
          )}
        </div>
        <div>
          <h2>Fases</h2>
          <ul>
            <li>0 a 11 concluídas: o plano do documento mestre está completo.</li>
            <li>12 — instaladores, release e atualização automática.</li>
          </ul>
        </div>
      </div>
    </div>
  );
}
