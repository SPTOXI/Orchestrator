// "Configurações" (ADR-0021): one place for everything that shapes how the
// Orchestrator and its AIs work. New sections live here; the older tabs
// (Autonomia, GitHub, Contexto, Sobre) are shown inside it.

import type { ReactNode } from "react";
import { RulesSection, SkillsSection, useGuidance } from "./GuidanceSections";

export type SettingsSection =
  | "rules"
  | "skills"
  | "mcp"
  | "clis"
  | "offline"
  | "policies"
  | "context"
  | "github"
  | "about";

export const SETTINGS_SECTIONS: Array<{ id: SettingsSection; label: string; group: string }> = [
  { id: "rules", label: "Regras de desenvolvimento", group: "IAs" },
  { id: "skills", label: "Skills", group: "IAs" },
  { id: "policies", label: "Políticas e segredos", group: "IAs" },
  { id: "context", label: "Contexto e compactação", group: "IAs" },
  { id: "clis", label: "Assinaturas (CLI)", group: "Modelos" },
  { id: "offline", label: "Modelos offline", group: "Modelos" },
  { id: "mcp", label: "Servidores MCP", group: "Integrações" },
  { id: "github", label: "GitHub", group: "Integrações" },
  { id: "about", label: "Sobre e atualizações", group: "Orchestrator" },
];

interface Props {
  ready: boolean;
  active: boolean;
  section: SettingsSection;
  onSection: (section: SettingsSection) => void;
  /** Sections drawn by other views (they get `active` for themselves). */
  embedded: (section: SettingsSection, active: boolean) => ReactNode;
}

export function SettingsView({ ready, active, section, onSection, embedded }: Props) {
  const own = section === "rules" || section === "skills";
  const guidance = useGuidance(ready, active && own);
  const groups = [...new Set(SETTINGS_SECTIONS.map((s) => s.group))];
  const label = SETTINGS_SECTIONS.find((s) => s.id === section)?.label ?? "";

  return (
    <div className="editor settings-view" hidden={!active}>
      <nav className="settings-nav" aria-label="Seções das configurações">
        {groups.map((group) => (
          <div key={group}>
            <div className="settings-group">{group}</div>
            {SETTINGS_SECTIONS.filter((s) => s.group === group).map((s) => (
              <button
                key={s.id}
                className={`settings-link ${s.id === section ? "active" : ""}`}
                onClick={() => onSection(s.id)}
              >
                {s.label}
              </button>
            ))}
          </div>
        ))}
      </nav>
      <div className="settings-content">
        {own ? (
          <>
            <div className="editor-toolbar">
              <span className="profile-title">{label}</span>
            </div>
            {guidance.error && <div className="inline-error">{guidance.error}</div>}
            <div className="task-body">
              {section === "rules" ? <RulesSection guidance={guidance} /> : <SkillsSection guidance={guidance} />}
            </div>
          </>
        ) : (
          SETTINGS_SECTIONS.filter((s) => s.id !== "rules" && s.id !== "skills").map((s) => (
            <div key={s.id} className="settings-embedded" hidden={s.id !== section}>
              {embedded(s.id, active && s.id === section)}
            </div>
          ))
        )}
      </div>
    </div>
  );
}

/** A settings section drawn outside this file, with the same header. */
export function SettingsPage({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="editor">
      <div className="editor-toolbar">
        <span className="profile-title">{title}</span>
      </div>
      {children}
    </div>
  );
}
