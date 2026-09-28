import { type MouseEvent as ReactMouseEvent, type ReactNode, useCallback, useEffect, useState } from "react";
import { CommandPanel } from "./components/CommandPanel";
import { ContextBar } from "./components/ContextBar";
import { Explorer } from "./components/Explorer";
import { FileEditor } from "./components/FileEditor";
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
import { SessionsPanel } from "./components/SessionsPanel";
import { StatusBar } from "./components/StatusBar";
import { TerminalPanel } from "./components/TerminalPanel";
import { baseName } from "./lib/format";
import { appApi, errorMessage, fsApi, isTauri, shellApi } from "./lib/runtime";
import type { AppInfo, ShellList } from "./lib/types";
import { useRuntimeSessions } from "./lib/useRuntimeSessions";

type PanelId = "project" | "providers" | "tasks" | "agents" | "terminal" | "git" | "memory" | "history";
type BottomTab = "terminal" | "processes" | "command";

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

const WORKSPACE_KEY = "orchestrator.workspace";

function loadStoredWorkspace(): string | null {
  try {
    return localStorage.getItem(WORKSPACE_KEY);
  } catch {
    return null;
  }
}

function storeWorkspace(path: string) {
  try {
    localStorage.setItem(WORKSPACE_KEY, path);
  } catch {
    // Storage unavailable: the workspace just is not remembered.
  }
}

export function App() {
  const ready = isTauri();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [shells, setShells] = useState<ShellList | null>(null);
  const [workspace, setWorkspace] = useState("");
  const [panel, setPanel] = useState<PanelId>("project");
  const [bottomTab, setBottomTab] = useState<BottomTab>("terminal");
  const [openFiles, setOpenFiles] = useState<string[]>([]);
  const [activeFile, setActiveFile] = useState<string | null>(null);
  const [dirtyFiles, setDirtyFiles] = useState<Set<string>>(new Set());
  const [confirmClose, setConfirmClose] = useState<string | null>(null);
  const [bottomHeight, setBottomHeight] = useState(300);
  const [startupError, setStartupError] = useState<string | null>(null);
  const sessions = useRuntimeSessions(ready);

  useEffect(() => {
    if (!ready) return;
    appApi
      .info()
      .then((appInfo) => {
        setInfo(appInfo);
        const stored = loadStoredWorkspace();
        if (!stored) return setWorkspace(appInfo.baseDir);
        fsApi
          .list(stored)
          .then(() => setWorkspace(stored))
          .catch(() => setWorkspace(appInfo.baseDir));
      })
      .catch((e) => setStartupError(errorMessage(e)));
    shellApi
      .list()
      .then(setShells)
      .catch((e) => setStartupError(errorMessage(e)));
  }, [ready]);

  const changeWorkspace = (path: string) => {
    setWorkspace(path);
    storeWorkspace(path);
  };

  const openFile = (path: string) => {
    setOpenFiles((files) => (files.includes(path) ? files : [...files, path]));
    setActiveFile(path);
  };

  const closeFile = (path: string) => {
    if (dirtyFiles.has(path) && confirmClose !== path) {
      setConfirmClose(path);
      return;
    }
    setConfirmClose(null);
    const index = openFiles.indexOf(path);
    const next = openFiles.filter((f) => f !== path);
    setOpenFiles(next);
    if (activeFile === path) setActiveFile(next[Math.min(index, next.length - 1)] ?? null);
    setDirtyFiles((dirty) => {
      const next = new Set(dirty);
      next.delete(path);
      return next;
    });
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

  const sidebar = (() => {
    switch (panel) {
      case "project":
        return (
          <Explorer ready={ready} workspace={workspace} onWorkspaceChange={changeWorkspace} onOpenFile={openFile} />
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
          <PhasePlaceholder
            title="Git"
            phase="Fase 2"
            description="Git local primeiro; GitHub é remoto (Fase 10)."
            items={["Branch atual, arquivos modificados/novos/removidos", "Diff e últimos commits", "Remote"]}
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

  return (
    <div className="app">
      <ContextBar terminals={sessions.terminals} processes={sessions.processes} />
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
            {openFiles.length > 0 && (
              <div className="tabs">
                {openFiles.map((path) => (
                  <div
                    key={path}
                    className={`tab ${path === activeFile ? "active" : ""}`}
                    onClick={() => setActiveFile(path)}
                    title={path}
                  >
                    <span>{baseName(path)}</span>
                    {confirmClose === path ? (
                      <span className="confirm">
                        descartar?
                        <button
                          className="link"
                          onClick={(e) => {
                            e.stopPropagation();
                            closeFile(path);
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
                        className={`icon-button small ${dirtyFiles.has(path) ? "dirty" : ""}`}
                        title={dirtyFiles.has(path) ? "Alterações não salvas" : "Fechar"}
                        onClick={(e) => {
                          e.stopPropagation();
                          closeFile(path);
                        }}
                      >
                        <CloseIcon />
                      </button>
                    )}
                  </div>
                ))}
              </div>
            )}
            {openFiles.length === 0 ? (
              <Welcome />
            ) : (
              openFiles.map((path) => (
                <FileEditor key={path} path={path} active={path === activeFile} onDirtyChange={onDirtyChange} />
              ))
            )}
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
      <StatusBar ready={ready} info={info} workspace={workspace} />
    </div>
  );
}

function Welcome() {
  return (
    <div className="welcome">
      <h1>Orchestrator</h1>
      <p className="quote">A IA é substituível. O projeto é permanente.</p>
      <div className="welcome-grid">
        <div>
          <h2>Fase 1 — runtime local</h2>
          <ul>
            <li>
              <strong>PROJECT</strong>: navegue no workspace, abra, edite (Ctrl+S), renomeie e exclua arquivos.
            </li>
            <li>
              <strong>Terminal</strong>: shells reais (PowerShell, CMD, WSL, Bash…) em PTY.
            </li>
            <li>
              <strong>Processos</strong>: <code>npm run dev</code> e afins, com saída ao vivo e encerramento da árvore.
            </li>
            <li>
              <strong>Comando</strong>: execução não interativa com stdout, stderr e exit code.
            </li>
            <li>
              <strong>HISTORY</strong>: toda chamada de ferramenta é auditada.
            </li>
          </ul>
        </div>
        <div>
          <h2>Próximas fases</h2>
          <ul>
            <li>2 — Project Discovery, Project Profile, Git</li>
            <li>3–5 — AIProvider, OpenAI/Codex, Claude Code</li>
            <li>6–7 — SQLite, memória, Context Builder, Handoff</li>
            <li>8–9 — Tasks, agentes, File Locks, autonomia</li>
          </ul>
        </div>
      </div>
    </div>
  );
}
