// Council settings (ADR-0011): mode, members (provider + model), shortlist,
// cache, time limit and default preference; plus the recent deliberations.

import { useEffect, useState } from "react";
import {
  councilProviders,
  decisionSource,
  duplicateMembers,
  memberLabel,
  MODE_HINTS,
  MODE_LABELS,
  PREFERENCE_LABELS,
  votesSummary,
} from "../lib/council";
import { formatTime } from "../lib/format";
import { errorMessage } from "../lib/runtime";
import { formatUsage } from "../lib/transcript";
import type { Council } from "../lib/useCouncil";
import type { CouncilMember, CouncilMode, CouncilSettings, Deliberation, Preference, ProviderInfo } from "../lib/types";
import { CloseIcon, PlusIcon } from "./icons";

interface Props {
  ready: boolean;
  active: boolean;
  council: Council;
  providers: ProviderInfo[];
  onOpenDeliberation: (deliberation: Deliberation) => void;
}

const MODES: CouncilMode[] = ["off", "suggest", "full"];

function MemberRow({
  member,
  providers,
  duplicate,
  onChange,
  onRemove,
}: {
  member: CouncilMember;
  providers: ProviderInfo[];
  duplicate: boolean;
  onChange: (member: CouncilMember) => void;
  onRemove: () => void;
}) {
  const provider = providers.find((p) => p.id === member.provider);
  const models = provider?.capabilities.models ?? [];
  return (
    <div className={`member-row ${duplicate ? "duplicate" : ""}`}>
      <select
        value={member.provider}
        onChange={(e) => onChange({ provider: e.target.value, model: null })}
        title="Provider do membro"
      >
        {!provider && <option value={member.provider}>{member.provider} (indisponível)</option>}
        {providers.map((p) => (
          <option key={p.id} value={p.id}>
            {p.name}
          </option>
        ))}
      </select>
      <select
        value={member.model ?? ""}
        onChange={(e) => onChange({ ...member, model: e.target.value || null })}
        title="Modelo do membro"
      >
        <option value="">padrão{provider?.capabilities.defaultModel ? ` (${provider.capabilities.defaultModel})` : ""}</option>
        {member.model && !models.some((m) => m.id === member.model) && (
          <option value={member.model}>{member.model} (não listado)</option>
        )}
        {models.map((m) => (
          <option key={m.id} value={m.id}>
            {m.id}
          </option>
        ))}
      </select>
      <button className="icon-button small" title="Remover membro" onClick={onRemove}>
        <CloseIcon />
      </button>
      {duplicate && <span className="meta err-text">repetido</span>}
    </div>
  );
}

