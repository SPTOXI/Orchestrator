// Main area: add or edit an API connection (ADR-0010). Any API can be
// registered: native protocols (OpenAI and compatible, Anthropic, Gemini)
// or a generic profile that describes the HTTP/JSON API.

import { type InputHTMLAttributes, useEffect, useMemo, useState } from "react";
import {
  KIND_LABELS,
  cloneConnection,
  defaultToolMode,
  emptyModel,
  formatPrices,
  mergeModels,
  parseJson,
  slugify,
  toJsonText,
} from "../lib/connections";
import { formatDuration } from "../lib/format";
import { connectionApi, errorMessage } from "../lib/runtime";
import { formatUsage } from "../lib/transcript";
import type {
  ApiKind,
  Connection,
  ConnectionsView,
  CredentialSource,
  GenericProfile,
  ModelEntry,
  TestReport,
  ToolMode,
} from "../lib/types";
import { PlusIcon, TrashIcon } from "./icons";

interface Props {
  ready: boolean;
  active: boolean;
  /** Connection being edited; null for a new one. */
  connectionId: string | null;
  view: ConnectionsView | null;
  onSaved: (id: string) => void;
  onDeleted: () => void;
}

function numberOrNull(text: string): number | null {
  const value = Number(text.replace(",", "."));
  return text.trim() === "" || !Number.isFinite(value) ? null : value;
}

function tagList(text: string): string[] {
  return text
    .split(",")
    .map((t) => t.trim())
    .filter(Boolean);
}

/**
 * Text field over a parsed value (number, tag list): the draft is updated as
 * the user types, but the text shown is only normalized on blur, so "0," or
 * "código," can be typed.
 */
function DraftInput({
  value,
  onCommit,
  ...props
}: { value: string; onCommit: (text: string) => void } & Omit<
  InputHTMLAttributes<HTMLInputElement>,
  "value" | "onChange"
>) {
  const [text, setText] = useState(value);
  const [focused, setFocused] = useState(false);
  useEffect(() => {
    if (!focused) setText(value);
  }, [value, focused]);
  return (
    <input
      {...props}
      value={text}
      onFocus={() => setFocused(true)}
      onBlur={() => setFocused(false)}
      onChange={(e) => {
        setText(e.target.value);
        onCommit(e.target.value);
      }}
    />
  );
}

function TestResult({ report }: { report: TestReport }) {
  const tools: Record<TestReport["tools"], string> = {
    notTested: "não testadas (modo sem ferramentas)",
    passed: "ok — o modelo chamou a ferramenta de teste",
    noCall: "o modelo não chamou a ferramenta",
    failed: "falhou",
  };
  return (
    <div className={report.ok ? "inline-notice ok" : "inline-error"}>
      <strong>{report.ok ? "Conexão funcionando" : "Falha no teste"}</strong> · {formatDuration(report.latencyMs)}
      {report.model && ` · modelo ${report.model}`}
      {report.servedModel && report.servedModel !== report.model && ` (respondido por ${report.servedModel})`}
      {report.error && <div>{report.error}</div>}
      {report.reply !== null && <div>Resposta: “{report.reply}”</div>}
      {report.ok && (
        <div>
          Ferramentas: {tools[report.tools]}
          {report.toolsDetail && ` — ${report.toolsDetail}`}
        </div>
      )}
      {report.usage && <div className="meta">{formatUsage(report.usage)}</div>}
    </div>
  );
}

