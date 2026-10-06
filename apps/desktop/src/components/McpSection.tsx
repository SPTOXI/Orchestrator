// "Servidores MCP" (ADR-0021): programs or URLs that speak the Model
// Context Protocol. Their tools reach every AI as mcp.<server>.<tool>,
// through the same autonomy gate and history as the Orchestrator's own.

import { useCallback, useEffect, useState } from "react";
import { errorMessage, mcpApi } from "../lib/runtime";
import type { McpServerConfig, McpServerView, McpStatus } from "../lib/types";

const STATUS: Record<McpStatus, [string, string]> = {
  ready: ["ok", "conectado"],
  starting: ["", "conectando…"],
  failed: ["err", "falhou"],
  disabled: ["off", "desligado"],
};

const TEMPLATES: Array<{ label: string; hint: string; server: Partial<McpServerConfig> }> = [
  {
    label: "Playwright (navegador de verdade)",
    hint: "As IAs abrem páginas, clicam, preenchem formulários e tiram prints. Precisa do Node.js.",
    server: { id: "playwright", name: "Playwright", transport: "stdio", command: "npx", args: ["-y", "@playwright/mcp@latest"] },
  },
  {
    label: "Context7 (documentação atualizada de bibliotecas)",
    hint: "Documentação recente de bibliotecas e frameworks. Precisa do Node.js.",
    server: { id: "context7", name: "Context7", transport: "stdio", command: "npx", args: ["-y", "@upstash/context7-mcp"] },
  },
  {
    label: "GitHub oficial (HTTP)",
    hint: "O servidor MCP do GitHub. Salve o token como segredo GITHUB_TOKEN em Políticas e segredos.",
    server: {
      id: "github-mcp",
      name: "GitHub (MCP)",
      transport: "http",
      url: "https://api.githubcopilot.com/mcp/",
      headers: { Authorization: "Bearer {{secret:GITHUB_TOKEN}}" },
    },
  },
];

function blank(): McpServerConfig {
  return {
    id: "",
    name: "",
    transport: "stdio",
    command: "",
    args: [],
    env: {},
    cwd: null,
    url: "",
    headers: {},
    enabled: true,
    timeoutSecs: null,
    disabledTools: [],
  };
}

function idFrom(name: string): string {
  const id = name
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 24)
    .replace(/-+$/, "");
  return /^[a-z]/.test(id) ? id : `mcp-${id}`.slice(0, 24);
}

/** `KEY=value` lines ↔ a map. */
function toLines(map: Record<string, string>, sep: string): string {
  return Object.entries(map)
    .map(([k, v]) => `${k}${sep}${v}`)
    .join("\n");
}

function fromLines(text: string, sep: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const at = line.indexOf(sep);
    if (at > 0) out[line.slice(0, at).trim()] = line.slice(at + sep.length).trim();
  }
  return out;
}

interface Draft {
  server: McpServerConfig;
  previousId: string | null;
  args: string;
  env: string;
  headers: string;
  idTouched: boolean;
}

function draftOf(server: McpServerConfig, previousId: string | null): Draft {
  return {
    server,
    previousId,
    args: server.args.join("\n"),
    env: toLines(server.env, "="),
    headers: toLines(server.headers, ": "),
    idTouched: previousId !== null,
  };
}

