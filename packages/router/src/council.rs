//! The Council: AI models deliberate which candidate fits a task
//! (ADR-0011). This module builds the question, reads the answers and adds
//! up the votes; `service` runs the members.

use crate::score::{format_context, Candidate, ModelRef, Recommendation};
use crate::settings::{CouncilMember, CouncilMode};
use chrono::{DateTime, Utc};
use orchestrator_core::{DeliberationId, TokenUsage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Longest task description sent to the members.
const TASK_CHARS: usize = 4_000;
/// Longest reason kept from a member.
const REASON_CHARS: usize = 400;
/// Weight of a vote without a confidence.
const DEFAULT_CONFIDENCE: f64 = 0.7;

/// Instructions of every member.
pub const SYSTEM: &str = "You are a member of the Orchestrator's model Council. \
Choose which AI model should carry out the user's task, from a fixed list of candidates. \
Judge by the task, the activity, the requirements and each model's data: price per million \
tokens, context window, tool support, tags and the router score (a rule-based estimate). \
Prefer the cheapest model that will do the task well; pay for a stronger model only when \
the task needs it.\n\
Answer with ONLY one JSON object and no other text:\n\
{\"choice\": \"<candidate id>\", \"ranking\": [\"<candidate id>\", ...], \
\"confidence\": <number from 0 to 1>, \"reason\": \"<one or two sentences in Brazilian Portuguese>\"}\n\
Use only the candidate ids of the list (c1, c2, ...).";

/// One member's answer.
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

/// Who made the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DecisionSource {
    Router,
    Council,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    #[serde(flatten)]
    pub model_ref: ModelRef,
    pub provider_name: String,
    pub model_name: String,
    pub source: DecisionSource,
    pub reason: String,
    /// Share of the valid votes that chose this model (Council only).
    pub agreement: Option<f64>,
}

/// A deliberation: the router's ranking, the members' votes and the
/// decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deliberation {
    pub id: DeliberationId,
    pub created_at: DateTime<Utc>,
    pub task: String,
    pub mode: CouncilMode,
    pub recommendation: Recommendation,
    /// Candidates handed to the Council (best first).
    pub shortlist: Vec<ModelRef>,
    pub votes: Vec<Vote>,
    pub decision: Option<Decision>,
    /// Spent by this deliberation (zero when it came from the cache).
    pub usage: TokenUsage,
    pub cached: bool,
    /// The deliberation reused from the cache.
    pub cached_from: Option<DeliberationId>,
    /// What the original deliberation spent (cache hits).
    pub saved_usage: Option<TokenUsage>,
    pub notices: Vec<String>,
    /// Full mode applies the decision without asking.
    pub auto_apply: bool,
    pub duration_ms: u64,
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

fn yes_no(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "sim",
        Some(false) => "não",
        None => "não informado",
    }
}

fn price(value: Option<f64>) -> String {
    value.map_or_else(|| "?".into(), |p| format!("{p}"))
}

/// The question a member receives.
pub fn prompt(task: &str, recommendation: &Recommendation, shortlist: &[&Candidate]) -> String {
    let task = task.trim();
    let mut text = String::from("Tarefa:\n");
    text.push_str(if task.is_empty() {
        "(sem descrição; use a atividade)"
    } else {
        task
    });
    let task_text = truncate(&text, TASK_CHARS + 8);
    let mut text = task_text;
    text.push_str(&format!(
        "\n\nAtividade: {} ({})\nRequisitos: ferramentas {} · contexto mínimo {} · preferência {}\n\nCandidatos:\n",
        recommendation.activity.id(),
        recommendation.activity.label(),
        if recommendation.needs_tools { "obrigatórias" } else { "opcionais" },
        recommendation.min_context.map_or_else(|| "—".into(), format_context),
        recommendation.preference.label(),
    ));
    for (i, candidate) in shortlist.iter().enumerate() {
        let tags = if candidate.tags.is_empty() {
            "—".to_owned()
        } else {
            candidate.tags.join(", ")
        };
        text.push_str(&format!(
            "c{} · {} / {} · US$ {} entrada e {} saída por M tokens · contexto {} · ferramentas: {} · etiquetas: {} · nota do roteador {}\n",
            i + 1,
            candidate.provider_name,
            candidate.model_ref.model,
            price(candidate.input_price),
            price(candidate.output_price),
            candidate.context_window.map_or_else(|| "?".into(), format_context),
            yes_no(candidate.supports_tools),
            tags,
            candidate.score,
        ));
    }
    text
}

