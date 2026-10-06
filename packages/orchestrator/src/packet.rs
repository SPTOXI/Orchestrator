//! The `HandoffPacket` (ADR-0013): limits, the facts from the history, the
//! source AI's answer and how the next AI reads it.

use crate::text::{clip, line, relative, when};
use orchestrator_core::{Handoff, HandoffPacket};
use orchestrator_memory::{Decision, DecisionStatus, SessionFacts, WorkingCommand};
use serde_json::Value;
use std::cell::Cell;
use std::path::Path;

/// Items per list.
pub const MAX_ITEMS: usize = 15;
/// Characters per list item.
pub const MAX_ITEM_CHARS: usize = 300;
/// Characters of `goal`, `status` and `nextAction`.
pub const MAX_TEXT_CHARS: usize = 600;

/// What the source AI is asked when it writes the narrative of a handoff.
/// English, like the rest of the instructions; the answer comes in the
/// conversation's language.
pub const AGENT_PROMPT: &str = "The user is handing this work over to another AI, which will \
NOT see this conversation. Reply with ONLY a JSON object (no prose, no code fence), written in \
the language of this conversation, with these fields:\n\
{\"goal\": \"what the work is for\", \"status\": \"where it stands now\", \
\"completed\": [\"done items\"], \"remaining\": [\"items still to do\"], \
\"errors\": [\"open problems\"], \"decisions\": [\"decisions taken and why\"], \
\"tests\": [\"tests run or needed, with results\"], \
\"nextAction\": \"the very next concrete step\"}\n\
Be specific and brief: at most 8 items per list, one line each.";

/// The examples in `AGENT_PROMPT`. An answer that repeats them (an echo, or
/// a model quoting the request) says nothing about the work.
const PLACEHOLDERS: &[&str] = &[
    "what the work is for",
    "where it stands now",
    "done items",
    "items still to do",
    "open problems",
    "decisions taken and why",
    "tests run or needed, with results",
    "the very next concrete step",
];

fn is_placeholder(text: &str) -> bool {
    let text = text.trim();
    PLACEHOLDERS.iter().any(|p| text.eq_ignore_ascii_case(p))
}

fn clean_list(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let item = line(
            item.trim().trim_start_matches(['-', '*', '•']).trim(),
            MAX_ITEM_CHARS,
        );
        if !item.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&item)) {
            out.push(item);
        }
        if out.len() == MAX_ITEMS {
            break;
        }
    }
    out
}

/// Trims every field, drops blank and repeated items, and applies the
/// limits. Applied to drafts and before saving.
pub fn normalize(packet: HandoffPacket) -> HandoffPacket {
    HandoffPacket {
        goal: clip(packet.goal.trim(), MAX_TEXT_CHARS),
        status: clip(packet.status.trim(), MAX_TEXT_CHARS),
        completed: clean_list(packet.completed),
        remaining: clean_list(packet.remaining),
        files: clean_list(packet.files),
        commands: clean_list(packet.commands),
        errors: clean_list(packet.errors),
        decisions: clean_list(packet.decisions),
        tests: clean_list(packet.tests),
        next_action: clip(packet.next_action.trim(), MAX_TEXT_CHARS),
    }
}

/// True for commands of known test runners (`pnpm test`, `cargo test`,
/// `pytest`, `npm run test:unit`…).
pub fn is_test_command(command: &str) -> bool {
    const RUNNERS: &[&str] = &[
        "pytest",
        "jest",
        "vitest",
        "mocha",
        "rspec",
        "phpunit",
        "ctest",
        "playwright",
        "cypress",
        "nextest",
    ];
    command
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word == "test" || word == "tests" || RUNNERS.contains(&word))
}

fn outcome(command: &WorkingCommand) -> String {
    if command.background {
        return "em segundo plano".into();
    }
    match command.exit_code {
        Some(0) => "ok".into(),
        Some(code) => format!("saída {code}"),
        None => "sem código".into(),
    }
}

fn change_label(change: &str) -> &str {
    match change {
        "created" => "criado",
        "modified" | "written" => "modificado",
        "deleted" => "removido",
        "moved" => "movido",
        other => other,
    }
}

pub fn decision_status_label(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Proposed => "proposta",
        DecisionStatus::Accepted => "aceita",
        DecisionStatus::Superseded => "substituída",
        DecisionStatus::Rejected => "rejeitada",
    }
}

