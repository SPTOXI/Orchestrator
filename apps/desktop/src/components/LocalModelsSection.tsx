// "Modelos locais" (ADR-0025): AI models that run on this computer with the
// Orchestrator's own engine (llama.cpp) — no internet, no key, no cost per
// token, nothing else to install. The engine, the models (catalog, Hugging
// Face, Ollama, a file) and the context each one runs with.

import { useCallback, useEffect, useState } from "react";
import { formatContext } from "../lib/council";
import { localEvents } from "../lib/events";
import { formatBytes } from "../lib/format";
import {
  BACKEND_LABELS,
  contextProblem,
  contextWarning,
  isNewerEngine,
  memoryLine,
  progressText,
  repoId,
  serverLine,
  sourceLabel,
  systemLine,
} from "../lib/local";
import { errorMessage, localApi } from "../lib/runtime";
import type { HfFile, LocalBackend, LocalModel, LocalSettings, LocalView } from "../lib/types";

interface Props {
  ready: boolean;
  active: boolean;
  /** Opens the connection with the local models. */
  onOpenConnection: (id: string) => void;
  /** Models or the connection changed: AI Providers lists them again. */
  onChanged?: () => void;
}

type Progress = Record<string, { done: number; total: number | null }>;

function Bar({ progress }: { progress?: { done: number; total: number | null } }) {
  return (
    <div className="meta">
      <progress max={progress?.total ?? 1} value={progress?.total ? progress.done : 0} />{" "}
      {progress ? progressText(progress.done, progress.total) : "iniciando…"}
    </div>
  );
}

function ModelRow({
  model,
  busy,
  onContext,
  onRemove,
}: {
  model: LocalModel;
  busy: boolean;
  onContext: (context: number) => void;
  onRemove: () => void;
}) {
  const [text, setText] = useState(String(model.context));
  const [confirm, setConfirm] = useState(false);
  useEffect(() => setText(String(model.context)), [model.context]);
  const value = Number(text);
  const problem = contextProblem(model, value);
  const changed = value !== model.context;
  const warning = !problem ? contextWarning(value) : null;
  return (
    <li className="skill-item local-model">
      <div className="grow">
        <div className="title">
          {model.name} <span className="meta mono">{model.id}</span>
        </div>
        <div className="meta">
          {[
            formatBytes(model.size),
            model.sizeLabel,
            model.quantization,
            model.tools ? "ferramentas nativas" : "ferramentas por prompt",
            model.chatTemplate ? null : "sem modelo de conversa no arquivo",
            sourceLabel(model.source),
          ]
            .filter(Boolean)
            .join(" · ")}
        </div>
        <div className="row local-context">
          <label className="meta" htmlFor={`ctx-${model.id}`}>
            Contexto
          </label>
          <input
            id={`ctx-${model.id}`}
            className={`num ${problem ? "invalid" : ""}`}
            type="number"
            min={2048}
            step={1024}
            value={text}
            onChange={(e) => setText(e.target.value)}
          />
          <span className="meta">
            tokens
            {model.trainedContext ? ` (treinado para até ${formatContext(model.trainedContext)})` : ""} ·{" "}
            {memoryLine(model, problem ? model.context : value)}
          </span>
          {changed && (
            <button className="button small primary" disabled={busy || !!problem} onClick={() => onContext(value)}>
              Salvar
            </button>
          )}
        </div>
        {problem && <div className="err-text">{problem}</div>}
        {warning && <div className="meta warn-text">{warning}</div>}
      </div>
      {confirm ? (
        <span className="meta">
          {model.source.kind === "file" ? "Tirar da lista (o arquivo fica)?" : "Apagar do disco?"}{" "}
          <button
            className="link danger"
            onClick={() => {
              setConfirm(false);
              onRemove();
            }}
          >
            sim
          </button>{" "}
          <button className="link" onClick={() => setConfirm(false)}>
            não
          </button>
        </span>
      ) : (
        <button className="link" disabled={busy} onClick={() => setConfirm(true)}>
          Remover
        </button>
      )}
    </li>
  );
}

