// Development rules and skills (ADR-0021): what every AI follows, and
// instructions for kinds of work that an AI reads when a task calls for
// them. The Orchestrator's own are edited here; a project's files are only
// read (nothing is written inside a project).

import { useCallback, useEffect, useState } from "react";
import { errorMessage, guidanceApi } from "../lib/runtime";
import type { GuidanceSettings, GuidanceView, SkillInfo, SkillSource } from "../lib/types";

const SOURCE_LABELS: Record<SkillSource, string> = {
  orchestrator: "Orchestrator",
  project: "projeto",
  claudeUser: "Claude Code (~/.claude)",
};

/** The view, reloaded on demand; shared by both sections. */
export function useGuidance(ready: boolean, active: boolean) {
  const [view, setView] = useState<GuidanceView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    try {
      setView(await guidanceApi.get());
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);
  useEffect(() => {
    if (ready && active) void load();
  }, [ready, active, load]);
  const saveSettings = async (patch: Partial<GuidanceSettings>) => {
    if (!view) return;
    try {
      const settings = await guidanceApi.saveSettings({ ...view.settings, ...patch });
      setView({ ...view, settings });
      await load();
    } catch (e) {
      setError(errorMessage(e));
    }
  };
  return { view, error, load, saveSettings };
}

type Guidance = ReturnType<typeof useGuidance>;

function Check({ checked, onChange, children }: { checked: boolean; onChange: (v: boolean) => void; children: string }) {
  return (
    <label className="check-line">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span>{children}</span>
    </label>
  );
}