/// A member's answer, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Ballot {
    /// Index in the shortlist.
    pub choice: usize,
    /// Indexes, choice first, no repeats.
    pub ranking: Vec<usize>,
    pub confidence: Option<f64>,
    pub reason: Option<String>,
}

/// `"c2"`, `"C2"`, `"2"` or `2` → index 1.
fn candidate_index(value: &Value, count: usize) -> Option<usize> {
    let number = match value {
        Value::Number(n) => n.as_u64()?,
        Value::String(s) => {
            let s = s.trim().to_lowercase();
            let digits = s.strip_prefix('c').unwrap_or(&s).trim();
            digits.parse::<u64>().ok()?
        }
        _ => return None,
    };
    let index = usize::try_from(number).ok()?.checked_sub(1)?;
    (index < count).then_some(index)
}

/// The first JSON object in `text` (answers may come in code fences or
/// with text around).
fn first_object(text: &str) -> Result<serde_json::Map<String, Value>, String> {
    let mut last_error = None;
    for (start, _) in text.match_indices('{') {
        let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
        match stream.next() {
            Some(Ok(Value::Object(object))) => return Ok(object),
            Some(Err(err)) => last_error = Some(err.to_string()),
            _ => {}
        }
    }
    Err(match last_error {
        Some(err) => format!("JSON inválido na resposta ({err})"),
        None => "a resposta não trouxe um objeto JSON".into(),
    })
}

/// Reads a member's answer against a shortlist of `count` candidates.
pub fn parse_ballot(text: &str, count: usize) -> Result<Ballot, String> {
    let object = first_object(text)?;
    let mut ranking: Vec<usize> = Vec::new();
    if let Some(Value::Array(items)) = object.get("ranking") {
        for item in items {
            if let Some(index) = candidate_index(item, count) {
                if !ranking.contains(&index) {
                    ranking.push(index);
                }
            }
        }
    }
    let choice = match object.get("choice") {
        Some(value) => candidate_index(value, count).ok_or_else(|| {
            format!(
                "escolheu um candidato fora da lista: {}",
                truncate(&value.to_string(), 40)
            )
        })?,
        None => *ranking
            .first()
            .ok_or_else(|| "a resposta não indicou a escolha (\"choice\")".to_owned())?,
    };
    ranking.retain(|i| *i != choice);
    ranking.insert(0, choice);
    let confidence = object.get("confidence").and_then(Value::as_f64).map(|c| {
        let c = if c > 1.0 && c <= 100.0 { c / 100.0 } else { c };
        c.clamp(0.0, 1.0)
    });
    let reason = object
        .get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| truncate(r, REASON_CHARS));
    Ok(Ballot {
        choice,
        ranking,
        confidence,
        reason,
    })
}

/// Result of adding up the ballots.
#[derive(Debug, Clone, PartialEq)]
pub struct Tally {
    pub winner: usize,
    /// Points per shortlist index.
    pub points: Vec<f64>,
    /// Share of the ballots that chose the winner.
    pub agreement: f64,
}

