// Main area: hand a session's work to another AI (ADR-0013). The packet
// comes from the history (facts) and, if wanted, from the session's AI
// (narrative); the user reviews it, picks who takes over and passes it on.
// The next AI receives the packet in its context, never the conversation.

import { useEffect, useMemo, useState } from "react";
import { emptyPacket, handoffStatusLabel, handoffTask, linesOf, PACKET_LISTS } from "../lib/context";
import { formatTime } from "../lib/format";
import { councilApi, errorMessage, handoffApi } from "../lib/runtime";
import { formatUsage } from "../lib/transcript";
import type { Handoff, HandoffDraft, HandoffPacket, ProviderInfo, SessionInfo, TokenUsage } from "../lib/types";
import type { ContextTabRequest } from "./ContextView";

interface Props {
  ready: boolean;
  active: boolean;
  /** Source session of a new handoff. */
  sessionId: string | null;
  /** A saved handoff. */
  handoffId: string | null;
  sessions: Map<string, SessionInfo>;
  providers: ProviderInfo[];
  onSaved: (handoff: Handoff) => void;
  onOpenSession: (id: string) => void;
  onOpenContext: (request: ContextTabRequest) => void;
}

type Lists = Record<string, string>;

function listsOf(packet: HandoffPacket): Lists {
  return Object.fromEntries(PACKET_LISTS.map(({ key }) => [key, (packet[key] as string[]).join("\n")]));
}

