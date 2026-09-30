// Autonomy tab (ADR-0016): the mode of the project, the requests waiting
// for the user, what was allowed for a session, the rules of each mode and
// "Experimentar". Only the user changes any of this; no AI tool reaches it.

import { useEffect, useMemo, useRef, useState } from "react";
import {
  DECISION_LABELS,
  EMPTY_DRAFT,
  MODE_LABELS,
  MODE_ORDER,
  MODE_SUMMARY,
  describeRule,
  fromDraft,
  grantLabel,
  moveItem,
  requester,
  sameRules,
  toDraft,
  trialArgs,
  waitedFor,
  type RuleDraft,
} from "../lib/autonomy";
import { appApi, autonomyApi, errorMessage } from "../lib/runtime";
import type { Autonomy } from "../lib/useAutonomy";
import type {
  ApprovalAnswer,
  ApprovalView,
  AutonomyMode,
  PolicyRule,
  RuleDecision,
  Trial,
} from "../lib/types";

/** Tools the AIs have besides the runtime's (ADR-0013, ADR-0015). */
const ENGINE_TOOLS = [
  "memory.working",
  "memory.search",
  "memory.list",
  "memory.save",
  "decision.list",
  "decision.save",
  "agent.finish",
  "agent.delegate",
];

interface Props {
  active: boolean;
  ready: boolean;
  projectName: string | null;
  autonomy: Autonomy;
}

