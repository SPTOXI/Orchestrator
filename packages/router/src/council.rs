//! The Council (ADR-0011, ADR-0024): its members analyze a demand
//! together, each one with the project context; one of them writes the
//! joint plan; the first member available carries it out, with the others
//! as its reserves. Models outside the Council are never used. This module
//! holds the records and the texts the members get; `service` runs them.

use crate::score::{ModelRef, Recommendation};
use crate::settings::{CouncilMember, CouncilMode};
use chrono::{DateTime, Utc};
use orchestrator_core::{DeliberationId, TokenUsage};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Longest demand sent to the members.
const TASK_CHARS: usize = 8_000;
/// Longest analysis handed to the synthesis and kept.
const ANALYSIS_CHARS: usize = 6_000;
/// Longest plan kept and sent to the one who carries it out.
const PLAN_CHARS: usize = 8_000;

/// Instructions of every member analyzing a demand.
pub const ANALYSIS_SYSTEM: &str = "Você é membro do Conselho de IAs do Orchestrator. Os membros \
analisam juntos uma demanda de desenvolvimento antes que um deles a execute no projeto. Você não \
tem ferramentas: conte só com a demanda e o contexto do projeto que vêm abaixo, e diga o que \
precisaria ser confirmado no código. Responda em português do Brasil, em Markdown, em até 350 \
palavras, com as seções **Entendimento**, **Abordagem** (passos), **Riscos e dúvidas** e **O que \
verificar no código**. Não escreva o código completo.";

/// Instructions of the member who joins the analyses into one plan.
pub const SYNTHESIS_SYSTEM: &str = "Você é o relator do Conselho de IAs do Orchestrator. Junte as \
análises dos membros num único plano para quem vai executar a demanda: onde concordam, onde \
divergem e qual caminho seguir (com o motivo), e os passos. Não invente o que nenhuma análise \
disse. Responda em português do Brasil, em Markdown, em até 400 palavras, com as seções \
**Consenso**, **Divergências** (e a decisão), **Plano** (passos numerados) e **Cuidados**.";

/// One member's answer in a deliberation made before ADR-0024, when the
/// Council voted for a model. Kept so the old history still reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vote {
    pub member: CouncilMember,
    pub provider_name: String,
    /// Model that answered.
    pub model: Option<String>,
    pub choice: Option<ModelRef>,
    /// The member's order of preference (choice first).
    pub ranking: Vec<ModelRef>,
    pub confidence: Option<f64>,
    pub reason: Option<String>,
    /// Why the vote does not count (no answer, bad JSON, …).
    pub error: Option<String>,
    pub usage: TokenUsage,
    pub duration_ms: u64,
}

/// One member's analysis of the demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub member: CouncilMember,
    pub provider_name: String,
    /// Model that answered.
    pub model: Option<String>,
    pub text: Option<String>,
    /// Why there is no analysis (no answer in time, provider down, …).
    pub error: Option<String>,
    pub usage: TokenUsage,
    pub duration_ms: u64,
}

/// How the plan came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanSource {
    /// A member joined two or more analyses.
    Synthesis,
    /// Only one member answered: its analysis is the plan.
    Single,
    /// Nobody could write the synthesis: the analyses side by side.
    Joined,
}

/// The Council's plan for the one who carries out the demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub text: String,
    pub source: PlanSource,
    /// Who wrote it (the synthesis, or the only analysis).
    pub by: Option<CouncilMember>,
    pub by_name: Option<String>,
    /// Spent by the synthesis.
    pub usage: TokenUsage,
    /// Members that could not write the synthesis, and why.
    pub failures: Vec<String>,
}

/// A member's place in the execution: the first carries out the demand,
/// the others are its reserves, in this order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Seat {
    pub member: CouncilMember,
    pub provider_name: String,
    /// The member's model, or its provider's default.
    pub model_name: String,
    /// Why it moved to the end of the line (failed the analysis, provider
    /// unavailable).
    pub demoted: Option<String>,
}

/// Who made the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DecisionSource {
    Router,
    Council,
}

/// The model a session opens with: the router's best (Council off) or the
/// Council member that carries out the demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    #[serde(flatten)]
    pub model_ref: ModelRef,
    pub provider_name: String,
    pub model_name: String,
    pub source: DecisionSource,
    pub reason: String,
    /// Share of the valid votes that chose this model (deliberations made
    /// before ADR-0024).
    pub agreement: Option<f64>,
}

/// A deliberation: the router's ranking and, with the Council on, the
/// members' analyses, the plan and who carries it out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deliberation {
    pub id: DeliberationId,
    pub created_at: DateTime<Utc>,
    pub task: String,
    pub mode: CouncilMode,
    pub recommendation: Recommendation,
    /// Before ADR-0024: the candidates handed to the Council.
    #[serde(default)]
    pub shortlist: Vec<ModelRef>,
    /// Before ADR-0024: the members' votes.
    #[serde(default)]
    pub votes: Vec<Vote>,
    /// Each member's analysis, in Council order.
    #[serde(default)]
    pub analyses: Vec<Analysis>,
    #[serde(default)]
    pub plan: Option<Plan>,
    /// Who carries out the demand, then the reserves.
    #[serde(default)]
    pub seats: Vec<Seat>,
    /// The project the demand is about.
    #[serde(default)]
    pub project_path: Option<PathBuf>,
    pub decision: Option<Decision>,
    /// Spent by this deliberation (zero when it came from the cache).
    pub usage: TokenUsage,
    pub cached: bool,
    /// The deliberation reused from the cache.
    pub cached_from: Option<DeliberationId>,
    /// What the original deliberation spent (cache hits).
    pub saved_usage: Option<TokenUsage>,
    pub notices: Vec<String>,
    /// Full mode carries out the plan without asking.
    pub auto_apply: bool,
    pub duration_ms: u64,
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// What a member receives to analyze the demand.
pub fn analysis_prompt(task: &str, context: Option<&str>) -> String {
    let mut text = format!("Demanda:\n{}", truncate(task, TASK_CHARS));
    match context.map(str::trim).filter(|c| !c.is_empty()) {
        Some(context) => {
            text.push_str("\n\nContexto do projeto (montado pelo Orchestrator):\n");
            text.push_str(context);
        }
        None => text.push_str("\n\n(Sem contexto do projeto: analise pela demanda.)"),
    }
    text
}