/// Borda count weighted by confidence (0.2–1): position `p` of a ranking is
/// worth `count - p` points. Ties go to the router's order.
pub fn tally(ballots: &[Ballot], count: usize) -> Option<Tally> {
    if ballots.is_empty() || count == 0 {
        return None;
    }
    let mut points = vec![0.0; count];
    for ballot in ballots {
        let weight = ballot
            .confidence
            .unwrap_or(DEFAULT_CONFIDENCE)
            .clamp(0.2, 1.0);
        for (position, index) in ballot.ranking.iter().enumerate() {
            points[*index] += weight * (count - position) as f64;
        }
    }
    let mut winner = 0;
    for (index, value) in points.iter().enumerate() {
        if *value > points[winner] + 1e-9 {
            winner = index;
        }
    }
    let chose = ballots.iter().filter(|b| b.choice == winner).count();
    Some(Tally {
        winner,
        agreement: chose as f64 / ballots.len() as f64,
        points,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ballot(choice: usize, ranking: &[usize], confidence: Option<f64>) -> Ballot {
        Ballot {
            choice,
            ranking: ranking.to_vec(),
            confidence,
            reason: None,
        }
    }

    #[test]
    fn reads_answers_in_any_wrapping() {
        let plain = r#"{"choice":"c2","ranking":["c2","c1","c3"],"confidence":0.9,"reason":"Barato e bom."}"#;
        assert_eq!(
            parse_ballot(plain, 3).unwrap(),
            Ballot {
                choice: 1,
                ranking: vec![1, 0, 2],
                confidence: Some(0.9),
                reason: Some("Barato e bom.".into())
            }
        );
        let fenced = "Claro!\n```json\n{\"choice\": \"C3\", \"confidence\": 80}\n```\nEspero ter ajudado {:)}";
        let read = parse_ballot(fenced, 3).unwrap();
        assert_eq!(read.choice, 2);
        assert_eq!(read.ranking, vec![2]);
        assert_eq!(read.confidence, Some(0.8));

        // Numbers, repeated and unknown ids in the ranking are tolerated.
        let loose = r#"{"ranking": [2, "c2", "c9", "1"], "reason": "  "}"#;
        let read = parse_ballot(loose, 2).unwrap();
        assert_eq!(
            (read.choice, read.ranking, read.reason),
            (1, vec![1, 0], None)
        );

        // The choice goes first even when the ranking disagrees.
        let read = parse_ballot(r#"{"choice":"c1","ranking":["c2","c1"]}"#, 2).unwrap();
        assert_eq!(read.ranking, vec![0, 1]);
    }

    #[test]
    fn rejects_unusable_answers_with_a_reason() {
        assert_eq!(
            parse_ballot("Eco: qual modelo?", 3).unwrap_err(),
            "a resposta não trouxe um objeto JSON"
        );
        assert!(parse_ballot("{\"choice\": ", 3)
            .unwrap_err()
            .starts_with("JSON inválido"));
        assert_eq!(
            parse_ballot(r#"{"choice":"gpt-5"}"#, 3).unwrap_err(),
            "escolheu um candidato fora da lista: \"gpt-5\""
        );
        assert_eq!(
            parse_ballot(r#"{"choice":"c4"}"#, 3).unwrap_err(),
            "escolheu um candidato fora da lista: \"c4\""
        );
        assert_eq!(
            parse_ballot(r#"{"reason":"sem escolha"}"#, 3).unwrap_err(),
            "a resposta não indicou a escolha (\"choice\")"
        );
    }

    #[test]
    fn tally_weighs_rankings_by_confidence() {
        // One confident vote for c2 beats a hesitant one for c1.
        let ballots = [
            ballot(1, &[1, 0, 2], Some(1.0)),
            ballot(0, &[0, 2, 1], Some(0.3)),
        ];
        let result = tally(&ballots, 3).unwrap();
        assert_eq!(result.winner, 1);
        assert_eq!(result.agreement, 0.5);
        assert!((result.points[1] - (3.0 + 0.3)).abs() < 1e-9);

        // Rankings count: two second places beat one first place.
        let ballots = [
            ballot(0, &[0, 1], None),
            ballot(2, &[2, 1], None),
            ballot(3, &[3, 1], None),
        ];
        assert_eq!(tally(&ballots, 4).unwrap().winner, 1);

        // Ties go to the router's order; one member = the manager.
        assert_eq!(
            tally(&[ballot(1, &[1], None), ballot(0, &[0], None)], 2)
                .unwrap()
                .winner,
            0
        );
        let manager = tally(&[ballot(2, &[2], Some(0.1))], 3).unwrap();
        assert_eq!((manager.winner, manager.agreement), (2, 1.0));
        assert!(tally(&[], 3).is_none());
    }
}