export function AutonomyView({ active, ready, projectName, autonomy }: Props) {
  const { overview, pending } = autonomy;
  const [busy, setBusy] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [confirmUnrestricted, setConfirmUnrestricted] = useState(false);
  const [drafts, setDrafts] = useState<RuleDraft[]>([]);
  const [tools, setTools] = useState<string[]>(ENGINE_TOOLS);
  const [now, setNow] = useState(() => Date.now());

  // The editor starts from what is saved, and follows it while the user has
  // not touched it.
  const saved = overview?.rules ?? null;
  const draftRules = useMemo(() => drafts.map(fromDraft), [drafts]);
  const previous = useRef<PolicyRule[] | null>(null);
  useEffect(() => {
    if (!saved) return;
    const before = previous.current;
    previous.current = saved;
    setDrafts((current) =>
      before === null || sameRules(current.map(fromDraft), before) ? saved.map(toDraft) : current,
    );
  }, [saved]);
  const dirty = saved !== null && !sameRules(draftRules, saved);

  useEffect(() => {
    if (!active) return;
    appApi
      .tools()
      .then((specs) => setTools([...specs.map((s) => s.name), ...ENGINE_TOOLS]))
      .catch(() => undefined);
    const tick = setInterval(() => setNow(Date.now()), 5000);
    return () => clearInterval(tick);
  }, [active]);

  const run = async (label: string, action: () => Promise<void>) => {
    setBusy(label);
    setFailure(null);
    try {
      await action();
    } catch (err) {
      setFailure(errorMessage(err));
    } finally {
      setBusy(null);
    }
  };

  const choose = (mode: AutonomyMode) => {
    if (mode === "unrestricted" && overview?.mode !== "unrestricted") {
      setConfirmUnrestricted(true);
      return;
    }
    setConfirmUnrestricted(false);
    void run(`mode:${mode}`, () => autonomy.setMode(mode));
  };

  const update = (index: number, patch: Partial<RuleDraft>) =>
    setDrafts((current) => current.map((d, i) => (i === index ? { ...d, ...patch } : d)));

  if (!overview) {
    return (
      <div className="editor autonomy-view" hidden={!active}>
        <div className="editor-toolbar">
          <span className="profile-title">Autonomia</span>
        </div>
        {autonomy.error ? (
          <div className="inline-error">{autonomy.error}</div>
        ) : (
          <p className="meta pad">Carregando…</p>
        )}
      </div>
    );
  }

  const scope = projectName ?? "projetos sem modo próprio";

  return (
    <div className="editor autonomy-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">Autonomia</span>
        <span className="meta grow ellipsis">
          {projectName ? `Projeto ${projectName}` : "Nenhum projeto aberto: vale o modo padrão"}
          {" · "}
          {MODE_LABELS[overview.mode]}
          {overview.pausedAll && " · IAs pausadas"}
        </span>
        <button
          className={`button small${overview.pausedAll ? " primary" : ""}`}
          disabled={!ready || busy !== null}
          onClick={() =>
            void run("pause", overview.pausedAll ? autonomy.resumeAll : autonomy.pauseAll)
          }
          title={
            overview.pausedAll
              ? "As IAs voltam a agir; a fila de agentes anda"
              : "Toda chamada de ferramenta de qualquer IA espera; nenhum agente começa turno novo"
          }
        >
          {overview.pausedAll ? "Retomar IAs" : "Pausar IAs"}
        </button>
      </div>
      {overview.warning && <div className="inline-notice">{overview.warning}</div>}
      {(failure || autonomy.error) && <div className="inline-error">{failure ?? autonomy.error}</div>}
      <div className="task-body">
        <section>
          <h3>{projectName ? `Modo de ${projectName}` : "Modo padrão"}</h3>
          <p className="meta">
            O que as IAs deste projeto podem fazer sem perguntar. Vale para as sessões e para os
            agentes; ao pôr um agente para trabalhar, dá para escolher outro modo só para ele.
          </p>
          <div className="mode-cards">
            {MODE_ORDER.map((mode) => (
              <button
                key={mode}
                className={`mode-card ${mode}${overview.mode === mode ? " selected" : ""}`}
                disabled={!ready || busy !== null}
                onClick={() => choose(mode)}
                aria-pressed={overview.mode === mode}
              >
                <span className="mode-name">{MODE_LABELS[mode]}</span>
                <span className="mode-summary">{MODE_SUMMARY[mode]}</span>
                {overview.mode === mode && <span className="mode-current">em uso</span>}
              </button>
            ))}
          </div>
          {confirmUnrestricted && (
            <div className="unrestricted-confirm">
              <p>
                <strong>Acesso Irrestrito</strong> significa que o Orchestrator não impõe nenhuma
                política às IAs de {scope}: elas leem, criam, alteram e excluem arquivos, executam
                comandos e scripts, instalam dependências, usam o Git (inclusive push), criam
                subagentes e mexem fora da pasta do projeto — sem pedir autorização.
              </p>
              <p className="meta">
                Continua valendo: tudo registrado no histórico; as travas de arquivo entre os seus
                agentes; os limites de turnos e de paralelismo; e os botões Pausar e Parar.
              </p>
              <div className="row wrap">
                <button
                  className="button danger"
                  disabled={!ready || busy !== null}
                  onClick={() => {
                    setConfirmUnrestricted(false);
                    void run("mode:unrestricted", () => autonomy.setMode("unrestricted"));
                  }}
                >
                  Conceder Acesso Irrestrito{projectName ? ` a ${projectName}` : ""}
                </button>
                <button className="button" onClick={() => setConfirmUnrestricted(false)}>
                  Cancelar
                </button>
              </div>
            </div>
          )}
          {projectName && (
            <p className="meta">
              {overview.projectMode
                ? `Escolhido para este projeto. O padrão para os outros é ${MODE_LABELS[overview.defaultMode]}. `
                : `Este projeto usa o modo padrão (${MODE_LABELS[overview.defaultMode]}). `}
              {overview.projectMode && (
                <button
                  className="subagent-link"
                  disabled={!ready || busy !== null}
                  onClick={() => void run("mode:default", () => autonomy.setMode(null))}
                >
                  Voltar ao modo padrão
                </button>
              )}
            </p>
          )}
          <label className="field inline-field">
            <span>Modo padrão (projetos sem modo próprio)</span>
            <select
              value={overview.defaultMode}
              disabled={!ready || busy !== null}
              onChange={(e) =>
                void run("default", () => autonomy.setDefault(e.target.value as AutonomyMode))
              }
            >
              {MODE_ORDER.map((mode) => (
                <option key={mode} value={mode}>
                  {MODE_LABELS[mode]}
                </option>
              ))}
            </select>
          </label>
        </section>

        <section className="task-step">
          <h3>
            Pedidos de autorização <span className="meta">{pending.length}</span>
          </h3>
          {pending.length === 0 ? (
            <p className="meta">Nenhuma IA esperando por você.</p>
          ) : (
            <ul className="plain-list approvals">
              {pending.map((request) => (
                <ApprovalCard
                  key={request.id}
                  request={request}
                  now={now}
                  disabled={!ready || busy !== null}
                  onAnswer={(answer, note) =>
                    run(`answer:${request.id}`, () => autonomy.answer(request.id, answer, note))
                  }
                />
              ))}
            </ul>
          )}
        </section>

        {overview.grants.length > 0 && (
          <section className="task-step">
            <h3>Liberado nesta sessão</h3>
            <p className="meta">
              O que você permitiu "nesta sessão" não pergunta de novo enquanto ela existir. Mudar
              o modo ou salvar as regras apaga as liberações.
            </p>
            <ul className="plain-list">
              {overview.grants.map((grant) => (
                <li key={grant.id} className="list-item">
                  <div className="grow">
                    <div className="ellipsis">{grantLabel(grant)}</div>
                    <div className="meta ellipsis">
                      {grant.agentTitle ? `Agente "${grant.agentTitle}"` : `Sessão ${grant.sessionId.slice(0, 8)}…`}
                    </div>
                  </div>
                  <button
                    className="button small"
                    disabled={!ready || busy !== null}
                    onClick={() => void run(`revoke:${grant.id}`, () => autonomy.revoke(grant.id))}
                  >
                    Revogar
                  </button>
                </li>
              ))}
            </ul>
          </section>
        )}

        <section className="task-step">
          <h3>Regras do modo Assistido</h3>
          <p className="meta">Fixas. A primeira que casa decide.</p>
          <ol className="rule-list">
            {overview.assistedRules.map((rule, i) => (
              <li key={i}>
                <code>{describeRule(rule)}</code>
                {rule.note && <span className="meta"> — {rule.note}</span>}
              </li>
            ))}
          </ol>
        </section>

        <section className="task-step">
          <h3>Regras do modo Autônomo {dirty && <span className="badge waiting">não salvas</span>}</h3>
          <p className="meta">
            A primeira regra que casa decide; se nenhuma casar, o Orchestrator pergunta. Um comando
            composto (<code>;</code>, <code>&&</code>, <code>|</code>) é julgado por partes e vale a
            decisão mais restritiva; padrões de comando não valem para comandos com{" "}
            <code>$(…)</code> ou crases. As regras julgam o que a ferramenta recebe, não o que um
            script faz depois: não são uma sandbox.
          </p>
          <div className="rules-editor">
            <div className="rule-row rule-head">
              <span>#</span>
              <span>Ferramentas</span>
              <span>Tipo</span>
              <span title="Dentro ou fora da pasta do projeto">Onde</span>
              <span>Comando</span>
              <span>Caminho</span>
              <span>Decisão</span>
              <span>Nota</span>
              <span />
            </div>
            {drafts.map((draft, i) => (
              <div key={i} className={`rule-row decision-${draft.decision}`}>
                <span className="meta">{i + 1}</span>
                <input
                  list="autonomy-tools"
                  value={draft.tools}
                  placeholder="todas"
                  title="Nomes separados por vírgula: git.push, git.* ou *. Vazio: todas."
                  onChange={(e) => update(i, { tools: e.target.value })}
                />
                <select
                  value={draft.access}
                  onChange={(e) => update(i, { access: e.target.value as RuleDraft["access"] })}
                >
                  <option value="">todas</option>
                  <option value="read">consulta</option>
                  <option value="write">ação</option>
                </select>
                <select
                  value={draft.where}
                  onChange={(e) => update(i, { where: e.target.value as RuleDraft["where"] })}
                >
                  <option value="">tudo</option>
                  <option value="inside">dentro</option>
                  <option value="outside">fora</option>
                </select>
                <input
                  value={draft.command}
                  placeholder="qualquer"
                  title="Padrão do comando (shell.execute, process.start, terminal.write); * vale qualquer coisa. Ex.: rm *"
                  onChange={(e) => update(i, { command: e.target.value })}
                />
                <input
                  value={draft.path}
                  placeholder="qualquer"
                  title="Padrão de caminho como no .gitignore: .env*, src/**"
                  onChange={(e) => update(i, { path: e.target.value })}
                />
                <select
                  value={draft.decision}
                  onChange={(e) => update(i, { decision: e.target.value as RuleDecision })}
                >
                  {(["allow", "ask", "deny"] as RuleDecision[]).map((d) => (
                    <option key={d} value={d}>
                      {DECISION_LABELS[d]}
                    </option>
                  ))}
                </select>
                <input
                  value={draft.note}
                  placeholder="por quê"
                  onChange={(e) => update(i, { note: e.target.value })}
                />
                <span className="rule-actions">
                  <button
                    className="icon-button"
                    title="Subir"
                    disabled={i === 0}
                    onClick={() => setDrafts((d) => moveItem(d, i, -1))}
                  >
                    ↑
                  </button>
                  <button
                    className="icon-button"
                    title="Descer"
                    disabled={i === drafts.length - 1}
                    onClick={() => setDrafts((d) => moveItem(d, i, 1))}
                  >
                    ↓
                  </button>
                  <button
                    className="icon-button"
                    title="Remover"
                    onClick={() => setDrafts((d) => d.filter((_, j) => j !== i))}
                  >
                    ✕
                  </button>
                </span>
              </div>
            ))}
            <datalist id="autonomy-tools">
              <option value="*" />
              {[...new Set(tools.map((t) => `${t.split(".")[0]}.*`))].map((group) => (
                <option key={group} value={group} />
              ))}
              {tools.map((tool) => (
                <option key={tool} value={tool} />
              ))}
            </datalist>
          </div>
          <div className="row wrap">
            <button
              className="button small"
              onClick={() => setDrafts((d) => [...d, { ...EMPTY_DRAFT }])}
            >
              Adicionar regra
            </button>
            <button
              className="button small"
              disabled={!dirty}
              onClick={() => setDrafts((overview.rules ?? []).map(toDraft))}
            >
              Descartar alterações
            </button>
            <button
              className="button small"
              disabled={!ready || busy !== null}
              onClick={() => void run("reset", autonomy.resetRules)}
              title="Volta às regras com que o Orchestrator vem"
            >
              Restaurar padrão
            </button>
            <button
              className="button small primary"
              disabled={!ready || busy !== null || !dirty}
              onClick={() => void run("rules", () => autonomy.saveRules(draftRules))}
            >
              {busy === "rules" ? "Salvando…" : "Salvar regras"}
            </button>
          </div>
        </section>

        <TrySection
          ready={ready}
          projectId={overview.projectId}
          currentMode={overview.mode}
          draft={draftRules}
        />
      </div>
    </div>
  );
}

