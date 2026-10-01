//! Context compaction (ADR-0018): the AI summarizes its own conversation,
//! and the summary replaces it.
//!
//! "Simple compaction": the summary request is the conversation itself —
//! same model, system prompt and tools, so it reads the prompt cache —
//! plus the request to summarize. Afterwards nothing of the old transcript
//! is sent again (no native parts, no signed reasoning): the next user
//! message opens with the summary. Only at a turn or tool-round boundary,
//! never with a tool call waiting for its result.

use crate::config::{ModelEntry, ToolMode};
use crate::conversation::{Conversation, Message, Part, Role};
use crate::protocol::{Delta, Stop};
use crate::provider::{as_prompt_messages, retry_notice, ApiProvider, ModelCall};
use crate::tools::parse_prompt_calls;
use orchestrator_core::ToolDefinition;
use orchestrator_providers::{Compaction, CompactionPolicy, ProviderError, TurnContext};

/// Asked after the conversation, as the last user text of the request.
const INSTRUCTION: &str = "The Orchestrator is compacting this conversation to save context: \
everything above will be replaced by your summary, and the work continues from it. \
Summarize the transcript inside <summary></summary> tags. Include relevant information in the \
summary such that this conversation will be continued by a new context window without needing \
to redo work or be reprovided with relevant constraints or context. Be sure to preserve: \
(1) any difficulties or problems that came up, and how they were handled or resolved; \
(2) any possibilities, options, or approaches that were raised, tried, or set aside, and why; \
(3) anything that was asked for, decided, agreed, ruled out, or established as a preference, \
constraint, or boundary - stated exactly; (4) exactly where things stand now - what has been \
covered, settled, or completed so far, including files read or changed; (5) anything still \
open, unresolved, promised, or expected to happen next; (6) specific details that would be \
hard to reconstruct - names, paths, numbers, dates, exact wording, links or references - kept \
exactly. Be complete on these even at the cost of length; keep everything else concise. Weight \
the two voices differently: keep what the user said, asked for, shared, or established \
carefully and close to their own words; your own explanations and reasoning can be condensed \
much further, to what they concluded or produced - as long as nothing in the six items above \
is dropped. Write the summary in the language the user writes in. Do not call any tools while \
writing this summary; respond with text only.";

/// Opens the first user message after a compaction.
const INTRO: &str = "[The Orchestrator compacted the earlier part of this conversation to save \
context. This is the summary you wrote of it:]";

/// After a compaction in the middle of a turn: what the AI does next.
pub(crate) const CONTINUE: &str = "Continue the current task from where the summary leaves off.";

/// About four characters per token: enough to decide when to compact.
pub(crate) fn estimate(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

fn estimate_messages(messages: &[Message]) -> u64 {
    messages
        .iter()
        .flat_map(|m| &m.parts)
        .map(|part| match part {
            Part::Text(text) => estimate(text),
            Part::ToolCall(call) => estimate(&call.name) + estimate(&call.args.to_string()),
            Part::ToolResult(result) => estimate(&result.content),
        })
        .sum()
}

/// Size of the next prompt: what the provider reported (plus what was
/// added since), or an estimate when it reports nothing.
pub(crate) fn prompt_size(
    conv: &Conversation,
    system: Option<&str>,
    tools: &[ToolDefinition],
) -> u64 {
    if conv.last_prompt_tokens > 0 {
        return conv.last_prompt_tokens;
    }
    let tools = serde_json::to_string(tools).map_or(0, |t| estimate(&t));
    let summary = match (&conv.summary, conv.messages.is_empty()) {
        (Some(summary), true) => estimate(summary),
        _ => 0,
    };
    system.map_or(0, estimate) + tools + summary + estimate_messages(&conv.messages)
}

/// Whether the conversation passed the limit and has something to compact.
/// After a compaction, it must also have grown by half the limit beyond
/// what compacting cannot remove: when the system prompt, the tools and the
/// summary alone pass the limit, the AI is not asked to summarize again on
/// every round.
pub(crate) fn due(
    policy: &CompactionPolicy,
    model: &ModelEntry,
    conv: &Conversation,
    prompt: u64,
) -> bool {
    let limit = policy.limit(model.context_window);
    policy.auto
        && conv.messages.len() >= 2
        && prompt >= limit
        && prompt >= conv.floor_tokens + limit / 2
}

/// The text inside `<summary>` tags, or the whole reply without them.
pub(crate) fn summary_of(text: &str) -> Option<String> {
    let text = text.trim();
    let inner = match (text.find("<summary>"), text.rfind("</summary>")) {
        (Some(start), Some(end)) if end > start => &text[start + "<summary>".len()..end],
        (Some(start), None) => &text[start + "<summary>".len()..],
        _ => text,
    };
    let inner = inner.trim();
    (!inner.is_empty()).then(|| inner.to_owned())
}

/// A user message; right after a compaction it opens with the summary.
pub(crate) fn user_message(conv: &Conversation, text: &str) -> Message {
    match conv.summary.as_deref().filter(|_| conv.messages.is_empty()) {
        Some(summary) => Message::user(format!(
            "{INTRO}\n\n<summary>\n{summary}\n</summary>\n\n{text}"
        )),
        None => Message::user(text),
    }
}

/// The conversation plus the request to summarize it, as the last user
/// text (merged into the last user message, so roles still alternate).
fn summary_request(conv: &Conversation, mode: ToolMode) -> Vec<Message> {
    let mut messages = if mode == ToolMode::Prompt {
        as_prompt_messages(&conv.messages)
    } else {
        conv.messages.clone()
    };
    match messages.last_mut() {
        Some(last) if last.role == Role::User => last.parts.push(Part::Text(INSTRUCTION.into())),
        _ => messages.push(Message::user(INSTRUCTION)),
    }
    messages
}

/// Where the compaction runs from.
pub(crate) struct Compact<'a> {
    pub reference: &'a str,
    pub model: &'a ModelEntry,
    pub system: Option<&'a str>,
    /// Native tools, the same as the conversation's (the cache prefix).
    pub tools: &'a [ToolDefinition],
    pub mode: ToolMode,
    /// By the threshold, or asked by the user.
    pub automatic: bool,
}