export function RulesSection({ guidance }: { guidance: Guidance }) {
  const { view, saveSettings } = guidance;
  const [text, setText] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const current = text ?? view?.userRules ?? "";
  const dirty = text !== null && text !== view?.userRules;

  const save = async () => {
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      await guidanceApi.saveRules(current);
      await guidance.load();
      setText(null);
      setSaved(true);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  if (!view) return <div className="meta">carregando…</div>;
  return (
    <>
      <section>
        <h3>Regras de desenvolvimento</h3>
        <p className="meta">
          O que toda IA deve seguir neste Orchestrator, em qualquer projeto: estilo de código, testes, commits, o
          que nunca fazer. As regras vão inteiras nas instruções de cada sessão nova (sessões, agentes e tasks), junto
          com o contexto do projeto. Regras de um projeto só ficam na memória do projeto (tipo "regra") ou nos arquivos
          do próprio projeto, abaixo.
        </p>
        <Check checked={view.settings.rulesEnabled} onChange={(v) => void saveSettings({ rulesEnabled: v })}>
          Enviar as regras para as IAs
        </Check>
        <textarea
          className="mono rules-text"
          spellCheck={false}
          placeholder={"Exemplos:\n- Escreva testes para todo código novo.\n- Use TypeScript estrito; nada de any.\n- Mensagens de commit em português, no imperativo.\n- Nunca rode migrações no banco de produção."}
          value={current}
          onChange={(e) => {
            setText(e.target.value);
            setSaved(false);
          }}
        />
        <div className="row">
          <button className="button primary small" disabled={!dirty || busy} onClick={() => void save()}>
            Salvar regras
          </button>
          {dirty && (
            <button className="button small" disabled={busy} onClick={() => setText(null)}>
              Descartar
            </button>
          )}
          <span className="meta grow ellipsis mono" title={view.rulesPath}>
            {view.rulesPath}
          </span>
        </div>
        {saved && <div className="inline-notice ok">Regras salvas. Valem a partir da próxima sessão.</div>}
        {error && <div className="inline-error">{error}</div>}
      </section>
      <section className="task-step">
        <h3>Arquivos de instruções do projeto</h3>
        <p className="meta">
          Muitos projetos já trazem instruções para IAs em <code>AGENTS.md</code>, <code>CLAUDE.md</code> ou{" "}
          <code>.orchestrator/rules.md</code>. O Orchestrator lê esses arquivos (nunca os altera) e os envia com as
          regras.
        </p>
        <Check
          checked={view.settings.projectRuleFiles}
          onChange={(v) => void saveSettings({ projectRuleFiles: v })}
        >
          Ler os arquivos de instruções do projeto
        </Check>
        {!view.projectPath && <div className="meta">Abra um projeto para ver os arquivos dele.</div>}
        {view.projectPath && view.projectFiles.length === 0 && (
          <div className="meta">Este projeto não tem nenhum desses arquivos.</div>
        )}
        <ul className="list plain">
          {view.projectFiles.map((file) => (
            <li key={file.path} className="meta">
              <code>{file.name}</code> · {file.chars.toLocaleString("pt-BR")} caracteres
              {file.truncated && " (só o começo é enviado)"}
            </li>
          ))}
        </ul>
      </section>
    </>
  );
}

const EMPTY_SKILL = { name: "", description: "", body: "", previousName: null as string | null };

export function SkillsSection({ guidance }: { guidance: Guidance }) {
  const { view, saveSettings } = guidance;
  const [editing, setEditing] = useState<typeof EMPTY_SKILL | null>(null);
  const [viewing, setViewing] = useState<{ info: SkillInfo; body: string; files: string[] } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      await guidance.load();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const open = (skill: SkillInfo) =>
    void run(async () => {
      const doc = await guidanceApi.skill(skill.name);
      if (skill.source === "orchestrator") {
        setViewing(null);
        setEditing({ name: doc.name, description: doc.description, body: doc.body, previousName: doc.name });
      } else {
        setEditing(null);
        setViewing({ info: skill, body: doc.body, files: doc.files });
      }
    });

  if (!view) return <div className="meta">carregando…</div>;
  const skills = view.skills;
  return (
    <>
      <section>
        <h3>Skills</h3>
        <p className="meta">
          Uma skill é um conjunto de instruções para um tipo de trabalho ("como fazemos deploy", "como revisar um PR",
          "como escrever uma migração"). As IAs recebem só o nome e a descrição de cada uma e leem a skill inteira
          (ferramenta <code>skill.read</code>) quando a tarefa pede. O formato é o do Claude Code: uma pasta com um{" "}
          <code>SKILL.md</code>, então as skills do Claude Code servem aqui também.
        </p>
        <Check checked={view.settings.skillsEnabled} onChange={(v) => void saveSettings({ skillsEnabled: v })}>
          Oferecer skills às IAs
        </Check>
        <Check checked={view.settings.projectSkills} onChange={(v) => void saveSettings({ projectSkills: v })}>
          Incluir as skills do projeto (.orchestrator/skills e .claude/skills)
        </Check>
        <Check checked={view.settings.claudeUserSkills} onChange={(v) => void saveSettings({ claudeUserSkills: v })}>
          Incluir as minhas skills do Claude Code (~/.claude/skills)
        </Check>
      </section>
      <section className="task-step">
        <div className="row">
          <h3 className="grow">Skills encontradas ({skills.length})</h3>
          <button
            className="button small primary"
            disabled={busy}
            onClick={() => {
              setViewing(null);
              setEditing({ ...EMPTY_SKILL });
            }}
          >
            Nova skill
          </button>
        </div>
        {skills.length === 0 && <div className="meta">Nenhuma skill ainda. Crie uma ou adicione ao projeto.</div>}
        <ul className="list plain skill-list">
          {skills.map((skill) => (
            <li key={skill.path} className={`skill-item ${skill.enabled && !skill.shadowed ? "" : "off"}`}>
              <input
                type="checkbox"
                title={skill.enabled ? "Desligar esta skill" : "Ligar esta skill"}
                checked={skill.enabled}
                disabled={busy || skill.shadowed}
                onChange={(e) => void run(async () => void (await guidanceApi.setSkillEnabled(skill.name, e.target.checked)))}
              />
              <div className="grow">
                <div className="title">
                  <button className="link" onClick={() => open(skill)}>
                    {skill.name}
                  </button>{" "}
                  <span className="tag small">{SOURCE_LABELS[skill.source]}</span>
                  {skill.shadowed && <span className="meta"> · outra com o mesmo nome vem antes</span>}
                </div>
                <div className="meta">{skill.description || "(sem descrição: as IAs não sabem quando usar)"}</div>
              </div>
              {skill.source === "orchestrator" &&
                (confirmDelete === skill.name ? (
                  <span className="meta">
                    Apagar?{" "}
                    <button
                      className="link danger"
                      onClick={() =>
                        void run(async () => {
                          await guidanceApi.deleteSkill(skill.name);
                          setConfirmDelete(null);
                          if (editing?.previousName === skill.name) setEditing(null);
                        })
                      }
                    >
                      sim
                    </button>{" "}
                    <button className="link" onClick={() => setConfirmDelete(null)}>
                      não
                    </button>
                  </span>
                ) : (
                  <button className="link" disabled={busy} onClick={() => setConfirmDelete(skill.name)}>
                    Apagar
                  </button>
                ))}
            </li>
          ))}
        </ul>
        {error && <div className="inline-error">{error}</div>}
        <div className="meta mono ellipsis" title={view.skillsDir}>
          Skills do Orchestrator: {view.skillsDir}
        </div>
      </section>
      {editing && (
        <section className="task-step skill-editor">
          <h3>{editing.previousName ? `Editar ${editing.previousName}` : "Nova skill"}</h3>
          <label className="form-row">
            <span>Nome</span>
            <input
              className="mono"
              placeholder="revisar-pr"
              value={editing.name}
              onChange={(e) => setEditing({ ...editing, name: e.target.value.toLowerCase().replace(/\s+/g, "-") })}
            />
          </label>
          <label className="form-row">
            <span>Quando usar</span>
            <input
              placeholder="Ao revisar um pull request antes do merge"
              value={editing.description}
              onChange={(e) => setEditing({ ...editing, description: e.target.value })}
            />
          </label>
          <label className="form-row top">
            <span>Instruções</span>
            <textarea
              className="mono skill-body"
              spellCheck={false}
              placeholder={"# Revisar um PR\n\n1. Leia a descrição e o diff inteiro.\n2. Rode os testes.\n3. ..."}
              value={editing.body}
              onChange={(e) => setEditing({ ...editing, body: e.target.value })}
            />
          </label>
          <div className="row">
            <button
              className="button primary small"
              disabled={busy || !editing.name.trim() || !editing.description.trim()}
              onClick={() =>
                void run(async () => {
                  const saved = await guidanceApi.saveSkill(editing);
                  setEditing({ ...editing, name: saved.name, previousName: saved.name });
                })
              }
            >
              Salvar skill
            </button>
            <button className="button small" disabled={busy} onClick={() => setEditing(null)}>
              Fechar
            </button>
          </div>
        </section>
      )}
      {viewing && (
        <section className="task-step skill-editor">
          <div className="row">
            <h3 className="grow">
              {viewing.info.name} <span className="tag small">{SOURCE_LABELS[viewing.info.source]}</span>
            </h3>
            <button className="button small" onClick={() => setViewing(null)}>
              Fechar
            </button>
          </div>
          <div className="meta mono ellipsis" title={viewing.info.path}>
            {viewing.info.path}
          </div>
          <p className="meta">Esta skill não é do Orchestrator: edite o arquivo no próprio lugar.</p>
          <pre className="skill-preview">{viewing.body}</pre>
          {viewing.files.length > 0 && <div className="meta">Outros arquivos: {viewing.files.join(", ")}</div>}
        </section>
      )}
    </>
  );
}