function ApprovalCard({
  request,
  now,
  disabled,
  onAnswer,
}: {
  request: ApprovalView;
  now: number;
  disabled: boolean;
  onAnswer: (answer: ApprovalAnswer, note: string | null) => Promise<void>;
}) {
  const [note, setNote] = useState("");
  return (
    <li className="approval-card">
      <div className="approval-head">
        <strong>{requester(request)}</strong>
        <span className="meta"> {waitedFor(request.requestedAt, now)}</span>
      </div>
      <div className="approval-summary">{request.summary}</div>
      <div className="meta">{request.reason}</div>
      {request.detail && (
        <details>
          <summary className="meta">Detalhes da chamada ({request.tool})</summary>
          <pre className="approval-detail">{request.detail}</pre>
        </details>
      )}
      <input
        className="approval-note"
        value={note}
        placeholder="Motivo, se for negar (a IA recebe)"
        onChange={(e) => setNote(e.target.value)}
      />
      <div className="row wrap">
        <button className="button small primary" disabled={disabled} onClick={() => void onAnswer("approve", null)}>
          Permitir
        </button>
        <button
          className="button small"
          disabled={disabled}
          title={`Não perguntar de novo nesta sessão por ${request.tool}${request.command ? ` \`${request.command}\`` : ""} (pela mesma regra)`}
          onClick={() => void onAnswer("approveSession", null)}
        >
          Permitir nesta sessão
        </button>
        <button
          className="button small danger"
          disabled={disabled}
          onClick={() => void onAnswer("deny", note.trim() || null)}
        >
          Negar
        </button>
      </div>
    </li>
  );
}

function TrySection({
  ready,
  projectId,
  currentMode,
  draft,
}: {
  ready: boolean;
  projectId: string | null;
  currentMode: AutonomyMode;
  draft: PolicyRule[];
}) {
  const [tool, setTool] = useState("shell.execute");
  const [target, setTarget] = useState("npm test && rm -rf dist");
  const [mode, setMode] = useState<AutonomyMode>(currentMode === "unrestricted" ? "autonomous" : currentMode);
  const [trial, setTrial] = useState<Trial | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Follows the project's mode when it changes (Unrestricted evaluates
  // nothing, so the rules worth trying are the Autonomous ones).
  useEffect(() => {
    setMode(currentMode === "unrestricted" ? "autonomous" : currentMode);
    setTrial(null);
  }, [currentMode]);

  const tryIt = async () => {
    setError(null);
    try {
      setTrial(
        await autonomyApi.tryCall({
          projectId,
          mode,
          rules: mode === "autonomous" ? draft : null,
          tool,
          args: trialArgs(tool, target),
        }),
      );
    } catch (err) {
      setTrial(null);
      setError(errorMessage(err));
    }
  };

  return (
    <section className="task-step">
      <h3>Experimentar</h3>
      <p className="meta">
        Veja qual regra decide uma chamada, com o mesmo código que o gate usa. No Autônomo vale o
        que está no editor, mesmo antes de salvar.
      </p>
      <div className="row wrap">
        <select className="trial-mode" value={mode} onChange={(e) => setMode(e.target.value as AutonomyMode)}>
          {MODE_ORDER.map((m) => (
            <option key={m} value={m}>
              {MODE_LABELS[m]}
            </option>
          ))}
        </select>
        <input list="autonomy-tools" value={tool} onChange={(e) => setTool(e.target.value)} />
        <input
          className="grow"
          value={target}
          placeholder="comando ou caminho"
          onChange={(e) => setTarget(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && void tryIt()}
        />
        <button className="button small" disabled={!ready || !tool.trim()} onClick={() => void tryIt()}>
          Experimentar
        </button>
      </div>
      {error && <div className="inline-error">{error}</div>}
      {trial && (
        <div className={`trial decision-${trial.decision}`}>
          <strong>{DECISION_LABELS[trial.decision]}</strong>
          <span className="meta"> — {trial.reason}</span>
          {trial.unknownTool && (
            <div className="meta">
              {tool} não é uma ferramenta conhecida: a chamada falharia de qualquer jeito.
            </div>
          )}
          {trial.opaque && (
            <div className="meta">O comando tem substituição: padrões de comando não valem para ele.</div>
          )}
          {trial.targets.length > 1 && (
            <ul className="plain-list">
              {trial.targets.map((t, i) => (
                <li key={i} className="meta">
                  {t.command ? `\`${t.command}\`` : t.path}
                  {t.command && ` em ${t.path}`}
                  {!t.inside && " (fora do projeto)"} → {DECISION_LABELS[t.decision].toLowerCase()}
                  {t.rule ? ` (regra ${t.rule})` : " (nenhuma regra)"}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}