export function HandoffView({
  ready,
  active,
  sessionId,
  handoffId,
  sessions,
  providers,
  onSaved,
  onOpenSession,
  onOpenContext,
}: Props) {
  const [handoff, setHandoff] = useState<Handoff | null>(null);
  const [draft, setDraft] = useState<HandoffDraft | null>(null);
  const [packet, setPacket] = useState<HandoffPacket>(emptyPacket());
  const [lists, setLists] = useState<Lists>(listsOf(emptyPacket()));
  const [askAgent, setAskAgent] = useState(true);
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  const [suggestion, setSuggestion] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [usage, setUsage] = useState<TokenUsage | null>(null);

  const source = sessionId ? (sessions.get(sessionId) ?? null) : null;
  const saved = handoff !== null;
  const accepted = handoff?.status === "accepted";
  const from =
    handoff?.from ??
    draft?.from ??
    (source ? { sessionId: source.id, provider: source.provider, model: source.model, title: source.title } : null);

  useEffect(() => {
    if (!ready || !handoffId) return;
    handoffApi
      .get(handoffId)
      .then((h) => {
        if (!h) return setError("Handoff não encontrado.");
        setHandoff(h);
        setPacket(h.packet);
        setLists(listsOf(h.packet));
      })
      .catch((e) => setError(errorMessage(e)));
  }, [ready, handoffId]);

  // Who takes over: by default another provider than the source's.
  useEffect(() => {
    if (provider || providers.length === 0) return;
    const other = providers.find((p) => p.id !== (from?.provider ?? source?.provider));
    setProvider((other ?? providers[0])?.id ?? "");
  }, [providers, provider, from?.provider, source?.provider]);

  const target = providers.find((p) => p.id === provider) ?? null;
  const models = target?.capabilities.models ?? [];

  const current = useMemo<HandoffPacket>(
    () => ({ ...packet, ...Object.fromEntries(PACKET_LISTS.map(({ key }) => [key, linesOf(lists[key] ?? "")])) }),
    [packet, lists],
  );

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

  const prepare = () =>
    run("prepare", async () => {
      if (!sessionId) return;
      const next = await handoffApi.prepare(sessionId, askAgent);
      setDraft(next);
      setPacket(next.packet);
      setLists(listsOf(next.packet));
      setUsage(next.usage);
    });

  const save = async (): Promise<Handoff | null> => {
    if (handoff) return handoff;
    if (!sessionId) return null;
    const created = await handoffApi.create(sessionId, current, draft?.byAgent ?? false);
    setHandoff(created);
    onSaved(created);
    return created;
  };

  const pass = () =>
    run("pass", async () => {
      const h = await save();
      if (!h || !provider) return;
      const started = await handoffApi.start({ handoffId: h.id, provider, model: model || null });
      setHandoff(started.handoff);
      if (started.sendError) setError(`Sessão aberta, mas a primeira mensagem falhou: ${started.sendError}`);
      onOpenSession(started.session.id);
    });

  const suggest = () =>
    run("suggest", async () => {
      const recommendation = await councilApi.recommend({ task: handoffTask(current) });
      const best = recommendation.candidates[0];
      if (!best) {
        setSuggestion("O roteador não encontrou um modelo disponível.");
        return;
      }
      setProvider(best.provider);
      setModel(best.model);
      setSuggestion(`Roteador: ${best.providerName} / ${best.modelName} (nota ${Math.round(best.score)})`);
    });

  const canEdit = !saved && (draft !== null || !sessionId);
  const text = (key: "goal" | "status" | "nextAction", label: string, placeholder: string) => (
    <label className="field">
      <span>{label}</span>
      <input
        value={packet[key]}
        disabled={!canEdit}
        placeholder={placeholder}
        onChange={(e) => setPacket({ ...packet, [key]: e.target.value })}
      />
    </label>
  );

  return (
    <div className="editor handoff-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">Handoff</span>
        <span className="meta grow ellipsis">
          {from ? `de "${from.title}" (${from.provider}${from.model ? `/${from.model}` : ""})` : ""}
          {handoff && ` · ${handoffStatusLabel(handoff)} · ${formatTime(handoff.createdAt)}`}
        </span>
        {handoff && (
          <button
            className="button small"
            onClick={() => onOpenContext({ handoffId: handoff.id, projectPath: handoff.projectPath })}
          >
            Contexto da nova IA
          </button>
        )}
      </div>
      {error && <div className="inline-error">{error}</div>}
      <div className="handoff-body">
        {!saved && sessionId && (
          <section className="handoff-step">
            <h3>1. Rascunho</h3>
            <p className="meta">
              Arquivos, comandos, testes e erros vêm do histórico da sessão, sem custo. O estado, o que foi feito, o que
              falta e a próxima ação podem ser escritos pela própria IA da sessão, num turno que aparece no transcript.
            </p>
            <div className="row wrap">
              <label className="check">
                <input type="checkbox" checked={askAgent} onChange={(e) => setAskAgent(e.target.checked)} />
                Pedir à IA desta sessão para escrever o resumo
              </label>
              <button
                className="button small primary"
                disabled={!ready || busy !== null || source?.status === "running"}
                onClick={() => void prepare()}
              >
                {busy === "prepare" ? "Gerando…" : draft ? "Gerar de novo" : "Gerar rascunho"}
              </button>
              {source?.status === "running" && <span className="meta">espere o turno em andamento terminar</span>}
            </div>
            {draft && (
              <div className="meta">
                {draft.byAgent ? "Resumo escrito pela IA da sessão" : "Só os fatos do histórico"}
                {usage && ` · custo do resumo: ${formatUsage(usage)}`}
              </div>
            )}
            {draft?.notes.map((note) => (
              <div key={note} className="inline-notice">
                {note}
              </div>
            ))}
          </section>
        )}

        {(draft || saved) && (
          <section className="handoff-step">
            <h3>{saved ? "Pacote" : "2. Revise o pacote"}</h3>
            {text("goal", "Objetivo (goal)", "O que este trabalho entrega")}
            {text("status", "Estado (status)", "Ex.: 50% concluído; bloqueado no webhook")}
            <div className="handoff-lists">
              {PACKET_LISTS.map(({ key, label, hint }) => (
                <label key={key} className="field">
                  <span>
                    {label} <span className="meta">— {hint}, um por linha</span>
                  </span>
                  <textarea
                    rows={3}
                    disabled={!canEdit}
                    value={lists[key] ?? ""}
                    onChange={(e) => setLists({ ...lists, [key]: e.target.value })}
                  />
                </label>
              ))}
            </div>
            {text("nextAction", "Próxima ação (nextAction)", "O primeiro passo concreto da próxima IA")}
          </section>
        )}

        {accepted && handoff?.to && (
          <section className="handoff-step">
            <h3>Assumido</h3>
            <p>
              Por <strong>{handoff.to.title}</strong> ({handoff.to.provider}
              {handoff.to.model ? `/${handoff.to.model}` : ""})
              {handoff.acceptedAt && ` em ${formatTime(handoff.acceptedAt)}`}.
            </p>
            <button className="button small" onClick={() => handoff.to && onOpenSession(handoff.to.sessionId)}>
              Abrir a sessão
            </button>
          </section>
        )}

        {(draft || saved) && !accepted && (
          <section className="handoff-step">
            <h3>{saved ? "Passar adiante" : "3. Quem assume"}</h3>
            <p className="meta">
              Uma sessão nova recebe o pacote no contexto (com a memória do projeto) e uma mensagem para começar pela
              próxima ação. A conversa desta sessão não vai junto.
            </p>
            <div className="row wrap">
              <select
                value={provider}
                onChange={(e) => {
                  setProvider(e.target.value);
                  setModel("");
                }}
              >
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
                disabled={!ready || busy !== null || !current.goal.trim()}
                onClick={() => void suggest()}
              >
                Sugerir com o roteador
              </button>
            </div>
            {suggestion && <div className="meta">{suggestion}</div>}
            <div className="row">
              {!saved && (
                <button
                  className="button small"
                  disabled={!ready || busy !== null || !current.goal.trim()}
                  onClick={() => void run("save", async () => void (await save()))}
                >
                  Salvar para depois
                </button>
              )}
              <button
                className="button small primary"
                disabled={!ready || busy !== null || !provider || !current.goal.trim()}
                onClick={() => void pass()}
              >
                {busy === "pass" ? "Passando…" : `Passar para ${target?.name ?? "a IA escolhida"}`}
              </button>
            </div>
          </section>
        )}
      </div>
    </div>
  );
}