/// The part of a packet the history proves: goal (first message), files,
/// commands, tests, errors and the decisions taken while the session was
/// open. Costs no tokens.
pub fn from_facts(facts: &SessionFacts, root: &Path, decisions: &[Decision]) -> HandoffPacket {
    let goal = facts
        .inputs
        .iter()
        .find(|input| !input.starts_with(AGENT_PROMPT_MARK))
        .cloned()
        .unwrap_or_else(|| facts.title.clone());
    let files = facts
        .files
        .iter()
        .map(|f| format!("{} ({})", relative(&f.path, root), change_label(&f.change)))
        .collect();
    let commands = facts
        .commands
        .iter()
        .map(|c| format!("{} → {}", line(&c.command, 200), outcome(c)))
        .collect();
    let tests = facts
        .commands
        .iter()
        .filter(|c| is_test_command(&c.command))
        .map(|c| {
            let verdict = match c.exit_code {
                Some(0) => " (passou)",
                Some(_) => " (falhou)",
                None => "",
            };
            format!("{} → {}{verdict}", line(&c.command, 200), outcome(c))
        })
        .collect();
    let errors = facts
        .errors
        .iter()
        .map(|e| match &e.detail {
            Some(detail) => format!("{}: {}", line(&e.summary, 120), line(detail, 160)),
            None => line(&e.summary, 200),
        })
        .collect();
    let decisions = decisions
        .iter()
        .filter(|d| d.updated_at >= facts.created_at)
        .map(|d| format!("{} ({})", d.title, decision_status_label(d.status)))
        .collect();
    normalize(HandoffPacket {
        goal,
        files,
        commands,
        errors,
        decisions,
        tests,
        ..Default::default()
    })
}

/// First words of [`AGENT_PROMPT`]: the handoff request itself is not the
/// session's goal.
const AGENT_PROMPT_MARK: &str = "The user is handing this work over";

fn list_of(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        Some(Value::String(text)) => text.lines().map(str::to_owned).collect(),
        _ => Vec::new(),
    }
}

fn text_of(object: &serde_json::Map<String, Value>, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|k| object.get(*k))
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; "),
            Value::Null => String::new(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

/// Reads the source AI's answer: the first JSON object in the text (code
/// fences and prose around it are tolerated), with the master document's
/// names or common variants.
pub fn parse_agent(text: &str) -> Result<HandoffPacket, String> {
    let start = text
        .find('{')
        .ok_or("a resposta não trouxe um objeto JSON")?;
    let end = text
        .rfind('}')
        .filter(|end| *end > start)
        .ok_or("a resposta não trouxe um objeto JSON")?;
    let value: Value = serde_json::from_str(&text[start..=end])
        .map_err(|e| format!("a resposta não é um JSON válido: {e}"))?;
    let object = value.as_object().ok_or("a resposta não é um objeto JSON")?;
    // The request's own examples are not facts.
    let repeated = Cell::new(false);
    let real = |text: String| {
        if is_placeholder(&text) {
            repeated.set(true);
            String::new()
        } else {
            text
        }
    };
    let text = |keys: &[&str]| real(text_of(object, keys));
    let pick = |keys: &[&str]| -> Vec<String> {
        list_of(keys.iter().find_map(|k| object.get(*k)))
            .into_iter()
            .map(real)
            .filter(|item| !item.is_empty())
            .collect()
    };
    let packet = normalize(HandoffPacket {
        goal: text(&["goal", "objective"]),
        status: text(&["status", "state"]),
        completed: pick(&["completed", "done"]),
        remaining: pick(&["remaining", "todo", "pending"]),
        files: pick(&["files"]),
        commands: pick(&["commands"]),
        errors: pick(&["errors", "problems", "issues"]),
        decisions: pick(&["decisions"]),
        tests: pick(&["tests"]),
        next_action: text(&["nextAction", "next_action", "next"]),
    });
    if packet.status.is_empty()
        && packet.next_action.is_empty()
        && packet.completed.is_empty()
        && packet.remaining.is_empty()
    {
        return Err(if repeated.get() {
            "a resposta só repetiu o modelo do pedido".into()
        } else {
            "a resposta não trouxe os campos do handoff".into()
        });
    }
    Ok(packet)
}

fn union(first: Vec<String>, then: Vec<String>) -> Vec<String> {
    clean_list(first.into_iter().chain(then).collect())
}

/// The AI writes the narrative; the facts stay, with what the AI adds.
pub fn merge(facts: HandoffPacket, agent: HandoffPacket) -> HandoffPacket {
    normalize(HandoffPacket {
        goal: if agent.goal.is_empty() {
            facts.goal
        } else {
            agent.goal
        },
        status: agent.status,
        completed: agent.completed,
        remaining: agent.remaining,
        files: union(facts.files, agent.files),
        commands: union(facts.commands, agent.commands),
        errors: union(agent.errors, facts.errors),
        decisions: union(agent.decisions, facts.decisions),
        tests: union(agent.tests, facts.tests),
        next_action: agent.next_action,
    })
}

/// The packet as the next AI reads it (body of the `HANDOFF` section).
pub fn render(handoff: &Handoff) -> String {
    let packet = &handoff.packet;
    let from = &handoff.from;
    let mut out = format!(
        "From session \"{}\" ({}{}), {}.\nGOAL: {}\n",
        from.title,
        from.provider,
        from.model
            .as_deref()
            .map(|m| format!("/{m}"))
            .unwrap_or_default(),
        when(&handoff.created_at),
        packet.goal
    );
    if !packet.status.is_empty() {
        out.push_str(&format!("STATUS: {}\n", packet.status));
    }
    for (name, items) in packet.lists() {
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("{}:\n", name.to_uppercase()));
        for item in items {
            out.push_str(&format!("- {item}\n"));
        }
    }
    if !packet.next_action.is_empty() {
        out.push_str(&format!("NEXT ACTION: {}\n", packet.next_action));
    }
    out.trim_end().to_owned()
}