impl ApiProvider {
    /// Replaces `conv`'s messages with a summary written by the model. On
    /// failure the conversation is left as it was.
    pub(crate) async fn compact(
        &self,
        conv: &mut Conversation,
        how: Compact<'_>,
        ctx: &TurnContext,
    ) -> Result<(), ProviderError> {
        let before = prompt_size(conv, how.system, how.tools);
        let request = summary_request(conv, how.mode);
        let quiet = |_: Delta| {};
        let reply = self
            .call_model(
                ModelCall {
                    model: how.model,
                    system: how.system,
                    messages: &request,
                    tools: how.tools,
                    cache_key: Some(how.reference),
                },
                &quiet,
                &|retry| retry_notice(ctx, retry),
                &ctx.cancellation(),
            )
            .await?;
        ctx.report_usage(crate::cost::priced(
            self.connection(),
            how.model,
            reply.usage,
        ));
        if let Stop::Refusal(reason) = &reply.stop {
            return Err(ProviderError::failed(format!(
                "the model declined to summarize ({reason})"
            )));
        }
        let text = match how.mode {
            ToolMode::Prompt => parse_prompt_calls(&reply.text, &mut 0).1,
            _ => reply.text,
        };
        let summary = summary_of(&text)
            .ok_or_else(|| ProviderError::failed("o modelo não devolveu um resumo da conversa"))?;
        let messages = conv.messages.len() as u32;
        conv.messages.clear();
        conv.summary = Some(summary.clone());
        conv.compactions += 1;
        conv.last_prompt_tokens = 0;
        let after = prompt_size(conv, how.system, how.tools);
        conv.floor_tokens = after;
        ctx.compacted(Compaction {
            automatic: how.automatic,
            before_tokens: before,
            after_tokens: after,
            messages,
            summary,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::ToolResultPart;

    #[test]
    fn the_summary_comes_out_of_its_tags() {
        assert_eq!(
            summary_of("Pronto.\n<summary>\nfeito X\n</summary>\n"),
            Some("feito X".into())
        );
        assert_eq!(summary_of("  só texto  "), Some("só texto".into()));
        assert_eq!(summary_of("<summary>cortado"), Some("cortado".into()));
        assert_eq!(summary_of("<summary> </summary>"), None);
        assert_eq!(summary_of(""), None);
    }

    #[test]
    fn the_limit_is_the_smaller_of_tokens_and_window_share() {
        let policy = CompactionPolicy::default();
        assert_eq!(policy.limit(None), 150_000);
        assert_eq!(policy.limit(Some(1_000_000)), 150_000);
        assert_eq!(policy.limit(Some(128_000)), 102_400);
        let model = ModelEntry {
            context_window: Some(100_000),
            ..ModelEntry::new("m")
        };
        let mut conv = Conversation::default();
        assert!(!due(&policy, &model, &conv, 90_000), "nothing to compact");
        conv.messages = vec![Message::user("a"), Message::user("b")];
        assert!(!due(&policy, &model, &conv, 79_999));
        assert!(due(&policy, &model, &conv, 80_000));
        let off = CompactionPolicy {
            auto: false,
            ..policy
        };
        assert!(!due(&off, &model, &conv, 1_000_000));
        // What compacting cannot remove already passes the limit: again
        // only after growing half the limit beyond it.
        conv.floor_tokens = 90_000;
        assert!(!due(&policy, &model, &conv, 120_000));
        assert!(due(&policy, &model, &conv, 130_000));
    }

    #[test]
    fn the_request_ends_with_the_instruction_and_the_summary_opens_what_follows() {
        let mut conv = Conversation {
            messages: vec![
                Message::user("leia"),
                Message {
                    role: Role::User,
                    parts: vec![Part::ToolResult(ToolResultPart {
                        id: "1".into(),
                        name: "filesystem.read".into(),
                        content: "conteúdo".into(),
                        is_error: false,
                        native_id: true,
                    })],
                    native: None,
                },
            ],
            ..Default::default()
        };
        let request = summary_request(&conv, ToolMode::Native);
        assert_eq!(request.len(), 2, "merged into the last user message");
        assert!(matches!(request[1].parts.last(), Some(Part::Text(t)) if t.contains("<summary>")));
        assert_eq!(
            conv.messages[1].parts.len(),
            1,
            "the conversation is untouched"
        );

        conv.messages.push(Message {
            role: Role::Assistant,
            parts: vec![Part::Text("ok".into())],
            native: None,
        });
        assert_eq!(summary_request(&conv, ToolMode::Native).len(), 4);

        assert_eq!(user_message(&conv, "oi").text(), "oi");
        conv.messages.clear();
        conv.summary = Some("tudo até aqui".into());
        let opened = user_message(&conv, "oi").text();
        assert!(opened.contains("<summary>\ntudo até aqui\n</summary>") && opened.ends_with("oi"));
        assert!(prompt_size(&conv, Some("sys"), &[]) >= estimate("tudo até aqui"));
    }
}