/// An analysis as the plan and the synthesis keep it.
pub fn clip_analysis(text: &str) -> String {
    truncate(text, ANALYSIS_CHARS)
}

/// What the member writing the synthesis receives: the demand and every
/// analysis, with who wrote it.
pub fn synthesis_prompt(task: &str, analyses: &[(String, String)]) -> String {
    let mut text = format!("Demanda:\n{}", truncate(task, TASK_CHARS));
    for (who, analysis) in analyses {
        text.push_str(&format!(
            "\n\n### Análise de {who}\n{}",
            clip_analysis(analysis)
        ));
    }
    text
}

/// The analyses side by side, when nobody could join them.
pub fn joined_plan(analyses: &[(String, String)]) -> String {
    let parts: Vec<String> = analyses
        .iter()
        .map(|(who, analysis)| format!("### Análise de {who}\n{}", clip_analysis(analysis)))
        .collect();
    truncate(&parts.join("\n\n"), PLAN_CHARS)
}

/// A plan as kept.
pub fn clip_plan(text: &str) -> String {
    truncate(text, PLAN_CHARS)
}

/// The first message of the session that carries out the demand: the
/// demand, the Council's plan and the role of the one carrying it out.
pub fn execution_message(task: &str, plan: Option<&Plan>, members: &[String]) -> String {
    let task = task.trim();
    let Some(plan) = plan else {
        return task.to_owned();
    };
    let how = match plan.source {
        PlanSource::Synthesis => format!(
            "os membros ({}) analisaram juntos; síntese de {}",
            members.join(", "),
            plan.by_name.as_deref().unwrap_or("um membro")
        ),
        PlanSource::Single => format!(
            "análise de {}",
            plan.by_name.as_deref().unwrap_or("um membro")
        ),
        PlanSource::Joined => format!("análises de {}", members.join(", ")),
    };
    format!(
        "{task}\n\n---\nPlano do Conselho ({how}):\n\n{}\n\n---\nVocê executa esta demanda pelo \
         Conselho. Siga o plano, confirme no código o que ele supõe e, se discordar de algo, diga \
         por quê antes de seguir outro caminho.",
        plan.text.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(source: PlanSource, by: Option<&str>) -> Plan {
        Plan {
            text: "1. Ler o worker\n2. Aumentar o prazo".into(),
            source,
            by: None,
            by_name: by.map(str::to_owned),
            usage: TokenUsage::default(),
            failures: Vec::new(),
        }
    }

    #[test]
    fn members_get_the_demand_and_the_project_context() {
        let text = analysis_prompt("  Corrigir o timeout ", Some("## PROJECT\nrust"));
        assert_eq!(
            text,
            "Demanda:\nCorrigir o timeout\n\nContexto do projeto (montado pelo Orchestrator):\n## PROJECT\nrust"
        );
        assert!(analysis_prompt("x", None)
            .ends_with("(Sem contexto do projeto: analise pela demanda.)"));
        assert!(analysis_prompt("x", Some("  ")).contains("Sem contexto"));
    }

    #[test]
    fn the_synthesis_sees_every_analysis_with_its_author() {
        let analyses = vec![
            ("Claude".to_owned(), "Usar fila".to_owned()),
            ("Gemini".to_owned(), "Usar cron".to_owned()),
        ];
        let text = synthesis_prompt("Agendar e-mails", &analyses);
        assert_eq!(
            text,
            "Demanda:\nAgendar e-mails\n\n### Análise de Claude\nUsar fila\n\n### Análise de Gemini\nUsar cron"
        );
        assert_eq!(
            joined_plan(&analyses),
            "### Análise de Claude\nUsar fila\n\n### Análise de Gemini\nUsar cron"
        );
        let long = "a".repeat(ANALYSIS_CHARS + 50);
        assert_eq!(clip_analysis(&long).chars().count(), ANALYSIS_CHARS + 1);
    }

    #[test]
    fn the_one_carrying_out_gets_the_demand_and_the_plan() {
        let members = vec!["Claude".to_owned(), "Gemini".to_owned()];
        let text = execution_message(
            "Corrigir o timeout",
            Some(&plan(PlanSource::Synthesis, Some("Claude"))),
            &members,
        );
        assert!(text.starts_with("Corrigir o timeout\n\n---\nPlano do Conselho (os membros (Claude, Gemini) analisaram juntos; síntese de Claude):\n\n1. Ler o worker"), "{text}");
        assert!(text.ends_with("antes de seguir outro caminho."), "{text}");
        let single = execution_message(
            "x",
            Some(&plan(PlanSource::Single, Some("Gemini"))),
            &members,
        );
        assert!(
            single.contains("Plano do Conselho (análise de Gemini)"),
            "{single}"
        );
        // Without a plan, just the demand.
        assert_eq!(execution_message(" x ", None, &members), "x");
    }
}
