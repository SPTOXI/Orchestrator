// Pure helpers of the autonomy screens (unit tested; ADR-0016). The rules
// are the engine's: here we only name, describe and edit them.

import type {
  AutonomyMode,
  AutonomyOverview,
  PolicyRule,
  RuleAccess,
  RuleDecision,
  RuleWhere,
  SessionGrant,
} from "./types";

export const MODE_ORDER: AutonomyMode[] = ["assisted", "autonomous", "unrestricted"];

export const MODE_LABELS: Record<AutonomyMode, string> = {
  assisted: "Assistido",
  autonomous: "Autônomo",
  unrestricted: "Acesso Irrestrito",
};

/** What each mode does, as the mode cards say it. */
export const MODE_SUMMARY: Record<AutonomyMode, string> = {
  assisted:
    "Consultas rodam sozinhas. Toda ação pede a sua autorização, e também ler fora do projeto ou arquivos de ambiente.",
  autonomous:
    "As IAs agem sozinhas conforme as suas regras: o que elas mandam perguntar, pergunta; o que mandam negar, é negado.",
  unrestricted:
    "O Orchestrator não impõe nenhuma política: sem confirmações, sem comandos proibidos. Tudo continua registrado no histórico.",
};

export const DECISION_LABELS: Record<RuleDecision, string> = {
  allow: "Permitir",
  ask: "Perguntar",
  deny: "Negar",
};

export const ACCESS_LABELS: Record<RuleAccess, string> = {
  read: "consultas",
  write: "ações",
};

export const WHERE_LABELS: Record<RuleWhere, string> = {
  inside: "dentro do projeto",
  outside: "fora do projeto",
};

/** Tools judged by command patterns. */
export const COMMAND_TOOLS = ["shell.execute", "process.start", "terminal.write"];

/** The rule as a sentence, as the engine writes it in a reason. */
export function describeRule(rule: PolicyRule): string {
  const parts: string[] = [];
  if (rule.access) parts.push(ACCESS_LABELS[rule.access]);
  const tools = rule.tools.filter((t) => t.trim() !== "");
  if (tools.length > 0 && !tools.includes("*")) parts.push(tools.join(", "));
  if (rule.command) parts.push(`comando \`${rule.command}\``);
  if (rule.path) parts.push(`caminho \`${rule.path}\``);
  if (rule.where) parts.push(WHERE_LABELS[rule.where]);
  const what = parts.length > 0 ? parts.join(" · ") : "qualquer chamada";
  return `${what} → ${DECISION_LABELS[rule.decision].toLowerCase()}`;
}

/** A row of the rules editor, with the fields as the user types them. */
export interface RuleDraft {
  tools: string;
  access: "" | RuleAccess;
  where: "" | RuleWhere;
  command: string;
  path: string;
  decision: RuleDecision;
  note: string;
}

export function toDraft(rule: PolicyRule): RuleDraft {
  return {
    tools: rule.tools.join(", "),
    access: rule.access ?? "",
    where: rule.where ?? "",
    command: rule.command ?? "",
    path: rule.path ?? "",
    decision: rule.decision,
    note: rule.note ?? "",
  };
}

export function fromDraft(draft: RuleDraft): PolicyRule {
  const text = (value: string) => (value.trim() === "" ? null : value.trim());
  return {
    tools: draft.tools
      .split(/[\s,]+/)
      .map((t) => t.trim())
      .filter((t) => t !== ""),
    access: draft.access === "" ? null : draft.access,
    where: draft.where === "" ? null : draft.where,
    command: text(draft.command),
    path: text(draft.path),
    decision: draft.decision,
    note: text(draft.note),
  };
}

export const EMPTY_DRAFT: RuleDraft = {
  tools: "",
  access: "",
  where: "",
  command: "",
  path: "",
  decision: "ask",
  note: "",
};

/** `list` with the item at `index` moved by `delta` (within bounds). */
export function moveItem<T>(list: T[], index: number, delta: number): T[] {
  const target = index + delta;
  if (index < 0 || index >= list.length || target < 0 || target >= list.length) return list;
  const next = [...list];
  const [item] = next.splice(index, 1);
  if (item === undefined) return list;
  next.splice(target, 0, item);
  return next;
}

function normalized(rule: PolicyRule): string {
  const r = fromDraft(toDraft(rule));
  return JSON.stringify([r.tools, r.access, r.where, r.command, r.path, r.decision, r.note]);
}

/** Whether two rule lists say the same (for "alterações não salvas"). */
export function sameRules(a: PolicyRule[], b: PolicyRule[]): boolean {
  return (
    a.length === b.length &&
    a.every((rule, i) => {
      const other = b[i];
      return other !== undefined && normalized(rule) === normalized(other);
    })
  );
}

/** What the `Autonomia` chip shows. */
export function autonomyChip(
  overview: AutonomyOverview | null,
  pending: number,
): { value: string; hint: string; warn: boolean } {
  if (!overview) return { value: "—", hint: "Carregando o modo de autonomia", warn: false };
  const parts = [MODE_LABELS[overview.mode]];
  if (overview.pausedAll) parts.push("pausado");
  if (pending > 0) parts.push(`${pending} ${pending === 1 ? "pedido" : "pedidos"}`);
  const whose = overview.projectId
    ? overview.projectMode
      ? "escolhido para este projeto"
      : "o padrão (este projeto não tem um modo próprio)"
    : "o padrão (nenhum projeto aberto)";
  return {
    value: parts.join(" · "),
    hint: `${MODE_LABELS[overview.mode]}: ${whose}. Clique para ver e mudar.`,
    warn: pending > 0 || overview.pausedAll,
  };
}

/** "há 12 s", "há 3 min", "há 2 h". */
export function waitedFor(since: string, now: number): string {
  const seconds = Math.max(0, Math.round((now - new Date(since).getTime()) / 1000));
  if (seconds < 60) return `há ${seconds} s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `há ${minutes} min`;
  return `há ${Math.round(minutes / 60)} h`;
}

/** What a session grant covers, in one line. */
export function grantLabel(grant: SessionGrant): string {
  const what = grant.command ? `${grant.tool} \`${grant.command}\`` : grant.tool;
  const rule = grant.rule ? ` (regra ${grant.rule} do ${MODE_LABELS[grant.mode]})` : "";
  return `${what}${rule}`;
}

/** Arguments of the "Experimentar" box: a command for command tools, a
 * path for the rest. */
export function trialArgs(tool: string, target: string): Record<string, string> {
  const value = target.trim();
  if (value === "") return {};
  if (tool === "terminal.write") return { data: value };
  if (COMMAND_TOOLS.includes(tool)) return { command: value };
  return { path: value };
}

/** How a request names who asks. */
export function requester(request: { agentTitle: string | null; projectName: string | null }): string {
  const who = request.agentTitle ? `Agente "${request.agentTitle}"` : "Sessão";
  return request.projectName ? `${who} · ${request.projectName}` : who;
}
