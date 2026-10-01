// "Tokens e custo" tab (ADR-0018): what the AIs spent — today, in 7 or 30
// days, in the open project or in all of them — by provider and model,
// with what came from the prompt cache, what it saved and how many
// conversations were compacted. Read from the history; nothing is stored
// twice.

import { useCallback, useEffect, useRef, useState } from "react";
import { cacheRatio, SPEND_PERIODS, spendLabel } from "../lib/cost";
import { auditEvents } from "../lib/events";
import { formatUsd } from "../lib/format";
import { costApi, errorMessage } from "../lib/runtime";
import type { SpendReport } from "../lib/types";

interface Props {
  active: boolean;
  ready: boolean;
  projectName: string | null;
  providerName: (id: string) => string;
  onOpenAgents: () => void;
}

const REFRESHING = new Set(["TURN_COMPLETED", "COUNCIL_DELIBERATED", "CONTEXT_COMPACTED", "PROJECT_OPENED"]);

export function CostView({ active, ready, projectName, providerName, onOpenAgents }: Props) {
  const [days, setDays] = useState(1);
  const [all, setAll] = useState(false);
  const [report, setReport] = useState<SpendReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const load = useCallback(async () => {
    if (!ready) return;
    try {
      setReport(await costApi.report(days, all));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [ready, days, all]);

  useEffect(() => {
    if (!active) return;
    void load();
    const unsubscribe = auditEvents.subscribe((event) => {
      if (!REFRESHING.has(event.kind)) return;
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => void load(), 500);
    });
    return () => {
      unsubscribe();
      if (timer.current) clearTimeout(timer.current);
    };
  }, [active, load]);

  const tokens = (n: number) => n.toLocaleString("pt-BR");
  const share = report ? cacheRatio(report.cachedInputTokens, report.inputTokens) : null;

  return (
    <div className="editor cost-view" hidden={!active}>
      <div className="editor-toolbar">
        <span className="profile-title">Tokens e custo</span>
        <span className="meta grow ellipsis">{all ? "todos os projetos" : (projectName ?? "nenhum projeto aberto")}</span>
        <div className="segmented">
          {SPEND_PERIODS.map((period) => (
            <button
              key={period.days}
              className={`button small${period.days === days ? " primary" : ""}`}
              onClick={() => setDays(period.days)}
            >
              {period.label}
            </button>
          ))}
        </div>
        <label className="check">
          <input type="checkbox" checked={all} onChange={(e) => setAll(e.target.checked)} />
          Todos os projetos
        </label>
        <button className="button small" disabled={!ready} onClick={() => void load()}>
          Atualizar
        </button>
      </div>
      {error && <div className="inline-error">{error}</div>}
      <div className="task-body">
        {report && (
          <>
            <section>
              <h3>{spendLabel(days)}</h3>
              <table className="cost-summary">
                <tbody>
                  <tr>
                    <th>Gasto</th>
                    <td>
                      <strong>{formatUsd(report.costUsd)}</strong>
                      {report.unpriced > 0 && (
                        <span className="meta">
                          {" "}
                          · {report.unpriced} {report.unpriced === 1 ? "chamada" : "chamadas"} sem preço: o gasto real é
                          maior (informe o preço do modelo na conexão)
                        </span>
                      )}
                    </td>
                  </tr>
                  <tr>
                    <th>Cache de prompt</th>
                    <td>
                      {share === null ? (
                        "—"
                      ) : (
                        <>
                          {share}% da entrada veio do cache ({tokens(report.cachedInputTokens)} tokens) ·{" "}
                          {report.cacheSavedUsd >= 0 ? "economizou" : "custou"} {formatUsd(Math.abs(report.cacheSavedUsd))}
                        </>
                      )}
                    </td>
                  </tr>
                  <tr>
                    <th>Tokens</th>
                    <td>
                      {tokens(report.inputTokens)} de entrada · {tokens(report.outputTokens)} de saída ·{" "}
                      {tokens(report.calls)} {report.calls === 1 ? "chamada" : "chamadas"}
                    </td>
                  </tr>
                  <tr>
                    <th>Compactações</th>
                    <td>
                      {report.compactions === 0
                        ? "nenhuma conversa compactada"
                        : `${report.compactions} ${report.compactions === 1 ? "conversa compactada" : "conversas compactadas"}`}
                    </td>
                  </tr>
                </tbody>
              </table>
            </section>
            <section>
              <h3>Por provider e modelo</h3>
              {report.rows.length === 0 ? (
                <p className="meta">Nenhuma chamada de IA neste período.</p>
              ) : (
                <table className="cost-table">
                  <thead>
                    <tr>
                      <th>Provider</th>
                      <th>Modelo</th>
                      <th>Chamadas</th>
                      <th>Entrada</th>
                      <th title="Parte da entrada lida do cache de prompt">Do cache</th>
                      <th>Saída</th>
                      <th>Custo</th>
                      <th title="Quanto o cache economizou (negativo: gravou e não leu de volta)">Economia</th>
                    </tr>
                  </thead>
                  <tbody>
                    {report.rows.map((row) => (
                      <tr key={`${row.provider}/${row.model ?? ""}`}>
                        <td className="ellipsis">{row.provider === "conselho" ? "Conselho" : providerName(row.provider)}</td>
                        <td className="mono ellipsis">{row.model ?? "—"}</td>
                        <td className="num">{tokens(row.calls)}</td>
                        <td className="num">{tokens(row.inputTokens)}</td>
                        <td className="num">
                          {cacheRatio(row.cachedInputTokens, row.inputTokens) ?? "—"}
                          {cacheRatio(row.cachedInputTokens, row.inputTokens) !== null && "%"}
                        </td>
                        <td className="num">{tokens(row.outputTokens)}</td>
                        <td className="num">
                          {formatUsd(row.costUsd)}
                          {row.unpriced > 0 && <span className="meta" title="Chamadas sem preço"> +?</span>}
                        </td>
                        <td className="num">{row.cacheSavedUsd ? formatUsd(row.cacheSavedUsd) : "—"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </section>
            <section>
              <h3>Como o Orchestrator economiza</h3>
              <ul className="plain-list meta">
                <li>
                  O contexto do projeto vai uma vez, com orçamento, e só com caminhos de arquivos; o resto a IA busca
                  quando precisa.
                </li>
                <li>
                  Instruções, ferramentas e conversa repetem o mesmo começo a cada mensagem, marcado para o cache do
                  fornecedor: a leitura custa uma fração da entrada.
                </li>
                <li>
                  Conversas longas são compactadas pela própria IA (aba Contexto); cada sessão tem o botão “Compactar”.
                </li>
                <li>
                  Agentes têm teto de turnos e de custo, e o projeto pode ter um orçamento diário.{" "}
                  <button className="link" onClick={onOpenAgents}>
                    Configurar agentes
                  </button>
                </li>
              </ul>
            </section>
          </>
        )}
      </div>
    </div>
  );
}
