// Main area: one task (ADR-0014). Title, state and the moves the engine
// allows, dependencies, subtasks, files, the AI chosen for it and the
// session that works on it — which starts with the task as its context.

import { useCallback, useEffect, useState } from "react";
import { formatTime } from "../lib/format";
import { councilApi, errorMessage, taskApi } from "../lib/runtime";
import { TASK_PRIORITY_LABELS, TASK_STATUS_LABELS, taskActionLabel, taskText } from "../lib/tasks";
import type { ProviderInfo, TaskPriority, TaskStatus, TaskView as TaskViewData } from "../lib/types";

const PRIORITIES: TaskPriority[] = ["LOW", "NORMAL", "HIGH", "URGENT"];

interface Props {
  ready: boolean;
  active: boolean;
  /** Task to show; `null` opens the form of a new one. */
  taskId: string | null;
  /** Changes when the tab is reopened. */
  nonce: number;
  tasks: TaskViewData[];
  providers: ProviderInfo[];
  onSaved: (id: string) => void;
  onOpenSession: (id: string) => void;
  onOpenContext: (taskId: string) => void;
}

export function TaskView({
  ready,
  active,
  taskId,
  nonce,
  tasks,
  providers,
  onSaved,
  onOpenSession,
  onOpenContext,
}: Props) {
  const [task, setTask] = useState<TaskViewData | null>(null);
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [files, setFiles] = useState("");
  const [result, setResult] = useState("");
  const [priority, setPriority] = useState<TaskPriority>("NORMAL");
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  const [dependencies, setDependencies] = useState<string[]>([]);
  const [suggestion, setSuggestion] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const fill = useCallback((next: TaskViewData | null) => {
    setTask(next);
    setTitle(next?.title ?? "");
    setDescription(next?.description ?? "");
    setFiles((next?.files ?? []).join("\n"));
    setResult(next?.result ?? "");
    setPriority(next?.priority ?? "NORMAL");
    setProvider(next?.provider ?? "");
    setModel(next?.model ?? "");
    setDependencies(next?.dependencies ?? []);
    setSuggestion(null);
    setError(null);
  }, []);

  useEffect(() => {
    if (!ready) return;
    if (!taskId) {
      fill(null);
      return;
    }
    taskApi
      .get(taskId)
      .then((found) => (found ? fill(found) : setError("Task não encontrada.")))
      .catch((e) => setError(errorMessage(e)));
  }, [ready, taskId, nonce, fill]);

  // The task may move without this tab asking: a session opened for it, a
  // dependency finished elsewhere, or (Fase 8b) an agent working on it.
  // Follow the panel's copy for everything the engine computes, and leave
  // the fields the user is editing alone.
  useEffect(() => {
    if (!taskId) return;
    const fresh = tasks.find((t) => t.id === taskId);
    if (!fresh) return;
    const state = (t: TaskViewData) =>
      JSON.stringify([t.updatedAt, t.status, t.waitingFor, t.subtasks, t.can, t.sessions]);
    setTask((current) => (current && state(current) === state(fresh) ? current : fresh));
  }, [tasks, taskId]);

  const target = providers.find((p) => p.id === provider) ?? null;
  const models = target?.capabilities.models ?? [];
  const others = tasks.filter((t) => t.id !== taskId);

  const run = async (label: string, action: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const save = () =>
    run("save", async () => {
      const stored = await taskApi.save({
        id: taskId ?? undefined,
        title,
        description,
        priority,
        provider: provider || undefined,
        model,
        dependencies,
        files: files
          .split("\n")
          .map((f) => f.trim())
          .filter(Boolean),
        result,
      });
      const view = await taskApi.get(stored.id);
      if (view) fill(view);
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
      onSaved(stored.id);
    });

  const move = (status: TaskStatus) =>
    run(status, async () => {
      if (!taskId) return;
      await taskApi.status(taskId, status);
      const view = await taskApi.get(taskId);
      if (view) fill(view);
      onSaved(taskId);
    });

  const startSession = () =>
    run("session", async () => {
      if (!taskId) return;
      const started = await taskApi.startSession({
        taskId,
        provider: provider || null,
        model: model || null,
      });
      if (started.sendError) setError(`Sessão aberta, mas a primeira mensagem falhou: ${started.sendError}`);
      onSaved(taskId);
      onOpenSession(started.session.id);
    });

  const suggest = () =>
    run("suggest", async () => {
      const recommendation = await councilApi.recommend({ task: taskText({ title, description }) });
      const best = recommendation.candidates[0];
      if (!best) {
        setSuggestion("O roteador não encontrou um modelo disponível.");
        return;
      }
      setProvider(best.provider);
      setModel(best.model);
      setSuggestion(`Roteador: ${best.providerName} / ${best.modelName} (nota ${Math.round(best.score)})`);
    });

  const toggleDependency = (id: string) =>
    setDependencies((current) =>
      current.includes(id) ? current.filter((d) => d !== id) : [...current, id],
    );

  /** While something it depends on is open, the engine refuses to start it. */
  const waiting = (task?.waitingFor.length ?? 0) > 0;
  const changed =
    task !== null &&
    (title !== task.title ||
      description !== task.description ||
      priority !== task.priority ||
      provider !== (task.provider ?? "") ||
      model !== (task.model ?? "") ||
      result !== task.result ||
      files !== task.files.join("\n") ||
      dependencies.join(",") !== task.dependencies.join(","));

  return (
    <div className="editor task-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">{task ? "Task" : "Nova task"}</span>
        {task && (
          <span className="meta grow ellipsis">
            <span className={`task-status ${task.status.toLowerCase()}`}>
              {TASK_STATUS_LABELS[task.status]}
            </span>
            {` · criada ${formatTime(task.createdAt)}`}
            {task.finishedAt && ` · encerrada ${formatTime(task.finishedAt)}`}
          </span>
        )}
        {task && (
          <button className="button small" onClick={() => onOpenContext(task.id)}>
            Ver contexto
          </button>
        )}
      </div>
      {error && <div className="inline-error">{error}</div>}
      <div className="task-body">
        <section>
          <label className="field">
            <span>Título</span>
            <input
              value={title}
              placeholder="Ex.: Aplicar as retentativas no gateway de pagamento"
              onChange={(e) => setTitle(e.target.value)}
            />
          </label>
          <label className="field">
            <span>Descrição</span>
            <textarea
              rows={4}
              value={description}
              placeholder="O que precisa ser feito, e o que já se sabe."
              onChange={(e) => setDescription(e.target.value)}
            />
          </label>
          <div className="row wrap">
            <label className="row">
              Prioridade
              <select value={priority} onChange={(e) => setPriority(e.target.value as TaskPriority)}>
                {PRIORITIES.map((value) => (
                  <option key={value} value={value}>
                    {TASK_PRIORITY_LABELS[value]}
                  </option>
                ))}
              </select>
            </label>
            <button
              className="button small primary"
              disabled={!ready || busy !== null || !title.trim() || (task !== null && !changed)}
              onClick={() => void save()}
            >
              {saved ? "Salvo" : task ? "Salvar" : "Criar task"}
            </button>
          </div>
        </section>

        {task && (
          <section className="task-step">
            <h3>Estado</h3>
            <p className="meta">
              Daqui, esta task pode ir para os estados abaixo. Ela só inicia quando tudo o que
              espera estiver concluído.
            </p>
            <div className="row wrap">
              {task.can.map((status) => (
                <button
                  key={status}
                  className={`button small${status === "IN_PROGRESS" ? " primary" : ""}`}
                  disabled={!ready || busy !== null || (status === "IN_PROGRESS" && waiting)}
                  title={
                    status === "IN_PROGRESS" && waiting
                      ? "Esta task espera outra terminar"
                      : undefined
                  }
                  onClick={() => void move(status)}
                >
                  {busy === status ? "…" : taskActionLabel(task.status, status)}
                </button>
              ))}
            </div>
            {task.waitingFor.length > 0 && (
              <div className="inline-notice">
                Esta task espera:{" "}
                {task.waitingFor.map((dep) => `"${dep.title}" (${TASK_STATUS_LABELS[dep.status].toLowerCase()})`).join("; ")}
              </div>
            )}
          </section>
        )}

        <section className="task-step">
          <h3>Arquivos</h3>
          <p className="meta">
            Caminhos relevantes, um por linha. Eles entram no contexto da IA como caminhos — nunca o
            conteúdo.
          </p>
          <textarea
            rows={3}
            value={files}
            placeholder="src/api/webhooks.ts"
            onChange={(e) => setFiles(e.target.value)}
          />
        </section>

        {others.length > 0 && (
          <section className="task-step">
            <h3>Dependências</h3>
            <p className="meta">Tasks que precisam terminar antes desta começar.</p>
            <ul className="plain-list task-deps">
              {others.map((other) => (
                <li key={other.id}>
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={dependencies.includes(other.id)}
                      onChange={() => toggleDependency(other.id)}
                    />
                    <span className="ellipsis">{other.title}</span>
                    <span className="meta"> · {TASK_STATUS_LABELS[other.status].toLowerCase()}</span>
                  </label>
                </li>
              ))}
            </ul>
          </section>
        )}

        {task && (
          <section className="task-step">
            <h3>Quem trabalha nesta task</h3>
            <p className="meta">
              A sessão recebe o contexto do projeto montado a partir desta task e começa por ela. A
              execução por agente entra na Fase 8b.
            </p>
            <div className="row wrap">
              <select
                value={provider}
                onChange={(e) => {
                  setProvider(e.target.value);
                  setModel("");
                }}
              >
                <option value="">provider ativo</option>
                {providers.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </select>
              <select value={model} onChange={(e) => setModel(e.target.value)}>
                <option value="">modelo padrão</option>
                {models.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name || m.id}
                  </option>
                ))}
              </select>
              <button
                className="button small"
                disabled={!ready || busy !== null || !title.trim()}
                onClick={() => void suggest()}
              >
                Sugerir com o roteador
              </button>
              <button
                className="button small primary"
                disabled={
                  !ready || busy !== null || waiting || task.status === "DONE" || task.status === "CANCELLED"
                }
                title={waiting ? "Esta task espera outra terminar" : undefined}
                onClick={() => void startSession()}
              >
                {busy === "session" ? "Abrindo…" : "Abrir sessão para esta task"}
              </button>
            </div>
            {suggestion && <div className="meta">{suggestion}</div>}
            {task.sessions.length > 0 && (
              <ul className="plain-list">
                {task.sessions.map((id) => (
                  <li key={id}>
                    <button className="subagent-link" onClick={() => onOpenSession(id)}>
                      Sessão {id.slice(0, 8)}…
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>
        )}

        {task && (
          <section className="task-step">
            <h3>Resultado</h3>
            <p className="meta">O que esta task entregou. Fica no histórico e na busca do projeto.</p>
            <textarea
              rows={3}
              value={result}
              placeholder="Escrito ao concluir."
              onChange={(e) => setResult(e.target.value)}
            />
          </section>
        )}
      </div>
    </div>
  );
}
