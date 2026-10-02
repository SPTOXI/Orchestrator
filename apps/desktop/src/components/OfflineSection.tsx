// "Modelos offline" (ADR-0021): AI models that run on this computer
// through Ollama — no internet, no key, no cost per token. Install Ollama,
// download models, and turn them into a connection the sessions can use.

import { useCallback, useEffect, useState } from "react";
import { offlineEvents } from "../lib/events";
import { formatBytes } from "../lib/format";
import { errorMessage, githubApi, offlineApi } from "../lib/runtime";
import { terminalRequests } from "../lib/terminalRequests";
import type { OfflineView, PullProgress } from "../lib/types";

interface Props {
  ready: boolean;
  active: boolean;
  /** Opens the connection with the offline models. */
  onOpenConnection: (id: string) => void;
}

function percent(p: PullProgress | undefined): string {
  if (!p) return "iniciando…";
  if (p.total && p.completed !== null) {
    return `${Math.floor((p.completed / p.total) * 100)}% · ${formatBytes(p.completed)} de ${formatBytes(p.total)}`;
  }
  return p.status;
}

export function OfflineSection({ ready, active, onOpenConnection }: Props) {
  const [view, setView] = useState<OfflineView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<Record<string, PullProgress>>({});
  const [custom, setCustom] = useState("");
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setView(await offlineApi.status());
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (ready && active) void load();
  }, [ready, active, load]);

  useEffect(
    () =>
      offlineEvents.subscribe((event) => {
        if (event.type === "progress") {
          setProgress((all) => ({ ...all, [event.model]: event }));
        } else {
          setProgress((all) => {
            const { [event.model]: _, ...rest } = all;
            return rest;
          });
          if (event.type === "done") setNotice(`${event.model} baixado.`);
          else setError(`${event.model}: ${event.error}`);
          void load();
        }
      }),
    [load],
  );

  const run = async (action: () => Promise<unknown>, done?: string) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await action();
      if (done) setNotice(done);
      await load();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const pull = (model: string) => {
    setError(null);
    setNotice(null);
    setProgress((all) => ({ ...all, [model]: { model, status: "iniciando…", completed: null, total: null } }));
    setView((v) => (v ? { ...v, downloading: [...v.downloading, model] } : v));
    // Resolves when the download ends; progress comes as events.
    offlineApi.pull(model).catch(() => undefined);
  };

  if (!view) {
    return (
      <div className="task-body">
        {error ? <div className="inline-error">{error}</div> : <div className="meta">verificando o Ollama…</div>}
      </div>
    );
  }

  const { status } = view;
  const installed = new Set(view.models.map((m) => m.name));
  const downloading = new Set([...view.downloading, ...Object.keys(progress)]);

  return (
    <div className="task-body offline-section">
      <section>
        <h3>Modelos offline</h3>
        <p className="meta">
          Modelos de IA que rodam no seu computador, sem internet, sem chave e sem custo por token, pelo{" "}
          <strong>Ollama</strong> (gratuito). São menores que os modelos das APIs: bons para tarefas do dia a dia, com
          privacidade total. A velocidade depende da memória e da placa de vídeo.
        </p>
        <div className={`offline-status ${status.running ? "ok" : "off"}`}>
          <span className={`dot ${status.running ? "ok" : "err"}`} />
          {status.running ? (
            <span>
              Ollama {status.version ?? ""} rodando em <code>{status.url}</code>
            </span>
          ) : status.program ? (
            <span>
              Ollama instalado, mas parado.{" "}
              <button
                className="button small primary"
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    await offlineApi.start();
                    await new Promise((r) => setTimeout(r, 1500));
                  }, "Ollama iniciado.")
                }
              >
                Iniciar o Ollama
              </button>
            </span>
          ) : (
            <span>O Ollama não está instalado neste computador.</span>
          )}
          <button className="button small" disabled={busy} onClick={() => void load()}>
            Verificar de novo
          </button>
        </div>
        {!status.running && !status.program && (
          <div className="offline-install">
            {view.installCommand && (
              <p className="meta">
                Instale pelo terminal (o comando abre num terminal do Orchestrator; responda o que ele pedir):{" "}
                <code>{view.installCommand}</code>
              </p>
            )}
            <div className="row">
              {view.installCommand && (
                <button
                  className="button primary small"
                  onClick={() => view.installCommand && terminalRequests.run(view.installCommand)}
                >
                  Instalar o Ollama
                </button>
              )}
              <button className="button small" onClick={() => void githubApi.openUrl(view.downloadPage)}>
                Baixar do site
              </button>
            </div>
            <p className="meta">Depois de instalar, clique em "Verificar de novo".</p>
          </div>
        )}
      </section>

      {status.running && (
        <>
          <section className="task-step">
            <div className="row">
              <h3 className="grow">Neste computador ({view.models.length})</h3>
              <button
                className="button small primary"
                disabled={busy || view.models.length === 0}
                title="Cria (ou atualiza) a conexão com estes modelos; as sessões passam a poder usá-los"
                onClick={() =>
                  void run(async () => {
                    const id = await offlineApi.use();
                    onOpenConnection(id);
                  }, "Conexão pronta: os modelos aparecem em AI Providers.")
                }
              >
                {view.connection ? "Atualizar a conexão" : "Usar nas sessões"}
              </button>
            </div>
            {view.connection && (
              <p className="meta">
                A conexão <code>{view.connection}</code> já está em AI Providers; novos downloads entram nela sozinhos.
              </p>
            )}
            {view.modelsError && <div className="inline-error">{view.modelsError}</div>}
            {view.models.length === 0 && <div className="meta">Nenhum modelo baixado ainda. Escolha um abaixo.</div>}
            <ul className="list plain">
              {view.models.map((model) => (
                <li key={model.name} className="skill-item">
                  <div className="grow">
                    <div className="title mono">{model.name}</div>
                    <div className="meta">
                      {[
                        formatBytes(model.sizeBytes),
                        model.parameterSize,
                        model.quantization,
                        model.contextWindow ? `contexto ${model.contextWindow.toLocaleString("pt-BR")}` : null,
                        model.tools === true
                          ? "ferramentas nativas"
                          : model.tools === false
                            ? "ferramentas por prompt"
                            : null,
                        model.vision ? "lê imagens" : null,
                      ]
                        .filter(Boolean)
                        .join(" · ")}
                    </div>
                  </div>
                  {confirmDelete === model.name ? (
                    <span className="meta">
                      Apagar do disco?{" "}
                      <button
                        className="link danger"
                        onClick={() => {
                          setConfirmDelete(null);
                          void run(() => offlineApi.remove(model.name), `${model.name} apagado.`);
                        }}
                      >
                        sim
                      </button>{" "}
                      <button className="link" onClick={() => setConfirmDelete(null)}>
                        não
                      </button>
                    </span>
                  ) : (
                    <button className="link" disabled={busy} onClick={() => setConfirmDelete(model.name)}>
                      Apagar
                    </button>
                  )}
                </li>
              ))}
            </ul>
          </section>

          <section className="task-step">
            <h3>Baixar modelos</h3>
            <p className="meta">
              Sugestões que funcionam com as ferramentas do Orchestrator. "Memória" é o quanto de RAM (ou memória da
              placa de vídeo) o modelo precisa para rodar bem; os tamanhos são aproximados.
            </p>
            <ul className="list plain">
              {view.catalog.map((model) => {
                const busyHere = downloading.has(model.name);
                return (
                  <li key={model.name} className="skill-item">
                    <div className="grow">
                      <div className="title">
                        {model.label} <span className="meta mono">{model.name}</span>
                      </div>
                      <div className="meta">
                        ≈ {model.sizeGb.toLocaleString("pt-BR")} GB · memória ≈ {model.memoryGb} GB · {model.note}
                      </div>
                      {busyHere && (
                        <div className="meta">
                          <progress
                            max={progress[model.name]?.total ?? 1}
                            value={progress[model.name]?.completed ?? 0}
                          />{" "}
                          {percent(progress[model.name])}
                        </div>
                      )}
                    </div>
                    {installed.has(model.name) ? (
                      <span className="badge ok">baixado</span>
                    ) : busyHere ? (
                      <button className="button small" onClick={() => void offlineApi.cancel(model.name)}>
                        Cancelar
                      </button>
                    ) : (
                      <button className="button small primary" onClick={() => pull(model.name)}>
                        Baixar
                      </button>
                    )}
                  </li>
                );
              })}
            </ul>
            <div className="row">
              <input
                className="mono grow"
                placeholder="outro modelo do ollama.com/library, ex.: mistral-nemo"
                value={custom}
                onChange={(e) => setCustom(e.target.value.trim())}
              />
              <button
                className="button small"
                disabled={!custom || downloading.has(custom)}
                onClick={() => {
                  pull(custom);
                  setCustom("");
                }}
              >
                Baixar
              </button>
            </div>
            {Object.values(progress)
              .filter((p) => !view.catalog.some((c) => c.name === p.model))
              .map((p) => (
                <div key={p.model} className="meta">
                  {p.model}: {percent(p)}{" "}
                  <button className="link" onClick={() => void offlineApi.cancel(p.model)}>
                    Cancelar
                  </button>
                </div>
              ))}
          </section>
        </>
      )}
      {notice && <div className="inline-notice ok">{notice}</div>}
      {error && <div className="inline-error">{error}</div>}
    </div>
  );
}
