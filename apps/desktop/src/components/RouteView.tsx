// Main area: a demand for the Council (ADR-0024). Its members analyze it
// together and one of them writes the plan; the first member carries it out
// with the others as reserves (Sugerir: after the user approves; Full: on
// its own). With the Council off, the router ranks every registered model
// and the user picks one, as can "Só o roteador" in any mode.

import { type KeyboardEvent, useEffect, useState } from "react";
import {
  councilSummary,
  decisionSource,
  formatContext,
  formatPercent,
  formatPricePair,
  memberLabel,
  MODE_HINTS,
  MODE_LABELS,
  parseContext,
  PLAN_SOURCES,
  PREFERENCE_LABELS,
  primaryAction,
  refKey,
  sameRef,
  seatRole,
  votesSummary,
} from "../lib/council";
import { formatDuration } from "../lib/format";
import { councilApi, errorMessage } from "../lib/runtime";
import { formatUsage } from "../lib/transcript";
import type { Council } from "../lib/useCouncil";
import type {
  Activity,
  Analysis,
  Candidate,
  Deliberation,
  ModelRef,
  Preference,
  ProviderInfo,
  Recommendation,
  RouteStarted,
  SessionInfo,
} from "../lib/types";
import { PlayIcon } from "./icons";

interface Props {
  ready: boolean;
  active: boolean;
  council: Council;
  providers: ProviderInfo[];
  projectPath: string | null;
  /** A deliberation from the history to show. */
  initial: Deliberation | null;
  onSessionStarted: (session: SessionInfo) => void;
  onOpenCouncil: () => void;
}

interface Result {
  recommendation: Recommendation;
  /** Absent for a router-only preview. */
  deliberation: Deliberation | null;
}

function ScoreBar({ score }: { score: number }) {
  return (
    <span className="score" title={`nota ${score} de 100`}>
      <span className="score-bar">
        <span style={{ width: `${Math.max(2, Math.min(100, score))}%` }} />
      </span>
      <span className="score-value">{score.toFixed(1)}</span>
    </span>
  );
}

/** One member's analysis, folded. */
function AnalysisCard({ analysis }: { analysis: Analysis }) {
  return (
    <details className="analysis-card" open={analysis.text === null}>
      <summary className="row">
        <strong>{analysis.providerName}</strong>
        <span className="meta mono ellipsis">{analysis.model ?? "padrão"}</span>
        <span className="grow" />
        {analysis.error ? (
          <span className="badge err">sem análise</span>
        ) : (
          <span className="meta">
            {formatUsage(analysis.usage)} · {formatDuration(analysis.durationMs)}
          </span>
        )}
      </summary>
      {analysis.error ? (
        <div className="err-text">{analysis.error}</div>
      ) : (
        <div className="plan-text">{analysis.text}</div>
      )}
    </details>
  );
}

function CandidateRow({
  candidate,
  position,
  shortlisted,
  decided,
  canStart,
  starting,
  onUse,
}: {
  candidate: Candidate;
  position: number;
  shortlisted: boolean;
  decided: boolean;
  canStart: boolean;
  starting: boolean;
  onUse: () => void;
}) {
  return (
    <li className={`candidate ${decided ? "decided" : ""}`}>
      <span className="candidate-rank">{position}</span>
      <div className="grow">
        <div className="row">
          <strong className="ellipsis">{candidate.modelName}</strong>
          <span className="meta ellipsis">
            {candidate.providerName} · {formatPricePair(candidate.inputPrice, candidate.outputPrice)} · ctx{" "}
            {formatContext(candidate.contextWindow)}
          </span>
          {shortlisted && <span className="tag small">no Conselho</span>}
          {decided && <span className="badge ok">decisão</span>}
        </div>
        <div className="chip-list">
          {candidate.reasons.map((reason) => (
            <span key={reason} className="tag small">
              {reason}
            </span>
          ))}
        </div>
      </div>
      <ScoreBar score={candidate.score} />
      <button className="button small" disabled={!canStart || starting} onClick={onUse} title="Iniciar a sessão com este modelo">
        Usar este
      </button>
    </li>
  );
}