export function McpSection({ ready, active }: { ready: boolean; active: boolean }) {
  const [servers, setServers] = useState<McpServerView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [importText, setImportText] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [importError, setImportError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setServers(await mcpApi.list());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (ready && active) void load();
  }, [ready, active, load]);

  // Connecting happens in the background: follow it.
  const connecting = servers?.some((s) => s.status === "starting") ?? false;
  useEffect(() => {
    if (!active || !connecting) return;
    const timer = setInterval(() => void load(), 1500);
    return () => clearInterval(timer);
  }, [active, connecting, load]);

  const run = async (action: () => Promise<unknown>, done?: string) => {
    setError(null);
    setNotice(null);
    try {
      await action();
      if (done) setNotice(done);
      await load();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const save = () => {
    if (!draft) return;
    const server: McpServerConfig = {
      ...draft.server,
      args: draft.args.split("\n").map((a) => a.trim()).filter(Boolean),
      env: fromLines(draft.env, "="),
      headers: fromLines(draft.headers, ":"),
    };
    void run(async () => {
      await mcpApi.save(server, draft.previousId);
      setDraft(null);
      setOpen(server.id);
    }, `${server.name || server.id} salvo; conectando…`);
  };

  const set = (patch: Partial<McpServerConfig>) => draft && setDraft({ ...draft, server: { ...draft.server, ...patch } });

  return (
    <div className="task-body mcp-section">
      <section>
        <h3>Servidores MCP</h3>
        <p className="meta">
          MCP (Model Context Protocol) é o padrão para dar ferramentas novas às IAs: um navegador, um banco de dados,
          o Jira, o Figma, documentação… Cada servidor ligado aqui oferece as ferramentas dele a <strong>todas</strong>{" "}
          as IAs do Orchestrator (APIs, modelos offline e assinaturas por CLI), com o nome <code>mcp.servidor.ferramenta</code>
          . Elas passam pelas mesmas regras de autonomia e ficam no histórico; as que o servidor marca como consulta
          contam como consulta. Valores secretos em cabeçalhos ou variáveis: <code>{"{{secret:NOME}}"}</code>.
        </p>
        <div className="row">
          <button className="button small primary" onClick={() => setDraft(draftOf(blank(), null))}>
            Adicionar servidor
          </button>
          <button className="button small" onClick={() => setImportText("")}>
            Importar JSON (Claude, Cursor…)
          </button>
          {TEMPLATES.map((t) => (
            <button
              key={t.label}
              className="button small"
              title={t.hint}
              disabled={servers?.some((s) => s.config.id === t.server.id)}
              onClick={() => setDraft(draftOf({ ...blank(), ...t.server }, null))}
            >
              + {t.label}
            </button>
          ))}
        </div>
      </section>

      {importText !== null && (
        <section className="task-step">
          <h3>Importar</h3>
          <p className="meta">
            Cole o JSON de configuração do Claude Desktop, Claude Code, Cursor ou outro (
            <code>{'{"mcpServers": {...}}'}</code>). Os servidores entram ligados.
          </p>
          <textarea
            className="mono rules-text"
            spellCheck={false}
            placeholder={'{\n  "mcpServers": {\n    "filesystem": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "C:/projetos"]}\n  }\n}'}
            value={importText}
            onChange={(e) => setImportText(e.target.value)}
          />
          <div className="row">
            <button
              className="button small primary"
              disabled={!importText.trim()}
              onClick={() =>
                void (async () => {
                  setImportError(null);
                  try {
                    const added = await mcpApi.importJson(importText);
                    setImportText(null);
                    setNotice(`Importados: ${added.join(", ")}. Conectando…`);
                    await load();
                  } catch (e) {
                    setImportError(errorMessage(e));
                  }
                })()
              }
            >
              Importar
            </button>
            <button
              className="button small"
              onClick={() => {
                setImportText(null);
                setImportError(null);
              }}
            >
              Cancelar
            </button>
          </div>
          {importError && <div className="inline-error">{importError}</div>}
        </section>
      )}

      {draft && (
        <section className="task-step">
          <h3>{draft.previousId ? `Editar ${draft.previousId}` : "Novo servidor"}</h3>
          <label className="form-row">
            <span>Nome</span>
            <input
              value={draft.server.name}
              onChange={(e) =>
                setDraft({
                  ...draft,
                  server: {
                    ...draft.server,
                    name: e.target.value,
                    id: draft.idTouched ? draft.server.id : idFrom(e.target.value),
                  },
                })
              }
            />
          </label>
          <label className="form-row">
            <span>Id (nas ferramentas)</span>
            <input
              className="mono"
              value={draft.server.id}
              onChange={(e) => setDraft({ ...draft, idTouched: true, server: { ...draft.server, id: e.target.value } })}
            />
          </label>
          <label className="form-row">
            <span>Tipo</span>
            <select value={draft.server.transport} onChange={(e) => set({ transport: e.target.value as "stdio" | "http" })}>
              <option value="stdio">Programa local (stdio)</option>
              <option value="http">Servidor remoto (HTTP)</option>
            </select>
          </label>
          {draft.server.transport === "stdio" ? (
            <>
              <label className="form-row">
                <span>Comando</span>
                <input
                  className="mono"
                  placeholder="npx, uvx, python, node, caminho de um programa…"
                  value={draft.server.command}
                  onChange={(e) => set({ command: e.target.value })}
                />
              </label>
              <label className="form-row top">
                <span>Argumentos (um por linha)</span>
                <textarea className="mono" value={draft.args} onChange={(e) => setDraft({ ...draft, args: e.target.value })} />
              </label>
              <label className="form-row top">
                <span>Variáveis (NOME=valor)</span>
                <textarea
                  className="mono"
                  placeholder="API_KEY={{secret:MINHA_CHAVE}}"
                  value={draft.env}
                  onChange={(e) => setDraft({ ...draft, env: e.target.value })}
                />
              </label>
              <label className="form-row">
                <span>Pasta de trabalho</span>
                <input
                  className="mono"
                  placeholder="(opcional)"
                  value={draft.server.cwd ?? ""}
                  onChange={(e) => set({ cwd: e.target.value || null })}
                />
              </label>
            </>
          ) : (
            <>
              <label className="form-row">
                <span>URL</span>
                <input
                  className="mono"
                  placeholder="https://exemplo.com/mcp"
                  value={draft.server.url}
                  onChange={(e) => set({ url: e.target.value })}
                />
              </label>
              <label className="form-row top">
                <span>Cabeçalhos (Nome: valor)</span>
                <textarea
                  className="mono"
                  placeholder="Authorization: Bearer {{secret:MEU_TOKEN}}"
                  value={draft.headers}
                  onChange={(e) => setDraft({ ...draft, headers: e.target.value })}
                />
              </label>
            </>
          )}
          <label className="form-row">
            <span>Tempo máx. por chamada (s)</span>
            <input
              className="num"
              placeholder="300"
              value={draft.server.timeoutSecs ?? ""}
              onChange={(e) => set({ timeoutSecs: e.target.value ? Math.max(1, Number(e.target.value) || 300) : null })}
            />
          </label>
          <div className="row">
            <button className="button small primary" onClick={save}>
              Salvar e conectar
            </button>
            <button className="button small" onClick={() => setDraft(null)}>
              Cancelar
            </button>
          </div>
        </section>
      )}

      <section className="task-step">
        <h3>Servidores ({servers?.length ?? 0})</h3>
        {servers?.length === 0 && <div className="meta">Nenhum servidor ainda.</div>}
        <ul className="list plain">
          {servers?.map((s) => {
            const [dot, label] = STATUS[s.status];
            const expanded = open === s.config.id;
            const enabledTools = s.tools.filter((t) => t.enabled).length;
            return (
              <li key={s.config.id} className="mcp-server">
                <div className="row">
                  <span className={`dot ${dot} ${s.status === "starting" ? "pulse" : ""}`} />
                  <button className="link grow ellipsis mcp-title" onClick={() => setOpen(expanded ? null : s.config.id)}>
                    <strong>{s.config.name || s.config.id}</strong>{" "}
                    <span className="meta">
                      {label}
                      {s.status === "ready" && ` · ${enabledTools} de ${s.tools.length} ferramentas`}
                      {s.serverName && ` · ${s.serverName} ${s.serverVersion ?? ""}`}
                    </span>
                  </button>
                  <label className="check-line" title="Ligar/desligar o servidor">
                    <input
                      type="checkbox"
                      checked={s.config.enabled}
                      onChange={(e) => void run(() => mcpApi.save({ ...s.config, enabled: e.target.checked }, null))}
                    />
                  </label>
                  <button className="link" onClick={() => void run(() => mcpApi.restart(s.config.id))}>
                    Reiniciar
                  </button>
                  <button className="link" onClick={() => setDraft(draftOf(s.config, s.config.id))}>
                    Editar
                  </button>
                  {confirmDelete === s.config.id ? (
                    <span className="meta">
                      Remover?{" "}
                      <button
                        className="link danger"
                        onClick={() => {
                          setConfirmDelete(null);
                          void run(() => mcpApi.remove(s.config.id));
                        }}
                      >
                        sim
                      </button>{" "}
                      <button className="link" onClick={() => setConfirmDelete(null)}>
                        não
                      </button>
                    </span>
                  ) : (
                    <button className="link" onClick={() => setConfirmDelete(s.config.id)}>
                      Remover
                    </button>
                  )}
                </div>
                <div className="meta mono ellipsis">
                  {s.config.transport === "http" ? s.config.url : [s.config.command, ...s.config.args].join(" ")}
                </div>
                {s.error && <div className="inline-error">{s.error}</div>}
                {expanded && (
                  <div className="mcp-details">
                    {s.tools.length > 0 && (
                      <ul className="list plain">
                        {s.tools.map((t) => (
                          <li key={t.name} className={`skill-item ${t.enabled ? "" : "off"}`}>
                            <input
                              type="checkbox"
                              title={t.enabled ? "Esconder esta ferramenta das IAs" : "Oferecer esta ferramenta às IAs"}
                              checked={t.enabled}
                              onChange={(e) => void run(() => mcpApi.setTool(s.config.id, t.remote, e.target.checked))}
                            />
                            <div className="grow">
                              <div className="title mono">
                                {t.name} {t.readOnly && <span className="tag small">consulta</span>}
                              </div>
                              <div className="meta">{t.description}</div>
                            </div>
                          </li>
                        ))}
                      </ul>
                    )}
                    {s.log.length > 0 && (
                      <details>
                        <summary className="meta">Saída do servidor ({s.log.length} linhas)</summary>
                        <pre className="skill-preview">{s.log.join("\n")}</pre>
                      </details>
                    )}
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      </section>
      {notice && <div className="inline-notice ok">{notice}</div>}
      {error && <div className="inline-error">{error}</div>}
    </div>
  );
}
