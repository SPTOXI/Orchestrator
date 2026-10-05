//! Reserves of a session (ADR-0024): who takes over when the session's AI
//! fails a turn, and what the one taking over is told — it has none of the
//! conversation, which lives with the provider that failed.

use orchestrator_core::{ProviderId, SessionEvent, SessionLogEntry, TurnId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A provider (and model) that takes over a session whose AI failed a turn.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reserve {
    pub provider: ProviderId,
    /// `None` = the provider's default model.
    #[serde(default)]
    pub model: Option<String>,
}

/// Earlier conversation handed to a reserve, newest kept.
const RECAP_CHARS: usize = 12_000;
/// Longest single message in the recap.
const MESSAGE_CHARS: usize = 2_000;
/// Longest text of the failed attempt in the recap.
const PARTIAL_CHARS: usize = 1_500;
/// Tool calls of the failed attempt listed, newest kept.
const CALLS_LISTED: usize = 30;

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// `filesystem.write config.toml`, `command.run cargo test`: the tool and
/// what it worked on.
fn call_line(tool: &str, args: &Value) -> String {
    let target = ["path", "command", "url", "query", "name"]
        .iter()
        .find_map(|key| args.get(key).and_then(Value::as_str))
        .map(|t| clip(t, 80));
    match target {
        Some(target) => format!("{tool} {target}"),
        None => tool.to_owned(),
    }
}

/// What the reserve receives instead of the bare `input`: who failed and
/// why, the conversation so far, what the failed attempt already did in
/// this turn (`turn_id`) and the input itself.
pub(crate) fn handover_input(
    entries: &[SessionLogEntry],
    turn_id: &TurnId,
    from: &str,
    reason: &str,
    input: &str,
) -> String {
    // Earlier turns: (input, answer), oldest first.
    let mut turns: Vec<(TurnId, String, String)> = Vec::new();
    let mut calls: Vec<(String, Option<bool>)> = Vec::new();
    let mut partial = String::new();
    for entry in entries {
        match &entry.event {
            SessionEvent::TurnStarted { turn_id: id, input } if id != turn_id => {
                turns.push((id.clone(), input.clone(), String::new()));
            }
            SessionEvent::TextDelta { turn_id: id, text } if id == turn_id => {
                partial.push_str(text);
            }
            SessionEvent::TextDelta { turn_id: id, text } => {
                if let Some(turn) = turns.iter_mut().rev().find(|t| &t.0 == id) {
                    turn.2.push_str(text);
                }
            }
            SessionEvent::ToolCallRequested { turn_id: id, call } if id == turn_id => {
                calls.push((call_line(&call.tool, &call.args), None));
            }
            SessionEvent::ToolCallCompleted {
                turn_id: id,
                result,
            } if id == turn_id => {
                if let Some(call) = calls
                    .iter_mut()
                    .rev()
                    .find(|c| c.1.is_none() && c.0.starts_with(&result.tool))
                {
                    call.1 = Some(result.ok);
                }
            }
            _ => {}
        }
    }

    let mut text = format!(
        "[Orchestrator: esta sessão era atendida por {from}, que falhou ({}). Você é a reserva e \
         continua daqui. A conversa ficou com quem falhou; segue um resumo.]",
        clip(reason, 300)
    );

    let mut recap: Vec<String> = Vec::new();
    let mut used = 0;
    for (_, asked, answered) in turns.iter().rev() {
        let mut block = format!("Usuário: {}", clip(asked, MESSAGE_CHARS));
        if !answered.trim().is_empty() {
            block.push_str(&format!("\nIA: {}", clip(answered, MESSAGE_CHARS)));
        }
        used += block.chars().count();
        if used > RECAP_CHARS && !recap.is_empty() {
            break;
        }
        recap.push(block);
    }
    if !recap.is_empty() {
        recap.reverse();
        let skipped = turns.len() - recap.len();
        text.push_str("\n\nConversa até aqui");
        if skipped > 0 {
            text.push_str(&format!(" (as {skipped} primeiras trocas foram omitidas)"));
        }
        text.push_str(":\n");
        text.push_str(&recap.join("\n\n"));
    }

    if !calls.is_empty() || !partial.trim().is_empty() {
        text.push_str(&format!("\n\nNeste pedido, {from} já tinha:"));
        let skipped = calls.len().saturating_sub(CALLS_LISTED);
        if skipped > 0 {
            text.push_str(&format!("\n- (e mais {skipped} chamadas antes)"));
        }
        for (line, ok) in calls.iter().skip(skipped) {
            let status = match ok {
                Some(true) => "ok",
                Some(false) => "erro",
                None => "sem resultado",
            };
            text.push_str(&format!("\n- chamado {line} ({status})"));
        }
        if !partial.trim().is_empty() {
            text.push_str(&format!("\n- escrito: «{}»", clip(&partial, PARTIAL_CHARS)));
        }
        text.push_str(
            "\nConfira o estado atual antes de refazer algo: arquivos e comandos podem já ter \
             mudado o projeto.",
        );
    }

    text.push_str("\n\nPedido:\n");
    text.push_str(input);
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use orchestrator_core::{CallOrigin, ToolCall, ToolResult};
    use serde_json::json;

    fn entry(seq: u64, event: SessionEvent) -> SessionLogEntry {
        SessionLogEntry {
            seq,
            at: Utc::now(),
            event,
        }
    }

    #[test]
    fn the_reserve_learns_the_conversation_and_what_was_already_done() {
        let earlier = TurnId::new();
        let now = TurnId::new();
        let call = ToolCall::new(
            "filesystem.write",
            json!({"path": "src/main.rs", "content": "fn main() {}"}),
            CallOrigin::User,
        );
        let result = ToolResult {
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            output: json!({}),
            error: None,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            duration_ms: 1,
        };
        let entries = vec![
            entry(
                1,
                SessionEvent::TurnStarted {
                    turn_id: earlier.clone(),
                    input: "Crie o main".into(),
                },
            ),
            entry(
                2,
                SessionEvent::TextDelta {
                    turn_id: earlier.clone(),
                    text: "Criei ".into(),
                },
            ),
            entry(
                3,
                SessionEvent::TextDelta {
                    turn_id: earlier,
                    text: "o main.".into(),
                },
            ),
            entry(
                4,
                SessionEvent::TurnStarted {
                    turn_id: now.clone(),
                    input: "Agora os testes".into(),
                },
            ),
            entry(
                5,
                SessionEvent::ToolCallRequested {
                    turn_id: now.clone(),
                    call: call.clone(),
                },
            ),
            entry(
                6,
                SessionEvent::ToolCallCompleted {
                    turn_id: now.clone(),
                    result,
                },
            ),
            entry(
                7,
                SessionEvent::TextDelta {
                    turn_id: now.clone(),
                    text: "Vou rodar".into(),
                },
            ),
        ];
        let text = handover_input(
            &entries,
            &now,
            "Claude",
            "sobrecarregado",
            "Agora os testes",
        );
        assert!(
            text.starts_with(
                "[Orchestrator: esta sessão era atendida por Claude, que falhou (sobrecarregado)."
            ),
            "{text}"
        );
        assert!(
            text.contains("Usuário: Crie o main\nIA: Criei o main."),
            "{text}"
        );
        // The current input is not repeated in the recap.
        assert_eq!(text.matches("Agora os testes").count(), 1, "{text}");
        assert!(
            text.contains("- chamado filesystem.write src/main.rs (ok)"),
            "{text}"
        );
        assert!(text.contains("- escrito: «Vou rodar»"), "{text}");
        assert!(text.ends_with("Pedido:\nAgora os testes"), "{text}");
    }

    #[test]
    fn a_first_turn_that_did_nothing_hands_over_just_the_input() {
        let now = TurnId::new();
        let entries = vec![entry(
            1,
            SessionEvent::TurnStarted {
                turn_id: now.clone(),
                input: "Oi".into(),
            },
        )];
        let text = handover_input(&entries, &now, "Gemini", "http 503", "Oi");
        assert!(!text.contains("Conversa até aqui"), "{text}");
        assert!(!text.contains("já tinha"), "{text}");
        assert!(text.ends_with("]\n\nPedido:\nOi"), "{text}");
    }

    #[test]
    fn long_conversations_keep_the_newest_exchanges() {
        let now = TurnId::new();
        let mut entries = Vec::new();
        for i in 0..20 {
            entries.push(entry(
                i,
                SessionEvent::TurnStarted {
                    turn_id: TurnId::new(),
                    input: format!("pedido {i} {}", "x".repeat(1_500)),
                },
            ));
        }
        let text = handover_input(&entries, &now, "A", "erro", "fim");
        assert!(text.contains("pedido 19"), "the newest stays");
        assert!(!text.contains("pedido 0 "), "the oldest goes");
        assert!(text.contains("primeiras trocas foram omitidas"));
    }
}
