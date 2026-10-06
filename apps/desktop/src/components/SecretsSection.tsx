// "Segredos" (ADR-0020): API keys and tokens the AIs use by name, as
// {{secret:NAME}}. The value goes to the OS vault and is never shown again,
// here or to any AI.

import { useCallback, useEffect, useState } from "react";
import { formatTime } from "../lib/format";
import { errorMessage, secretsApi } from "../lib/runtime";
import type { SecretsView } from "../lib/types";

export function SecretsSection({ ready }: { ready: boolean }) {
  const [view, setView] = useState<SecretsView | null>(null);
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setView(await secretsApi.list());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (ready) void load();
  }, [ready, load]);

  const run = async (action: () => Promise<SecretsView>, done?: string) => {
    setBusy(true);
    setError(null);
    setSaved(null);
    try {
      setView(await action());
      if (done) setSaved(done);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const save = () => {
    const trimmed = name.trim();
    void run(async () => {
      const next = await secretsApi.save(trimmed, value);
      setName("");
      setValue("");
      return next;
    }, `"${trimmed}" salvo no cofre. As IAs usam como {{secret:${trimmed}}}.`);
  };

  return (
    <section className="task-step secrets">
      <h3>Segredos</h3>
      <p className="meta">
        Chaves de API e tokens que as IAs podem usar pelo nome, escrevendo <code>{"{{secret:NOME}}"}</code> numa
        chamada de API (<code>http.request</code>, <code>web.fetch</code>) ou na variável de ambiente de um comando. O
        valor fica no {view?.vault ?? "cofre do sistema"} e <strong>nunca</strong> é mostrado a uma IA nem volta para
        esta tela: o que a resposta trouxer dele aparece como <code>***</code>.
      </p>
      {view?.warning && <div className="inline-notice">{view.warning}</div>}
      {error && <div className="inline-error">{error}</div>}
      {saved && <div className="inline-notice ok">{saved}</div>}
      <ul className="secret-list">
        {view && view.secrets.length === 0 && <li className="meta">Nenhum segredo salvo.</li>}
        {view?.secrets.map((secret) => (
          <li key={secret.name} className="secret-item">
            <code className="grow">{secret.placeholder}</code>
            <span className="meta" title={secret.loaded ? "Valor no cofre" : "O cofre não tem o valor: salve de novo"}>
              {secret.loaded ? `salvo ${formatTime(secret.updatedAt)}` : "sem valor no cofre"}
            </span>
            {confirmDelete === secret.name ? (
              <>
                <button
                  className="button small danger"
                  disabled={busy}
                  onClick={() => {
                    setConfirmDelete(null);
                    void run(() => secretsApi.remove(secret.name));
                  }}
                >
                  Apagar
                </button>
                <button className="button small" onClick={() => setConfirmDelete(null)}>
                  Cancelar
                </button>
              </>
            ) : (
              <button
                className="button small"
                disabled={busy}
                title="Apaga o valor do cofre; as IAs deixam de poder usá-lo"
                onClick={() => setConfirmDelete(secret.name)}
              >
                Apagar…
              </button>
            )}
          </li>
        ))}
      </ul>
      <form
        className="secret-form"
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <input
          className="mono"
          placeholder="NOME (ex.: OPENROUTER_KEY)"
          value={name}
          maxLength={64}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => setName(e.target.value)}
        />
        <input
          type="password"
          placeholder="valor (chave, token…)"
          value={value}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => setValue(e.target.value)}
        />
        <button className="button small primary" type="submit" disabled={!ready || busy || !name.trim() || !value.trim()}>
          Salvar no cofre
        </button>
      </form>
      <p className="meta">
        Salvar com um nome que já existe troca o valor. Para usar uma API que pede login, procure a página de chaves
        de API (ou tokens) do serviço e salve a chave aqui.
      </p>
    </section>
  );
}