export function LocalModelsSection({ ready, active, onOpenConnection, onChanged }: Props) {
  const [view, setView] = useState<LocalView | null>(null);
  /** Reading the state failed. */
  const [loadError, setLoadError] = useState<string | null>(null);
  /** What the last action said: stays until the next one starts. */
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<Progress>({});
  const [latest, setLatest] = useState<string | null>(null);
  const [log, setLog] = useState<string[] | null>(null);
  const [repo, setRepo] = useState("");
  const [files, setFiles] = useState<{ repo: string; list: HfFile[] } | null>(null);
  const [filePath, setFilePath] = useState("");

  const load = useCallback(async () => {
    try {
      setView(await localApi.status());
      setLoadError(null);
    } catch (e) {
      setLoadError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (ready && active) void load();
  }, [ready, active, load]);

  useEffect(
    () =>
      localEvents.subscribe((event) => {
        switch (event.type) {
          case "progress":
            setProgress((all) => ({ ...all, [event.key]: { done: event.done, total: event.total } }));
            return;
          case "downloadDone":
          case "downloadFailed":
            setProgress((all) => {
              const { [event.key]: _, ...rest } = all;
              return rest;
            });
            if (event.type === "downloadFailed" && event.error !== "download cancelado") {
              setError(`${event.key === "engine" ? "Motor" : event.key}: ${event.error}`);
            }
            void load();
            return;
          default:
            void load();
            if (event.type === "server" && log) void localApi.log().then(setLog);
        }
      }),
    [load, log],
  );

  const run = async (action: () => Promise<unknown>, done?: string) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await action();
      if (done) setNotice(done);
      await load();
      onChanged?.();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  /** A download: resolves when it ends; progress comes as events. */
  const download = (key: string, action: () => Promise<unknown>, done: string) => {
    setError(null);
    setNotice(null);
    setProgress((all) => ({ ...all, [key]: all[key] ?? { done: 0, total: null } }));
    setView((v) => (v ? { ...v, downloading: [...v.downloading, key] } : v));
    action()
      .then(() => setNotice(done))
      .catch((e) => {
        const message = errorMessage(e);
        if (message !== "download cancelado") setError(message);
      })
      .finally(() => {
        void load();
        onChanged?.();
      });
  };

  const saveSettings = (patch: Partial<LocalSettings>) => {
    if (!view) return;
    void run(() => localApi.saveSettings({ ...view.settings, ...patch }));
  };

  if (!view) {
    return (
      <div className="task-body">
        {loadError ? <div className="inline-error">{loadError}</div> : <div className="meta">verificando…</div>}
      </div>
    );
  }

  const downloading = new Set([...view.downloading, ...Object.keys(progress)]);
  const engine = view.engine;
  const chosen: LocalBackend | null = view.settings.backend ?? view.autoBackend;
  const engineBusy = downloading.has("engine");
  const runningModel = view.server.state === "ready" || view.server.state === "starting" ? view.server.model : null;

  return (
    <div className="task-body local-section">
      <section>
        <h3>Modelos locais</h3>
        <p className="meta">
          Modelos de IA que rodam no seu computador, sem internet, sem chave e sem custo por token, com o motor do
          próprio Orchestrator (llama.cpp): nada para instalar à parte. São menores que os modelos das APIs, bons para o
          dia a dia e com privacidade total. A velocidade depende da memória e da placa de vídeo.
        </p>
        <p className="meta">Este computador: {systemLine(view.system)}.</p>
        {view.legacyOllama && (
          <div className="inline-notice">
            A conexão antiga do <strong>Ollama</strong> ainda está em AI Providers. O Orchestrator não depende mais dele:
            importe os modelos abaixo (em "Importar do Ollama") e remova a conexão antiga.{" "}
            <button
              className="button small"
              disabled={busy}
              onClick={() => void run(() => localApi.removeLegacy(), "Conexão antiga do Ollama removida.")}
            >
              Remover a conexão antiga
            </button>
          </div>
        )}
        {view.warnings.map((w) => (
          <div key={w} className="inline-notice">
            {w}
          </div>
        ))}
      </section>

      {(notice || error || loadError) && (
        <div className="local-feedback" role="status">
          {notice && <div className="inline-notice ok">{notice}</div>}
          {error && <div className="inline-error">{error}</div>}
          {loadError && <div className="inline-error">{loadError}</div>}
        </div>
      )}

      <section className="task-step">
        <h3>Motor</h3>
        {engine ? (
          <div className={`local-status ok`}>
            <span className="dot ok" />
            <span>
              llama.cpp <strong>{engine.tag}</strong> · {BACKEND_LABELS[engine.backend]}
            </span>
            {latest && isNewerEngine(engine.tag, latest) ? (
              <button
                className="button small primary"
                disabled={busy || engineBusy}
                onClick={() => download("engine", () => localApi.installEngine(), `Motor atualizado para ${latest}.`)}
              >
                Atualizar para {latest}
              </button>
            ) : (
              <button
                className="button small"
                disabled={busy || engineBusy}
                onClick={() =>
                  void run(async () => {
                    const tag = await localApi.latestEngine();
                    setLatest(tag);
                    if (!isNewerEngine(engine.tag, tag)) setNotice(`O motor já é o mais novo (${tag}).`);
                  })
                }
              >
                Procurar atualização
              </button>
            )}
          </div>
        ) : (
          <p className="meta">
            O motor ainda não está instalado. O Orchestrator baixa o pacote oficial do llama.cpp feito para este
            computador (de 30 a 600 MB) e confere o arquivo antes de instalar.
          </p>
        )}
        <div className="row">
          <label className="meta" htmlFor="local-backend">
            Pacote
          </label>
          <select
            id="local-backend"
            value={view.settings.backend ?? ""}
            disabled={busy || engineBusy}
            onChange={(e) => saveSettings({ backend: (e.target.value || null) as LocalBackend | null })}
          >
            <option value="">
              Automático{view.autoBackend ? ` (${BACKEND_LABELS[view.autoBackend]})` : ""}
            </option>
            {view.backends.map((b) => (
              <option key={b} value={b}>
                {BACKEND_LABELS[b]}
              </option>
            ))}
          </select>
          {(!engine || (chosen && engine.backend !== chosen)) && (
            <button
              className="button small primary"
              disabled={busy || engineBusy || !chosen}
              onClick={() => download("engine", () => localApi.installEngine(), "Motor instalado.")}
            >
              {engine ? "Instalar com este pacote" : "Instalar o motor"}
            </button>
          )}
          {engineBusy && (
            <button className="button small" onClick={() => void localApi.cancel("engine")}>
              Cancelar
            </button>
          )}
        </div>
        {engineBusy && <Bar progress={progress.engine} />}
        {engine && (
          <>
            <div className={`local-status ${view.server.state === "failed" ? "off" : ""}`}>
              <span
                className={`dot ${view.server.state === "ready" ? "ok" : view.server.state === "failed" ? "err" : ""}`}
              />
              <span>{serverLine(view.server)}</span>
              {runningModel && (
                <button className="button small" disabled={busy} onClick={() => void run(() => localApi.stop())}>
                  Desligar
                </button>
              )}
              <button
                className="link"
                onClick={() => (log ? setLog(null) : void localApi.log().then(setLog))}
              >
                {log ? "Esconder o registro" : "Ver o registro"}
              </button>
            </div>
            {log && <pre className="output local-log">{log.length ? log.join("\n") : "(vazio)"}</pre>}
            <div className="row">
              <label className="meta" htmlFor="local-gpu">
                Placa de vídeo
              </label>
              <select
                id="local-gpu"
                value={view.settings.gpu}
                disabled={busy}
                onChange={(e) => saveSettings({ gpu: e.target.value as LocalSettings["gpu"] })}
              >
                <option value="auto">Automático (usa o que couber)</option>
                <option value="off">Desligada (só o processador)</option>
              </select>
              <label className="meta" htmlFor="local-idle">
                Desligar o motor depois de
              </label>
              <select
                id="local-idle"
                value={view.settings.idleMinutes}
                disabled={busy}
                onChange={(e) => saveSettings({ idleMinutes: Number(e.target.value) })}
              >
                {[5, 10, 30, 60, 0].map((m) => (
                  <option key={m} value={m}>
                    {m === 0 ? "nunca" : `${m} min parado`}
                  </option>
                ))}
              </select>
            </div>
            <div className="row">
              <button
                className="link danger"
                disabled={busy || engineBusy}
                onClick={() => void run(() => localApi.removeEngine(), "Motor removido (os modelos continuam).")}
              >
                Remover o motor
              </button>
            </div>
          </>
        )}
      </section>

      <section className="task-step">
        <div className="row">
          <h3 className="grow">Meus modelos ({view.models.length})</h3>
          {view.connection && (
            <button className="button small" onClick={() => view.connection && onOpenConnection(view.connection)}>
              Abrir a conexão
            </button>
          )}
        </div>
        {view.models.length === 0 ? (
          <div className="meta">Nenhum modelo ainda. Baixe um abaixo, importe do Ollama ou use um arquivo.</div>
        ) : view.connection && view.connectionEnabled ? (
          <p className="meta">
            Aparecem em AI Providers como <strong>Modelos locais</strong> e no Conselho. O motor liga o modelo quando
            uma sessão pede, com o contexto escolhido aqui, e desliga quando fica parado.
          </p>
        ) : (
          <div className="inline-notice">
            {view.connection
              ? "A conexão Modelos locais está desativada: os modelos não aparecem em AI Providers nem nas sessões."
              : "Os modelos ainda não aparecem em AI Providers."}{" "}
            <button
              className="button small primary"
              disabled={busy}
              onClick={() =>
                void run(() => localApi.activateConnection(), "Pronto: os modelos aparecem em AI Providers como Modelos locais.")
              }
            >
              Ativar
            </button>
          </div>
        )}
        <ul className="list plain">
          {view.models.map((model) => (
            <ModelRow
              key={model.id}
              model={model}
              busy={busy}
              onContext={(context) =>
                void run(() => localApi.setContext(model.id, context), `Contexto de ${model.name} salvo.`)
              }
              onRemove={() => void run(() => localApi.removeModel(model.id), `${model.name} removido.`)}
            />
          ))}
        </ul>
      </section>

      <section className="task-step">
        <h3>Baixar modelos</h3>
        <p className="meta">
          Do Hugging Face, conferidos pelo SHA-256. "Memória" é o quanto de RAM (ou memória da placa de vídeo) o modelo
          precisa para rodar bem; os tamanhos são aproximados. O download continua de onde parou se cair.
        </p>
        <ul className="list plain">
          {view.catalog.map((item) => {
            const busyHere = downloading.has(item.id);
            return (
              <li key={item.id} className="skill-item">
                <div className="grow">
                  <div className="title">
                    {item.label} <span className="meta mono">{item.quant}</span>
                  </div>
                  <div className="meta">
                    ≈ {item.sizeGb.toLocaleString("pt-BR")} GB · memória ≈ {item.memoryGb} GB · {item.note}
                  </div>
                  {busyHere && <Bar progress={progress[item.id]} />}
                </div>
                {item.installed ? (
                  <span className="badge ok">baixado</span>
                ) : busyHere ? (
                  <button className="button small" onClick={() => void localApi.cancel(item.id)}>
                    Cancelar
                  </button>
                ) : (
                  <button
                    className="button small primary"
                    onClick={() =>
                      download(item.id, () => localApi.downloadCatalog(item.id), `${item.label} baixado.`)
                    }
                  >
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
            placeholder="outro repositório do Hugging Face, ex.: bartowski/Mistral-Nemo-Instruct-2407-GGUF"
            value={repo}
            onChange={(e) => setRepo(e.target.value)}
          />
          <button
            className="button small"
            disabled={!repo.trim() || busy}
            onClick={() =>
              void run(async () => {
                const list = await localApi.hfFiles(repo);
                setFiles({ repo: repoId(repo), list });
                if (list.length === 0) setNotice("Esse repositório não tem arquivos .gguf.");
              })
            }
          >
            Ver arquivos
          </button>
        </div>
        {files && files.list.length > 0 && (
          <ul className="list plain">
            {files.list.map((file) => {
              const key = `${files.repo}/${file.path}`;
              const busyHere = downloading.has(key);
              return (
                <li key={file.path} className="skill-item">
                  <div className="grow">
                    <div className="title mono">{file.path}</div>
                    <div className="meta">{formatBytes(file.size)}</div>
                    {busyHere && <Bar progress={progress[key]} />}
                  </div>
                  {busyHere ? (
                    <button className="button small" onClick={() => void localApi.cancel(key)}>
                      Cancelar
                    </button>
                  ) : (
                    <button
                      className="button small primary"
                      onClick={() => download(key, () => localApi.downloadHf(files.repo, file.path), `${file.path} baixado.`)}
                    >
                      Baixar
                    </button>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </section>

      {view.ollama.length > 0 && (
        <section className="task-step">
          <h3>Importar do Ollama</h3>
          <p className="meta">
            Modelos que o Ollama já baixou neste computador. Importar não baixa de novo nem ocupa espaço (o arquivo é
            ligado, não copiado, quando estão no mesmo disco), e o modelo continua funcionando se você desinstalar o
            Ollama.
          </p>
          <ul className="list plain">
            {view.ollama.map((m) => (
              <li key={m.name} className="skill-item">
                <div className="grow">
                  <div className="title mono">{m.name}</div>
                  <div className="meta">{formatBytes(m.size)}</div>
                </div>
                {m.imported ? (
                  <span className="badge ok">importado</span>
                ) : (
                  <button
                    className="button small primary"
                    disabled={busy}
                    onClick={() => void run(() => localApi.importOllama(m.name), `${m.name} importado.`)}
                  >
                    Importar
                  </button>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}

      <section className="task-step">
        <h3>Arquivo do computador</h3>
        <p className="meta">Um modelo .gguf que você já tem. Ele é usado onde está: se mudar de lugar, adicione de novo.</p>
        <div className="row">
          <input
            className="mono grow"
            placeholder="caminho do arquivo .gguf"
            value={filePath}
            onChange={(e) => setFilePath(e.target.value)}
          />
          <button
            className="button small"
            disabled={busy}
            onClick={() =>
              void localApi.pickFile().then((path) => {
                if (path) setFilePath(path);
              })
            }
          >
            Escolher…
          </button>
          <button
            className="button small primary"
            disabled={busy || !filePath.trim()}
            onClick={() =>
              void run(async () => {
                const model = await localApi.addFile(filePath);
                setFilePath("");
                setNotice(`${model.name} adicionado.`);
              })
            }
          >
            Adicionar
          </button>
        </div>
      </section>

    </div>
  );
}