export function RouteView({
  ready,
  active,
  council,
  providers,
  projectPath,
  initial,
  onSessionStarted,
  onOpenCouncil,
}: Props) {
  const settings = council.view?.settings ?? null;
  const activities = council.view?.activities ?? [];
  const mode = settings?.mode ?? "off";
  const [task, setTask] = useState(initial?.task ?? "");
  const [activity, setActivity] = useState<Activity | "">("");
  const [preference, setPreference] = useState<Preference | "">("");
  const [tools, setTools] = useState<"" | "yes" | "no">("");
  const [contextText, setContextText] = useState("");
  const [sendTask, setSendTask] = useState<boolean | null>(null);
  const [result, setResult] = useState<Result | null>(
    initial ? { recommendation: initial.recommendation, deliberation: initial } : null,
  );
  const [started, setStarted] = useState<{ outcome: RouteStarted; byCouncil: boolean } | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
  const [busy, setBusy] = useState<"run" | "preview" | "start" | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Opening another deliberation from the history.
  useEffect(() => {
    if (!initial) return;
    setTask(initial.task);
    setResult({ recommendation: initial.recommendation, deliberation: initial });
    setStarted(null);
    setStartError(null);
  }, [initial]);

  const minContext = parseContext(contextText);
  const request = {
    task,
    activity: activity || null,
    preference: preference || null,
    needsTools: tools === "" ? null : tools === "yes",
    minContext: minContext ?? null,
  };
  const noProject = !projectPath;
  const invalid = minContext === undefined || (!task.trim() && !activity);
  const effectiveSend = sendTask ?? settings?.sendTask ?? true;

  const run = async (force = false) => {
    setBusy("run");
    setError(null);
    setStarted(null);
    setStartError(null);
    try {
      const outcome = await councilApi.run({ ...request, force });
      setResult({ recommendation: outcome.deliberation.recommendation, deliberation: outcome.deliberation });
      setStartError(outcome.startError);
      if (outcome.started) {
        setStarted({ outcome: outcome.started, byCouncil: true });
        onSessionStarted(outcome.started.session);
      }
      void council.refreshHistory();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const preview = async () => {
    setBusy("preview");
    setError(null);
    try {
      setResult({ recommendation: await councilApi.recommend(request), deliberation: null });
      setStarted(null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  /** The user approves the plan: the first member carries it out. */
  const execute = async (deliberationId: string) => {
    setBusy("start");
    setError(null);
    setStartError(null);
    try {
      const outcome = await councilApi.execute(deliberationId);
      setStarted({ outcome, byCouncil: false });
      onSessionStarted(outcome.session);
    } catch (e) {
      setStartError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const start = async (choice: ModelRef) => {
    setBusy("start");
    setError(null);
    try {
      const outcome = await councilApi.startSession({
        deliberationId: result?.deliberation?.id ?? null,
        provider: choice.provider,
        model: choice.model,
        task: task.trim() || null,
        sendTask: effectiveSend,
      });
      setStarted({ outcome, byCouncil: false });
      onSessionStarted(outcome.session);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && !invalid && busy === null && ready) {
      event.preventDefault();
      void run();
    }
  };

  const deliberation = result?.deliberation ?? null;
  const decision = deliberation?.decision ?? null;
  const recommendation = result?.recommendation ?? null;
  // The Council analyzed it (ADR-0024); older deliberations voted instead.
  const analyzed = !!deliberation && (deliberation.analyses.length > 0 || deliberation.seats.length > 0);
  const members = settings?.members ?? [];
  // Candidates the Council actually weighed (none in mode Desligado).
  const shortlist = new Set(deliberation && deliberation.votes.length > 0 ? deliberation.shortlist.map(refKey) : []);
  const canStart = ready && !noProject && busy === null;
  const detected = recommendation?.detected
    ? activities.find((a) => a.activity === recommendation.activity)?.label
    : null;

  return (
    <div className="editor route-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">Nova sessão com o Conselho</span>
        <span className="meta grow ellipsis" title={MODE_HINTS[mode]}>
          Modo {MODE_LABELS[mode]} · {MODE_HINTS[mode]}
        </span>
        <button className="button small" onClick={onOpenCouncil}>
          Configurar
        </button>
      </div>
      {noProject && <div className="inline-error">Abra um projeto: a sessão pertence ao projeto.</div>}
      {error && <div className="inline-error">{error}</div>}

      <div className="route-body">
        <section className="route-form">
          <textarea
            className="route-task"
            value={task}
            placeholder={
              mode === "off"
                ? "Descreva a tarefa (ex.: Corrigir o erro de timeout no worker de e-mails). Ctrl+Enter recomenda."
                : "Descreva a demanda (ex.: Corrigir o erro de timeout no worker de e-mails). Ctrl+Enter envia ao Conselho."
            }
            onChange={(e) => setTask(e.target.value)}
            onKeyDown={onKeyDown}
          />
          {mode === "off" ? (
            <div className="route-options">
              <label>
                <span>Atividade</span>
                <select value={activity} onChange={(e) => setActivity(e.target.value as Activity | "")}>
                  <option value="">detectar{detected ? ` (${detected})` : ""}</option>
                  {activities.map((a) => (
                    <option key={a.activity} value={a.activity}>
                      {a.label}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>Preferência</span>
                <select value={preference} onChange={(e) => setPreference(e.target.value as Preference | "")}>
                  <option value="">
                    padrão{settings?.preference ? ` (${PREFERENCE_LABELS[settings.preference]})` : " da atividade"}
                  </option>
                  {(Object.keys(PREFERENCE_LABELS) as Preference[]).map((p) => (
                    <option key={p} value={p}>
                      {PREFERENCE_LABELS[p]}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>Ferramentas</span>
                <select value={tools} onChange={(e) => setTools(e.target.value as "" | "yes" | "no")}>
                  <option value="">conforme a atividade</option>
                  <option value="yes">obrigatórias</option>
                  <option value="no">opcionais</option>
                </select>
              </label>
              <label>
                <span>Contexto mínimo</span>
                <input
                  className={`num ${minContext === undefined ? "invalid" : ""}`}
                  placeholder="ex.: 128k"
                  value={contextText}
                  onChange={(e) => setContextText(e.target.value)}
                />
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={effectiveSend}
                  onChange={(e) => setSendTask(e.target.checked)}
                />
                <span>enviar a tarefa como 1ª mensagem</span>
              </label>
            </div>
          ) : (
            <div className="meta council-line">
              {members.length === 0
                ? "O Conselho não tem membros."
                : members.map((m, i) => (
                    <span key={i} className="tag small">
                      {i + 1}. {memberLabel(m, providers)} · {seatRole(i)}
                    </span>
                  ))}
              <span>
                Só os membros trabalham: cada um analisa com o contexto do projeto, um junta as análises e o 1º
                disponível executa. Se ele falhar, a sessão passa para o próximo.
              </span>
            </div>
          )}
          <div className="row route-actions">
            {mode !== "off" && (
              <button
                className="button"
                disabled={!ready || invalid || busy !== null}
                onClick={() => void preview()}
                title="Ver o ranking de todos os modelos cadastrados e escolher um à mão, sem gastar tokens"
              >
                {busy === "preview" ? "Calculando…" : "Só o roteador (grátis)"}
              </button>
            )}
            <button
              className="button primary"
              disabled={!ready || invalid || busy !== null || (mode === "full" && noProject)}
              onClick={() => void run()}
              title={mode === "full" && noProject ? "Abra um projeto" : undefined}
            >
              {busy === "run"
                ? mode === "off"
                  ? "Calculando…"
                  : "O Conselho está analisando…"
                : primaryAction(mode)}
            </button>
          </div>
        </section>

        {deliberation && analyzed && (
          <section className="decision-card">
            <div className="row">
              <span className="badge ok">{decisionSource(deliberation)}</span>
              <strong className="decision-model">Plano do Conselho</strong>
              <span className="meta">
                {deliberation.plan
                  ? `${PLAN_SOURCES[deliberation.plan.source]}${deliberation.plan.byName ? ` · por ${deliberation.plan.byName}` : ""}`
                  : "sem plano"}
              </span>
              <span className="grow" />
              <span className="meta">{councilSummary(deliberation)}</span>
            </div>
            {deliberation.plan ? (
              <div className="plan-text">{deliberation.plan.text}</div>
            ) : (
              <div className="meta">Nenhum membro analisou: quem executa recebe só a demanda.</div>
            )}
            {deliberation.notices.map((notice) => (
              <div key={notice} className="meta">
                {notice}
              </div>
            ))}
            <div className="meta">
              {deliberation.usage.inputTokens + deliberation.usage.outputTokens > 0 &&
                `Conselho: ${formatUsage(deliberation.usage)} · `}
              {deliberation.savedUsage && `cache economizou ${formatUsage(deliberation.savedUsage)} · `}
              {formatDuration(deliberation.durationMs)}
            </div>

            {deliberation.seats.length > 0 && (
              <>
                <h3>Quem executa</h3>
                <ol className="seat-list">
                  {deliberation.seats.map((seat, i) => (
                    <li key={i} className={i === 0 ? "seat first" : "seat"}>
                      <strong>{seat.providerName}</strong> <span className="meta mono">{seat.modelName}</span>{" "}
                      <span className={`badge ${i === 0 ? "ok" : ""}`}>{seatRole(i)}</span>
                      {seat.demoted && <div className="meta">{seat.demoted}</div>}
                    </li>
                  ))}
                </ol>
              </>
            )}

            {startError && <div className="inline-error">Não foi possível abrir a sessão: {startError}</div>}
            {started ? (
              <div className="inline-notice ok">
                Sessão aberta{started.byCouncil ? " pelo Conselho (Full)" : ""}:{" "}
                <strong>{started.outcome.session.title}</strong> · {started.outcome.session.model}
                {started.outcome.turnId && " · demanda e plano enviados"}
                {started.outcome.skipped.map((skip) => (
                  <div key={skip} className="meta">
                    Não abriu com {skip}
                  </div>
                ))}
                {started.outcome.sendError && (
                  <div className="err-text">Não foi possível enviar a demanda: {started.outcome.sendError}</div>
                )}
                <button className="button small" onClick={() => onSessionStarted(started.outcome.session)}>
                  Abrir sessão
                </button>
              </div>
            ) : (
              <div className="row route-actions">
                {deliberation.seats.length > 0 && (
                  <button
                    className="button primary"
                    disabled={!ready || busy !== null}
                    title="O 1º da fila executa com a demanda e o plano; os outros ficam de reserva"
                    onClick={() => void execute(deliberation.id)}
                  >
                    <PlayIcon />{" "}
                    {busy === "start"
                      ? "Abrindo…"
                      : `${startError ? "Tentar de novo" : "Executar"} com ${deliberation.seats[0]?.providerName ?? "o Conselho"}`}
                  </button>
                )}
                <button className="button" disabled={busy !== null} onClick={() => void run(true)}>
                  Analisar de novo
                </button>
              </div>
            )}

            {deliberation.analyses.length > 0 && (
              <>
                <h3>Análises dos membros</h3>
                {deliberation.analyses.map((analysis, i) => (
                  <AnalysisCard key={i} analysis={analysis} />
                ))}
              </>
            )}
          </section>
        )}

        {deliberation && !analyzed && (
          <section className="decision-card">
            {decision ? (
              <>
                <div className="row">
                  <span className={`badge ${decision.source === "council" ? "ok" : ""}`}>{decisionSource(deliberation)}</span>
                  <strong className="decision-model">{decision.modelName}</strong>
                  <span className="meta">{decision.providerName}</span>
                  <span className="grow" />
                  {deliberation.votes.length > 0 && <span className="meta">{votesSummary(deliberation)}</span>}
                  {decision.agreement !== null && deliberation.votes.length > 1 && (
                    <span className="meta">· concordância {formatPercent(decision.agreement)}</span>
                  )}
                </div>
                <div className="decision-reason">{decision.reason}</div>
              </>
            ) : (
              <strong>Sem decisão</strong>
            )}
            {deliberation.notices.map((notice) => (
              <div key={notice} className="meta">
                {notice}
              </div>
            ))}
            <div className="meta">
              {deliberation.recommendation.detected ? "atividade detectada: " : "atividade: "}
              {activities.find((a) => a.activity === deliberation.recommendation.activity)?.label ??
                deliberation.recommendation.activity}{" "}
              · preferência {PREFERENCE_LABELS[deliberation.recommendation.preference].toLowerCase()} · ferramentas{" "}
              {deliberation.recommendation.needsTools ? "obrigatórias" : "opcionais"}
              {deliberation.usage.inputTokens + deliberation.usage.outputTokens > 0 &&
                ` · Conselho: ${formatUsage(deliberation.usage)}`}
              {deliberation.savedUsage && ` · cache economizou ${formatUsage(deliberation.savedUsage)}`} ·{" "}
              {formatDuration(deliberation.durationMs)}
            </div>
            {started ? (
              <div className="inline-notice ok">
                Sessão aberta: <strong>{started.outcome.session.title}</strong> · {started.outcome.session.model}
                {started.outcome.turnId && " · tarefa enviada"}
                {started.outcome.sendError && (
                  <div className="err-text">Não foi possível enviar a tarefa: {started.outcome.sendError}</div>
                )}
                <button className="button small" onClick={() => onSessionStarted(started.outcome.session)}>
                  Abrir sessão
                </button>
              </div>
            ) : (
              decision && (
                <div className="row route-actions">
                  <button
                    className="button primary"
                    disabled={!canStart}
                    title={noProject ? "Abra um projeto" : undefined}
                    onClick={() => void start(decision)}
                  >
                    <PlayIcon /> {busy === "start" ? "Iniciando…" : "Iniciar sessão"}
                  </button>
                </div>
              )
            )}
          </section>
        )}

        {deliberation && !analyzed && deliberation.votes.length > 0 && (
          <section>
            <h3>Votos (antes da análise em conjunto)</h3>
            <table className="votes-table">
              <thead>
                <tr>
                  <th>Membro</th>
                  <th>Escolha</th>
                  <th>Confiança</th>
                  <th>Motivo</th>
                  <th>Uso</th>
                </tr>
              </thead>
              <tbody>
                {deliberation.votes.map((vote, index) => {
                  const choice = vote.choice ? deliberation.recommendation.candidates.find((c) => sameRef(c, vote.choice)) : null;
                  return (
                    <tr key={index}>
                      <td className="ellipsis" title={`${vote.providerName} / ${vote.model ?? "padrão"}`}>
                        {vote.providerName}
                        <div className="meta mono ellipsis">{vote.model ?? "padrão"}</div>
                      </td>
                      <td className={vote.error ? "err-text" : ""}>
                        {vote.error ? "abstenção" : (choice?.modelName ?? vote.choice?.model)}
                        {sameRef(vote.choice, decision) && !vote.error && " ✓"}
                      </td>
                      <td>{vote.confidence !== null ? formatPercent(vote.confidence) : "—"}</td>
                      <td className={vote.error ? "err-text" : ""}>{vote.error ?? vote.reason ?? "—"}</td>
                      <td className="meta">
                        {vote.usage.inputTokens + vote.usage.outputTokens > 0 ? formatUsage(vote.usage) : "—"} ·{" "}
                        {formatDuration(vote.durationMs)}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </section>
        )}

        {recommendation && !analyzed && (
          <section>
            <h3>
              Ranking do roteador ({recommendation.candidates.length}{" "}
              {recommendation.candidates.length === 1 ? "candidato" : "candidatos"})
            </h3>
            <div className="meta form-hint">
              Nota de 0 a 100 por regras (etiquetas, preço, contexto, ferramentas, perfil do modelo), sem gastar
              tokens. Escolher aqui é à mão: o Conselho só trabalha com os seus membros.
            </div>
            {recommendation.candidates.length === 0 && <div className="meta">Nenhum modelo atende aos requisitos.</div>}
            <ul className="candidate-list">
              {recommendation.candidates.map((candidate, index) => (
                <CandidateRow
                  key={refKey(candidate)}
                  candidate={candidate}
                  position={index + 1}
                  shortlisted={shortlist.has(refKey(candidate))}
                  decided={sameRef(candidate, decision)}
                  canStart={canStart && !started}
                  starting={busy === "start"}
                  onUse={() => void start(candidate)}
                />
              ))}
            </ul>
            {recommendation.excluded.length > 0 && (
              <details className="excluded">
                <summary className="meta">
                  {recommendation.excluded.length} {recommendation.excluded.length === 1 ? "excluído" : "excluídos"}
                </summary>
                <ul>
                  {recommendation.excluded.map((e) => (
                    <li key={refKey(e)}>
                      <span className="mono">{e.model}</span> <span className="meta">({e.providerName})</span> — {e.reason}
                    </li>
                  ))}
                </ul>
              </details>
            )}
          </section>
        )}
      </div>
    </div>
  );
}