export function ConnectionEditor({ ready, active, connectionId, view, onSaved, onDeleted }: Props) {
  const existing = view?.connections.find((c) => c.connection.id === connectionId) ?? null;
  const presets = view?.presets ?? [];
  const [draft, setDraft] = useState<Connection | null>(null);
  const [previousId, setPreviousId] = useState<string | null>(null);
  const [idTouched, setIdTouched] = useState(false);
  const [apiKey, setApiKey] = useState("");
  const [clearKey, setClearKey] = useState(false);
  const [headersText, setHeadersText] = useState("");
  const [extraText, setExtraText] = useState("");
  const [genericText, setGenericText] = useState("");
  const [newModel, setNewModel] = useState("");
  const [busy, setBusy] = useState<"save" | "test" | "models" | "delete" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [report, setReport] = useState<TestReport | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const load = (connection: Connection, fromExisting: boolean) => {
    const copy = cloneConnection(connection);
    setDraft(copy);
    setPreviousId(fromExisting ? copy.id : null);
    setIdTouched(fromExisting);
    setHeadersText(toJsonText(copy.headers));
    setExtraText(toJsonText(copy.extraBody));
    setGenericText(copy.generic ? JSON.stringify(copy.generic, null, 2) : "");
    setApiKey("");
    setClearKey(false);
    setReport(null);
    setError(null);
    setNotice(null);
  };

  // Load the saved connection once it is known.
  const existingKey = existing ? JSON.stringify(existing.connection) : null;
  useEffect(() => {
    if (existing && (!draft || previousId === null)) load(existing.connection, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [existingKey]);

  const defaultsByKind = useMemo(() => {
    const byKind = new Map<ApiKind, Connection>();
    for (const preset of presets) {
      if (!byKind.has(preset.connection.kind)) byKind.set(preset.connection.kind, preset.connection);
    }
    return byKind;
  }, [presets]);

  if (!draft && connectionId) {
    return (
      <div className="editor connection-editor" hidden={!active}>
        {view ? (
          <div className="inline-error">Conexão {connectionId} não encontrada.</div>
        ) : (
          <div className="meta pad">Carregando…</div>
        )}
      </div>
    );
  }

  if (!draft) {
    return (
      <div className="editor connection-editor" hidden={!active}>
        <div className="editor-toolbar">
          <span className="profile-title">Adicionar API</span>
          <span className="meta">Escolha um ponto de partida; tudo pode ser editado depois.</span>
        </div>
        <div className="preset-grid">
          {presets.map((preset) => (
            <button key={preset.key} className="preset-card" onClick={() => load(preset.connection, false)}>
              <strong>{preset.label}</strong>
              <span className="meta">{preset.hint}</span>
              <span className="tag small">{KIND_LABELS[preset.connection.kind]}</span>
            </button>
          ))}
        </div>
      </div>
    );
  }

  const update = (patch: Partial<Connection>) => setDraft((d) => (d ? { ...d, ...patch } : d));
  const updateModel = (index: number, patch: Partial<ModelEntry>) =>
    setDraft((d) => (d ? { ...d, models: d.models.map((m, i) => (i === index ? { ...m, ...patch } : m)) } : d));
  const keyStatus = existing?.key ?? null;
  const toolMode = draft.toolMode ?? defaultToolMode(draft.kind);

  /** The draft with JSON fields parsed; throws a readable message. */
  const build = (): Connection => {
    const headers = parseJson(headersText);
    if (!headers.ok) throw new Error(`Headers extras: ${headers.error}`);
    if (headers.value !== null && (typeof headers.value !== "object" || Array.isArray(headers.value)))
      throw new Error("Headers extras: use um objeto JSON, ex.: {\"HTTP-Referer\": \"…\"}");
    const extra = parseJson(extraText);
    if (!extra.ok) throw new Error(`Campos extras do corpo: ${extra.error}`);
    let generic: GenericProfile | null = null;
    if (draft.kind === "generic") {
      const parsed = parseJson(genericText);
      if (!parsed.ok) throw new Error(`Perfil genérico: ${parsed.error}`);
      generic = parsed.value as GenericProfile | null;
    }
    return {
      ...draft,
      headers: (headers.value as Record<string, string> | null) ?? {},
      extraBody: extra.value,
      generic,
    };
  };

  const run = async (kind: NonNullable<typeof busy>, action: () => Promise<void>) => {
    setBusy(kind);
    setError(null);
    setNotice(null);
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
      const connection = build();
      const saved = await connectionApi.save({
        connection,
        apiKey: apiKey.trim() || undefined,
        clearKey,
        previousId: previousId ?? undefined,
      });
      setPreviousId(saved.connection.id);
      setApiKey("");
      setClearKey(false);
      setNotice("Conexão salva e registrada como provider.");
      onSaved(saved.connection.id);
    });

  const test = () =>
    run("test", async () => {
      setReport(null);
      setReport(
        await connectionApi.test({
          connection: build(),
          apiKey: apiKey.trim() || undefined,
          model: draft.defaultModel ?? undefined,
        }),
      );
    });

  const discover = () =>
    run("models", async () => {
      const found = await connectionApi.models({ connection: build(), apiKey: apiKey.trim() || undefined });
      const merged = mergeModels(draft.models, found);
      const added = merged.length - draft.models.length;
      const defaultModel =
        draft.defaultModel && merged.some((m) => m.id === draft.defaultModel)
          ? draft.defaultModel
          : (merged.find((m) => m.enabled)?.id ?? null);
      update({ models: merged, defaultModel });
      setNotice(
        `${found.length} ${found.length === 1 ? "modelo encontrado" : "modelos encontrados"}` +
          (added ? ` · ${added} ${added === 1 ? "novo" : "novos"} na lista` : "") +
          (merged.some((m) => !m.enabled) ? " · ative os que quiser usar" : ""),
      );
    });

  const remove = () =>
    run("delete", async () => {
      if (!previousId) return;
      await connectionApi.remove(previousId);
      onDeleted();
    });

  const changeKind = (kind: ApiKind) => {
    const defaults = defaultsByKind.get(kind);
    update({
      kind,
      toolMode: null,
      baseUrl: previousId ? draft.baseUrl : (defaults?.baseUrl ?? draft.baseUrl),
      generic: kind === "generic" ? (draft.generic ?? defaults?.generic ?? null) : null,
    });
    if (kind === "generic" && !genericText.trim() && defaults?.generic) {
      setGenericText(JSON.stringify(defaults.generic, null, 2));
    }
  };

  const addModel = () => {
    const id = newModel.trim();
    if (!id || draft.models.some((m) => m.id === id)) return;
    update({ models: [...draft.models, emptyModel(id)] });
    setNewModel("");
  };

  const enabledModels = draft.models.filter((m) => m.enabled);

  return (
    <div className="editor connection-editor" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title ellipsis">{previousId ? draft.name : `Nova API · ${draft.name}`}</span>
        <span className="badge">{KIND_LABELS[draft.kind]}</span>
        <span className="grow" />
        <button className="button small" disabled={!ready || busy !== null} onClick={() => void test()}>
          {busy === "test" ? "Testando…" : "Testar conexão"}
        </button>
        <button className="button small primary" disabled={!ready || busy !== null} onClick={() => void save()}>
          {busy === "save" ? "Salvando…" : "Salvar"}
        </button>
        {previousId &&
          (confirmDelete ? (
            <span className="confirm">
              excluir?
              <button className="link" onClick={() => void remove()}>
                sim
              </button>
              <button className="link" onClick={() => setConfirmDelete(false)}>
                não
              </button>
            </span>
          ) : (
            <button className="icon-button" title="Excluir conexão" onClick={() => setConfirmDelete(true)}>
              <TrashIcon />
            </button>
          ))}
      </div>
      {error && <div className="inline-error">{error}</div>}
      {notice && <div className="inline-notice ok">{notice}</div>}
      {report && <TestResult report={report} />}

      <div className="profile connection-form">
        <section>
          <h3>Conexão</h3>
          <label className="form-row">
            <span>Nome</span>
            <input
              value={draft.name}
              onChange={(e) => {
                const name = e.target.value;
                update(idTouched ? { name } : { name, id: slugify(name) });
              }}
            />
          </label>
          <label className="form-row">
            <span>Id (provider)</span>
            <input
              className="mono"
              value={draft.id}
              onChange={(e) => {
                setIdTouched(true);
                update({ id: e.target.value });
              }}
            />
          </label>
          <label className="form-row">
            <span>Tipo</span>
            <select value={draft.kind} onChange={(e) => changeKind(e.target.value as ApiKind)}>
              {(Object.keys(KIND_LABELS) as ApiKind[]).map((kind) => (
                <option key={kind} value={kind}>
                  {KIND_LABELS[kind]}
                </option>
              ))}
            </select>
          </label>
          <label className="form-row">
            <span>URL base</span>
            <input className="mono" value={draft.baseUrl} onChange={(e) => update({ baseUrl: e.target.value })} />
          </label>
          <label className="form-row check-row">
            <span>Ativa</span>
            <input type="checkbox" checked={draft.enabled} onChange={(e) => update({ enabled: e.target.checked })} />
          </label>
        </section>

        <section>
          <h3>Credencial</h3>
          <label className="form-row">
            <span>Origem</span>
            <select
              value={draft.credential.source}
              onChange={(e) =>
                update({ credential: { ...draft.credential, source: e.target.value as CredentialSource } })
              }
            >
              <option value="vault">Cofre do sistema{view?.vault ? ` — ${view.vault}` : ""}</option>
              <option value="env">Variável de ambiente</option>
              <option value="none">Sem chave (ex.: servidor local)</option>
            </select>
          </label>
          {draft.credential.source === "vault" && (
            <>
              <label className="form-row">
                <span>Chave de API</span>
                <input
                  type="password"
                  autoComplete="off"
                  className="mono"
                  value={apiKey}
                  placeholder={
                    keyStatus?.source === "vault" && keyStatus.present
                      ? "•••••• guardada no cofre (vazio = manter)"
                      : "cole a chave aqui"
                  }
                  onChange={(e) => setApiKey(e.target.value)}
                />
              </label>
              {keyStatus?.detail && <div className="meta err-text form-hint">{keyStatus.detail}</div>}
              {keyStatus?.present && (
                <label className="form-row check-row">
                  <span>Remover chave guardada</span>
                  <input type="checkbox" checked={clearKey} onChange={(e) => setClearKey(e.target.checked)} />
                </label>
              )}
              <div className="meta form-hint">
                A chave vai direto para o cofre do sistema: nunca é gravada em arquivo, no histórico ou devolvida à
                interface.
              </div>
            </>
          )}
          {draft.credential.source === "env" && (
            <label className="form-row">
              <span>Variável</span>
              <input
                className="mono"
                placeholder="OPENAI_API_KEY"
                value={draft.credential.envVar ?? ""}
                onChange={(e) => update({ credential: { ...draft.credential, envVar: e.target.value } })}
              />
            </label>
          )}
        </section>

        <section>
          <h3 className="row">
            <span className="grow">
              Modelos ({enabledModels.length} de {draft.models.length} ativos)
            </span>
            <button className="button small" disabled={!ready || busy !== null} onClick={() => void discover()}>
              {busy === "models" ? "Buscando…" : "Buscar modelos"}
            </button>
          </h3>
          <table className="models-table">
            <thead>
              <tr>
                <th title="Usar este modelo">Ativo</th>
                <th title="Modelo padrão das sessões">Padrão</th>
                <th>Id</th>
                <th>Contexto</th>
                <th title="USD por 1M de tokens de entrada">$ entrada</th>
                <th title="USD por 1M de tokens de saída">$ saída</th>
                <th title="Etiquetas livres, usadas pelo roteador e pelo Conselho">Etiquetas</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {draft.models.length === 0 && (
                <tr>
                  <td colSpan={8} className="meta">
                    Nenhum modelo. Use “Buscar modelos” ou adicione pelo id.
                  </td>
                </tr>
              )}
              {draft.models.map((model, index) => (
                <tr key={model.id} className={model.enabled ? "" : "muted"}>
                  <td>
                    <input
                      type="checkbox"
                      checked={model.enabled}
                      onChange={(e) => updateModel(index, { enabled: e.target.checked })}
                    />
                  </td>
                  <td>
                    <input
                      type="radio"
                      name={`default-${draft.id}`}
                      checked={draft.defaultModel === model.id}
                      onChange={() => update({ defaultModel: model.id })}
                    />
                  </td>
                  <td className="mono ellipsis" title={model.name ?? model.id}>
                    {model.id}
                  </td>
                  <td>
                    <DraftInput
                      className="num"
                      value={String(model.contextWindow ?? "")}
                      placeholder="—"
                      onCommit={(text) => updateModel(index, { contextWindow: numberOrNull(text) })}
                    />
                  </td>
                  <td>
                    <DraftInput
                      className="num"
                      value={String(model.inputPrice ?? "")}
                      placeholder="—"
                      onCommit={(text) => updateModel(index, { inputPrice: numberOrNull(text) })}
                    />
                  </td>
                  <td>
                    <DraftInput
                      className="num"
                      value={String(model.outputPrice ?? "")}
                      placeholder="—"
                      onCommit={(text) => updateModel(index, { outputPrice: numberOrNull(text) })}
                    />
                  </td>
                  <td>
                    <DraftInput
                      value={model.tags.join(", ")}
                      placeholder="código, barato…"
                      onCommit={(text) => updateModel(index, { tags: tagList(text) })}
                    />
                  </td>
                  <td>
                    <button
                      className="icon-button small"
                      title="Remover da lista"
                      onClick={() =>
                        update({
                          models: draft.models.filter((_, i) => i !== index),
                          defaultModel: draft.defaultModel === model.id ? null : draft.defaultModel,
                        })
                      }
                    >
                      <TrashIcon />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="row model-add">
            <input
              className="mono grow"
              placeholder="id do modelo (ex.: gpt-…, claude-…, llama3:8b)"
              value={newModel}
              onChange={(e) => setNewModel(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && addModel()}
            />
            <button className="button small" onClick={addModel} disabled={!newModel.trim()}>
              <PlusIcon /> Adicionar modelo
            </button>
          </div>
          {enabledModels.some((m) => formatPrices(m)) && (
            <div className="meta form-hint">
              O custo de cada turno é calculado com esses preços (as APIs não informam preço).
            </div>
          )}
        </section>

        <section>
          <h3>Ferramentas</h3>
          <label className="form-row">
            <span>Modo</span>
            <select value={toolMode} onChange={(e) => update({ toolMode: e.target.value as ToolMode })}>
              {draft.kind !== "generic" && <option value="native">Nativo (chamada de funções da API)</option>}
              <option value="prompt">Por prompt (qualquer modelo de texto)</option>
              <option value="none">Sem ferramentas (só conversa)</option>
            </select>
          </label>
          <label className="form-row">
            <span>Máx. rodadas por turno</span>
            <DraftInput
              className="num"
              value={String(draft.maxToolRounds)}
              onCommit={(text) => update({ maxToolRounds: Math.max(1, Math.round(numberOrNull(text) ?? 1)) })}
            />
          </label>
          <div className="meta form-hint">
            O modelo nunca executa nada sozinho: pede a ferramenta e o Orchestrator executa e registra no histórico. O
            limite de rodadas evita laços infinitos; ao atingi-lo o turno termina com um aviso.
          </div>
        </section>

        {draft.kind === "generic" && (
          <section>
            <h3>Perfil da API genérica</h3>
            <div className="meta form-hint">
              Caminho, autenticação (<code>bearer</code>, <code>header</code>, <code>query</code>, <code>none</code>),
              corpo com marcadores <code>{"{{model}}"}</code>, <code>{"{{messages}}"}</code>, <code>{"{{prompt}}"}</code>,{" "}
              <code>{"{{system}}"}</code>, <code>{"{{maxTokens}}"}</code>, <code>{"{{stream}}"}</code>; streaming{" "}
              <code>none</code>/<code>sse</code>/<code>ndjson</code>; caminhos com pontos para o texto, uso de tokens,
              fim e lista de modelos.
            </div>
            <textarea
              className="mono json-field tall"
              spellCheck={false}
              value={genericText}
              onChange={(e) => setGenericText(e.target.value)}
            />
          </section>
        )}

        <details className="evidence">
          <summary>Avançado</summary>
          <label className="form-row">
            <span>Máx. tokens de saída</span>
            <DraftInput
              className="num"
              placeholder={draft.kind === "anthropic" ? "64000 (padrão)" : "padrão da API"}
              value={String(draft.maxOutputTokens ?? "")}
              onCommit={(text) => {
                const tokens = numberOrNull(text);
                update({ maxOutputTokens: tokens === null ? null : Math.max(1, Math.round(tokens)) });
              }}
            />
          </label>
          {draft.kind === "openai" && (
            <label className="form-row check-row">
              <span>Pedir uso de tokens no streaming</span>
              <input
                type="checkbox"
                checked={draft.options.streamUsage !== false}
                onChange={(e) => update({ options: { ...draft.options, streamUsage: e.target.checked } })}
              />
            </label>
          )}
          {draft.kind === "anthropic" && (
            <>
              <label className="form-row check-row">
                <span>Streaming dos argumentos de ferramentas</span>
                <input
                  type="checkbox"
                  checked={draft.options.eagerToolStreaming !== false}
                  onChange={(e) => update({ options: { ...draft.options, eagerToolStreaming: e.target.checked } })}
                />
              </label>
              <label className="form-row check-row">
                <span>Fallback no servidor se o modelo recusar</span>
                <input
                  type="checkbox"
                  checked={draft.options.refusalFallback !== false}
                  onChange={(e) => update({ options: { ...draft.options, refusalFallback: e.target.checked } })}
                />
              </label>
            </>
          )}
          <label className="form-row top">
            <span>Headers extras (JSON)</span>
            <textarea
              className="mono json-field"
              spellCheck={false}
              placeholder={'{"HTTP-Referer": "https://…"}'}
              value={headersText}
              onChange={(e) => setHeadersText(e.target.value)}
            />
          </label>
          <label className="form-row top">
            <span>Campos extras do corpo (JSON)</span>
            <textarea
              className="mono json-field"
              spellCheck={false}
              placeholder={'{"temperature": 0.2}'}
              value={extraText}
              onChange={(e) => setExtraText(e.target.value)}
            />
          </label>
          <label className="form-row top">
            <span>Notas</span>
            <textarea value={draft.notes ?? ""} onChange={(e) => update({ notes: e.target.value || null })} />
          </label>
        </details>
      </div>
    </div>
  );
}