export function CouncilEditor({ ready, active, council, providers, onOpenDeliberation }: Props) {
  const saved = council.view?.settings ?? null;
  const [draft, setDraft] = useState<CouncilSettings | null>(saved);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const eligible = councilProviders(providers);
  const maxMembers = council.view?.maxMembers ?? 5;

  // Start from the stored settings (and follow them until edited).
  const dirty = draft !== null && saved !== null && JSON.stringify(draft) !== JSON.stringify(saved);
  useEffect(() => {
    if (saved && !dirty) setDraft(saved);
  }, [saved]); // `dirty` is read on purpose without re-running on edits.

  if (!draft) {
    return (
      <div className="editor council-editor" hidden={!active}>
        <div className="meta pad">{council.error ?? "Carregando…"}</div>
      </div>
    );
  }

  const update = (patch: Partial<CouncilSettings>) => {
    setDraft({ ...draft, ...patch });
    setNotice(null);
  };
  const duplicates = duplicateMembers(draft.members);
  const needsMembers = draft.mode !== "off" && draft.members.length === 0;

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await council.save(draft);
      setNotice("Configuração salva.");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const addMember = () => {
    const used = new Set(draft.members.map((m) => m.provider));
    const next = eligible.find((p) => !used.has(p.id)) ?? eligible[0];
    if (next) update({ members: [...draft.members, { provider: next.id, model: null }] });
  };

  return (
    <div className="editor council-editor" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">Conselho de IAs</span>
        <span className="meta grow">
          {MODE_LABELS[saved?.mode ?? "off"]} · {saved?.members.length ?? 0}{" "}
          {saved?.members.length === 1 ? "membro (gerenciador)" : "membros"}
          {dirty && " · alterações não salvas"}
        </span>
        {dirty && (
          <button className="button small" disabled={busy} onClick={() => saved && setDraft(saved)}>
            Descartar
          </button>
        )}
        <button
          className="button small primary"
          disabled={!ready || busy || !dirty || duplicates.length > 0 || needsMembers}
          onClick={() => void save()}
        >
          {busy ? "Salvando…" : "Salvar"}
        </button>
      </div>
      {council.view?.warning && <div className="inline-error">{council.view.warning}</div>}
      {error && <div className="inline-error">{error}</div>}
      {notice && <div className="inline-notice ok">{notice}</div>}

      <div className="profile council-form">
        <section>
          <h3>Modo</h3>
          <div className="mode-options">
            {MODES.map((mode) => (
              <label key={mode} className={`mode-option ${draft.mode === mode ? "selected" : ""}`}>
                <input
                  type="radio"
                  name="council-mode"
                  checked={draft.mode === mode}
                  onChange={() => update({ mode })}
                />
                <div>
                  <strong>{MODE_LABELS[mode]}</strong>
                  <div className="meta">{MODE_HINTS[mode]}</div>
                </div>
              </label>
            ))}
          </div>
        </section>

        <section>
          <h3 className="row">
            <span className="grow">
              Membros ({draft.members.length} de {maxMembers})
            </span>
            <button
              className="button small"
              disabled={draft.members.length >= maxMembers || eligible.length === 0}
              onClick={addMember}
            >
              <PlusIcon /> Adicionar membro
            </button>
          </h3>
          {draft.members.length === 0 && (
            <div className="meta form-hint">
              {eligible.length === 0
                ? "Cadastre uma API em AI PROVIDERS para ter membros."
                : "Nenhum membro. Com um membro, ele é o gerenciador; com vários, eles votam."}
            </div>
          )}
          {draft.members.map((member, index) => (
            <MemberRow
              key={index}
              member={member}
              providers={eligible}
              duplicate={duplicates.includes(index)}
              onChange={(next) => update({ members: draft.members.map((m, i) => (i === index ? next : m)) })}
              onRemove={() => update({ members: draft.members.filter((_, i) => i !== index) })}
            />
          ))}
          {needsMembers && (
            <div className="meta err-text form-hint">
              O modo {MODE_LABELS[draft.mode]} precisa de pelo menos um membro.
            </div>
          )}
          <div className="meta form-hint">
            Cada membro recebe só a descrição da tarefa e os dados dos modelos candidatos (preço, contexto,
            etiquetas). Não recebe arquivos, chaves nem ferramentas.
          </div>
        </section>

        <section>
          <h3>Opções</h3>
          <label className="form-row">
            <span>Preferência padrão</span>
            <select
              value={draft.preference ?? ""}
              onChange={(e) => update({ preference: (e.target.value || null) as Preference | null })}
            >
              <option value="">a de cada atividade</option>
              {(Object.keys(PREFERENCE_LABELS) as Preference[]).map((p) => (
                <option key={p} value={p}>
                  {PREFERENCE_LABELS[p]}
                </option>
              ))}
            </select>
          </label>
          <label className="form-row">
            <span>Candidatos para o Conselho</span>
            <input
              type="number"
              className="num"
              min={2}
              max={10}
              value={draft.shortlist}
              onChange={(e) => update({ shortlist: Number(e.target.value) })}
            />
          </label>
          <label className="form-row">
            <span>Cache (minutos)</span>
            <input
              type="number"
              className="num"
              min={0}
              max={1440}
              value={draft.cacheMinutes}
              onChange={(e) => update({ cacheMinutes: Number(e.target.value) })}
            />
          </label>
          <label className="form-row">
            <span>Prazo por membro (s)</span>
            <input
              type="number"
              className="num"
              min={5}
              max={300}
              value={draft.timeoutSecs}
              onChange={(e) => update({ timeoutSecs: Number(e.target.value) })}
            />
          </label>
          <label className="form-row check-row">
            <span>Enviar a tarefa como 1ª mensagem</span>
            <input type="checkbox" checked={draft.sendTask} onChange={(e) => update({ sendTask: e.target.checked })} />
          </label>
          <div className="meta form-hint">
            O roteador escolhe os melhores candidatos sem gastar tokens; só eles vão ao Conselho. Perguntas iguais
            usam o cache (0 = desligado).
          </div>
        </section>

        <section>
          <h3>Deliberações recentes</h3>
          {council.history.length === 0 && <div className="meta">Nenhuma deliberação nesta execução.</div>}
          <ul className="list deliberation-list">
            {council.history.map((d) => (
              <li key={d.id} className="list-item" onClick={() => onOpenDeliberation(d)}>
                <div className="grow">
                  <div className="title ellipsis">{d.task || "(sem descrição)"}</div>
                  <div className="meta ellipsis">
                    {formatTime(d.createdAt)} · {decisionSource(d)}
                    {d.decision ? ` → ${d.decision.modelName} (${d.decision.providerName})` : ""}
                    {d.votes.length > 0 && ` · ${votesSummary(d)}`}
                    {d.usage.inputTokens + d.usage.outputTokens > 0 && ` · ${formatUsage(d.usage)}`}
                  </div>
                </div>
              </li>
            ))}
          </ul>
          <div className="meta form-hint">
            Todas ficam no HISTORY (COUNCIL_DELIBERATED e ROUTE_DECIDED). Membros atuais:{" "}
            {saved && saved.members.length > 0 ? saved.members.map((m) => memberLabel(m, providers)).join("; ") : "nenhum"}.
          </div>
        </section>
      </div>
    </div>
  );
}
