// Main area: what the Context Builder sends to an AI (ADR-0013) — sections,
// estimated tokens, what the budget left out and the exact text — plus the
// context settings.

import { useCallback, useEffect, useState } from "react";
import { SECTION_LABELS, summaryLine } from "../lib/context";
import { contextApi, errorMessage, taskApi } from "../lib/runtime";
import type { ContextPack, ContextSettings, ContextSettingsView } from "../lib/types";

/** What the tab previews: a session's first message, a handoff, a saved
 * task, or a task typed here. */
export interface ContextTabRequest {
  sessionId?: string;
  task?: string;
  handoffId?: string;
  projectPath?: string;
  /** A task of the project: its own context, built by the engine. */
  taskId?: string;
}

interface Props {
  ready: boolean;
  active: boolean;
  request: ContextTabRequest;
  /** Changes when the tab is reopened with a new request. */
  nonce: number;
}

export function ContextView({ ready, active, request, nonce }: Props) {
  const [task, setTask] = useState(request.task ?? "");
  const [budget, setBudget] = useState("");
  const [pack, setPack] = useState<ContextPack | null>(null);
  const [view, setView] = useState<ContextSettingsView | null>(null);
  const [settings, setSettings] = useState<ContextSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    setTask(request.task ?? "");
    setBudget("");
  }, [nonce, request.task]);

  useEffect(() => {
    if (!ready) return;
    contextApi
      .settings()
      .then((v) => {
        setView(v);
        setSettings(v.settings);
      })
      .catch((e) => setError(errorMessage(e)));
  }, [ready]);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const parsed = Number.parseInt(budget, 10);
      setPack(
        request.taskId
          ? await taskApi.context(request.taskId)
          : await contextApi.preview({
              sessionId: request.sessionId,
              handoffId: request.handoffId,
              projectPath: request.projectPath,
              task,
              budget: Number.isFinite(parsed) ? parsed : undefined,
            }),
      );
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }, [budget, request.handoffId, request.projectPath, request.sessionId, request.taskId, task]);

  useEffect(() => {
    if (!ready || !active) return;
    const timer = setTimeout(() => void refresh(), 400);
    return () => clearTimeout(timer);
  }, [ready, active, refresh, nonce]);

  const saveSettings = async () => {
    if (!settings) return;
    try {
      const next = await contextApi.saveSettings(settings);
      setSettings(next);
      setView((v) => (v ? { ...v, settings: next, warning: null } : v));
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
      void refresh();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const changed =
    settings !== null &&
    view !== null &&
    (settings.autoAttach !== view.settings.autoAttach ||
      settings.budgetTokens !== view.settings.budgetTokens ||
      settings.compaction.auto !== view.settings.compaction.auto ||
      settings.compaction.thresholdTokens !== view.settings.compaction.thresholdTokens ||
      settings.compaction.thresholdPercent !== view.settings.compaction.thresholdPercent);
  const compaction = settings?.compaction;
  const setCompaction = (change: Partial<ContextSettings["compaction"]>) =>
    settings && setSettings({ ...settings, compaction: { ...settings.compaction, ...change } });

  return (
    <div className="editor context-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">Contexto do projeto</span>
        <span className="meta grow ellipsis">
          {pack ? summaryLine(pack) : loading ? "calculando…" : ""}
          {pack?.project && ` · ${pack.project.name}`}
        </span>
        <button className="button small" disabled={!ready || loading} onClick={() => void refresh()}>
          Atualizar
        </button>
      </div>
      {error && <div className="inline-error">{error}</div>}
      <div className="context-body">
        <section className="context-settings">
          <h3>Configuração</h3>
          {view?.warning && <div className="inline-notice">{view.warning}</div>}
          {settings && view && (
            <div className="row wrap">
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.autoAttach}
                  onChange={(e) => setSettings({ ...settings, autoAttach: e.target.checked })}
                />
                Anexar à primeira mensagem de toda sessão nova
              </label>
              <label className="row">
                Orçamento padrão
                <input
                  type="number"
                  className="narrow"
                  min={view.minBudget}
                  max={view.maxBudget}
                  step={100}
                  value={settings.budgetTokens}
                  onChange={(e) => setSettings({ ...settings, budgetTokens: Number(e.target.value) })}
                />
                tokens
              </label>
              <button className="button small primary" disabled={!changed} onClick={() => void saveSettings()}>
                {saved ? "Salvo" : "Salvar"}
              </button>
              <span className="meta">
                de {view.minBudget.toLocaleString("pt-BR")} a {view.maxBudget.toLocaleString("pt-BR")} tokens; cada
                sessão pode desligar o contexto antes da primeira mensagem.
              </span>
            </div>
          )}
          {compaction && (
            <div className="row wrap compaction-settings">
              <label className="check" title="ADR-0018">
                <input
                  type="checkbox"
                  checked={compaction.auto}
                  onChange={(e) => setCompaction({ auto: e.target.checked })}
                />
                Compactar a conversa automaticamente
              </label>
              <label className="row">
                quando passar de
                <input
                  type="number"
                  className="narrow"
                  min={8000}
                  max={2000000}
                  step={10000}
                  disabled={!compaction.auto}
                  value={compaction.thresholdTokens}
                  onChange={(e) => setCompaction({ thresholdTokens: Number(e.target.value) })}
                />
                tokens ou
                <input
                  type="number"
                  className="narrow"
                  min={10}
                  max={95}
                  step={5}
                  disabled={!compaction.auto}
                  value={compaction.thresholdPercent}
                  onChange={(e) => setCompaction({ thresholdPercent: Number(e.target.value) })}
                />
                % da janela do modelo (o que vier primeiro)
              </label>
              <span className="meta">
                A própria IA resume a conversa e o resumo passa a ir no lugar dela; a tela continua com tudo. Cada sessão
                também tem o botão “Compactar”.
              </span>
            </div>
          )}
        </section>

        <section>
          <h3>Prévia</h3>
          <p className="meta">
            O Orchestrator escolhe o que é relevante pelas palavras da tarefa, sem chamar nenhuma IA. Vão só caminhos
            de arquivos, nunca o conteúdo; o resto a IA busca pelas ferramentas de memória.
          </p>
          <label className="field">
            <span>Tarefa</span>
            <textarea
              rows={2}
              value={task}
              placeholder="Ex.: Corrija a validação da assinatura do webhook em src/api/webhooks.ts"
              onChange={(e) => setTask(e.target.value)}
            />
          </label>
          <div className="row wrap">
            <label className="row">
              Orçamento desta prévia
              <input
                type="number"
                className="narrow"
                min={view?.minBudget ?? 300}
                max={view?.maxBudget ?? 8000}
                step={100}
                placeholder={String(settings?.budgetTokens ?? "")}
                value={budget}
                onChange={(e) => setBudget(e.target.value)}
              />
            </label>
            {request.handoffId && <span className="badge">inclui o handoff</span>}
            {request.taskId && <span className="badge">o contexto desta task</span>}
            {request.sessionId && <span className="meta">sessão {request.sessionId.slice(0, 8)}…</span>}
          </div>
          {pack && (pack.omitted.length > 0 || pack.notes.length > 0) && (
            <ul className="plain-list context-notes">
              {pack.omitted.map((o) => (
                <li key={o} className="meta">
                  Fora pelo orçamento: {o.replace(/ \(orçamento\)$/, "")}
                </li>
              ))}
              {pack.notes.map((n) => (
                <li key={n} className="meta">
                  {n}
                </li>
              ))}
            </ul>
          )}
        </section>

        {pack?.sections.map((section) => (
          <section key={section.kind} className="context-section">
            <h3>
              {SECTION_LABELS[section.kind]} <span className="mono meta">{section.title}</span>
              <span className="meta">
                {" "}
                · {section.items.length}
                {section.found > section.items.length ? ` de ${section.found}` : ""}{" "}
                {section.found === 1 ? "item" : "itens"} · ~{section.tokens} tokens
              </span>
            </h3>
            <ul className="context-items">
              {section.items.map((item, i) => (
                <li key={i} className="mono">
                  {item}
                </li>
              ))}
            </ul>
          </section>
        ))}

        {pack && (
          <details className="context-text">
            <summary>Texto exato enviado à IA ({pack.text.length.toLocaleString("pt-BR")} caracteres)</summary>
            <pre className="output">{pack.text}</pre>
          </details>
        )}
      </div>
    </div>
  );
}