/// First message of the session that takes over (the user reads it too).
pub fn first_message(handoff: &Handoff) -> String {
    let next = if handoff.packet.next_action.is_empty() {
        "confirme o estado do trabalho e continue pelo que falta".to_owned()
    } else {
        handoff.packet.next_action.clone()
    };
    format!(
        "Você está assumindo um trabalho iniciado na sessão \"{}\" ({}). O HANDOFF no seu \
         contexto traz o objetivo, o que já foi feito, o que falta e os fatos registrados; a \
         conversa anterior não está disponível. Comece pela próxima ação: {next}",
        handoff.from.title, handoff.from.provider
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use orchestrator_core::{EventKind, HandoffEnd, HandoffId, HandoffStatus, SessionId};
    use orchestrator_memory::{Source, WorkingError, WorkingFile};

    #[test]
    fn test_commands_are_recognized() {
        for yes in [
            "pnpm test",
            "npm run test:unit",
            "cargo test -p x",
            "pytest -q",
            "npx vitest run",
            "go test ./...",
        ] {
            assert!(is_test_command(yes), "{yes}");
        }
        for no in ["pnpm build", "git status", "echo latest", "cat contest.txt"] {
            assert!(!is_test_command(no), "{no}");
        }
    }

    #[test]
    fn agent_answers_are_read_tolerantly() {
        let answer = "Claro! ```json\n{\"goal\": \"Pagamentos\", \"status\": \"50%\", \
             \"completed\": [\"checkout\", \"- checkout\"], \"remaining\": \"webhook\\ncancelamento\", \
             \"next_action\": \"Validar a assinatura\", \"tests\": [3]}\n```";
        let packet = parse_agent(answer).unwrap();
        assert_eq!(packet.goal, "Pagamentos");
        assert_eq!(packet.completed, ["checkout"]);
        assert_eq!(packet.remaining, ["webhook", "cancelamento"]);
        assert_eq!(packet.next_action, "Validar a assinatura");
        assert_eq!(packet.tests, ["3"]);
        assert!(parse_agent("Eco: sem JSON").is_err());
        assert!(parse_agent("{\"foo\": 1}").is_err());
        assert!(parse_agent("{não é json}").is_err());
    }

    #[test]
    fn the_request_template_is_not_an_answer() {
        for placeholder in PLACEHOLDERS {
            assert!(AGENT_PROMPT.contains(placeholder), "{placeholder}");
        }
        // An echo of the request (the `echo` provider, or a model quoting
        // it) gives nothing.
        let err = parse_agent(&format!("Eco: {AGENT_PROMPT}")).unwrap_err();
        assert_eq!(err, "a resposta só repetiu o modelo do pedido");
        // Examples left in a real answer are dropped.
        let packet = parse_agent(
            "{\"goal\": \"Pagamentos\", \"status\": \"Where it stands now\", \
             \"remaining\": [\"items still to do\", \"webhook\"], \"nextAction\": \"Testar\"}",
        )
        .unwrap();
        assert_eq!(packet.goal, "Pagamentos");
        assert!(packet.status.is_empty());
        assert_eq!(packet.remaining, ["webhook"]);
    }

    #[test]
    fn limits_apply() {
        let packet = normalize(HandoffPacket {
            goal: "x".repeat(2_000),
            completed: (0..40).map(|i| format!("item {i}")).collect(),
            errors: vec!["  ".into(), "a".repeat(1_000)],
            ..Default::default()
        });
        assert_eq!(packet.goal.chars().count(), MAX_TEXT_CHARS);
        assert_eq!(packet.completed.len(), MAX_ITEMS);
        assert_eq!(packet.errors.len(), 1);
        assert_eq!(packet.errors[0].chars().count(), MAX_ITEM_CHARS);
    }

    fn facts() -> SessionFacts {
        let now = Utc::now();
        SessionFacts {
            session_id: "s".into(),
            project_id: Some("p".into()),
            title: "Pagamentos".into(),
            created_at: now - Duration::minutes(30),
            inputs: vec!["Implementar pagamentos Stripe".into()],
            files: vec![WorkingFile {
                path: "/p/saas/src/pay.ts".into(),
                change: "modified".into(),
                at: now,
                by: "agent".into(),
            }],
            commands: vec![
                WorkingCommand {
                    command: "pnpm test".into(),
                    exit_code: Some(1),
                    background: false,
                    at: now,
                    by: "user".into(),
                },
                WorkingCommand {
                    command: "pnpm build".into(),
                    exit_code: Some(0),
                    background: false,
                    at: now,
                    by: "agent".into(),
                },
            ],
            errors: vec![WorkingError {
                kind: EventKind::CommandExecuted,
                summary: "pnpm test (exit 1)".into(),
                detail: Some("1 falha".into()),
                at: now,
            }],
        }
    }

    fn decision(title: &str, status: DecisionStatus, minutes_ago: i64) -> Decision {
        let at = Utc::now() - Duration::minutes(minutes_ago);
        Decision {
            id: title.into(),
            project_id: "p".into(),
            title: title.into(),
            context: String::new(),
            decision: String::new(),
            consequences: String::new(),
            status,
            source: Source::User,
            created_at: at,
            updated_at: at,
        }
    }

    #[test]
    fn facts_and_the_agent_narrative_merge() {
        let decisions = [
            decision("Stripe Billing", DecisionStatus::Accepted, 5),
            decision("Filas no Redis", DecisionStatus::Accepted, 600),
        ];
        let from_history = from_facts(&facts(), Path::new("/p/saas"), &decisions);
        assert_eq!(from_history.goal, "Implementar pagamentos Stripe");
        assert_eq!(from_history.files, ["src/pay.ts (modificado)"]);
        assert_eq!(
            from_history.commands,
            ["pnpm test → saída 1", "pnpm build → ok"]
        );
        assert_eq!(from_history.tests, ["pnpm test → saída 1 (falhou)"]);
        assert_eq!(from_history.errors, ["pnpm test (exit 1): 1 falha"]);
        assert_eq!(from_history.decisions, ["Stripe Billing (aceita)"]);
        assert!(from_history.status.is_empty() && from_history.next_action.is_empty());

        let agent = HandoffPacket {
            goal: String::new(),
            status: "50% concluído".into(),
            completed: vec!["checkout".into()],
            remaining: vec!["webhook".into()],
            errors: vec!["Assinatura do webhook inválida".into()],
            tests: vec!["pnpm test → saída 1 (falhou)".into()],
            next_action: "Validar a assinatura".into(),
            ..Default::default()
        };
        let merged = merge(from_history.clone(), agent);
        assert_eq!(merged.goal, from_history.goal);
        assert_eq!(merged.status, "50% concluído");
        assert_eq!(merged.files, from_history.files);
        assert_eq!(
            merged.errors,
            [
                "Assinatura do webhook inválida",
                "pnpm test (exit 1): 1 falha"
            ]
        );
        assert_eq!(merged.tests, ["pnpm test → saída 1 (falhou)"]);
        assert_eq!(merged.next_action, "Validar a assinatura");
    }

    #[test]
    fn the_request_itself_is_not_the_goal() {
        let mut f = facts();
        f.inputs = vec![AGENT_PROMPT.into()];
        assert_eq!(from_facts(&f, Path::new("/p"), &[]).goal, "Pagamentos");
    }

    #[test]
    fn renders_for_the_next_ai() {
        let handoff = Handoff {
            id: HandoffId::new(),
            project_id: None,
            project_path: "/p/saas".into(),
            from: HandoffEnd {
                session_id: SessionId::new(),
                provider: "nuvem".into(),
                model: Some("gpt-medio".into()),
                title: "Pagamentos".into(),
            },
            to: None,
            packet: HandoffPacket {
                goal: "Implementar pagamentos Stripe".into(),
                status: "50% concluído".into(),
                completed: vec!["checkout".into()],
                next_action: "Validar a assinatura".into(),
                ..Default::default()
            },
            status: HandoffStatus::Created,
            by_agent: true,
            created_at: Utc::now(),
            accepted_at: None,
        };
        let text = render(&handoff);
        assert!(text.starts_with("From session \"Pagamentos\" (nuvem/gpt-medio), "));
        assert!(text.contains(
            "\nGOAL: Implementar pagamentos Stripe\nSTATUS: 50% concluído\nCOMPLETED:\n- checkout\nNEXT ACTION: Validar a assinatura"
        ));
        assert!(!text.contains("REMAINING"));
        let first = first_message(&handoff);
        assert!(first.contains("\"Pagamentos\" (nuvem)"));
        assert!(first.ends_with("Comece pela próxima ação: Validar a assinatura"));
    }
}
