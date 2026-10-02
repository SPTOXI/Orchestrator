//! The autonomy gate (ADR-0016): the outermost tool executor of the
//! sessions, so every call an AI makes passes here before anything else.
//!
//! - The user's own calls do not come through the sessions, and a call
//!   that is not an AI's passes untouched.
//! - Unrestricted: nothing is evaluated.
//! - Otherwise the mode's rules decide: allow, ask (a request the user
//!   answers) or deny. A refused call is recorded here, since it never
//!   reaches the runtime.
//! - A pause holds the call; cancelling the turn releases it.

use super::service::{AutonomyService, Reply};
use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::{
    AutonomyMode, CallOrigin, Decision, ToolCall, ToolDefinition, ToolError, ToolErrorKind,
    ToolResult,
};
use orchestrator_providers::ToolExecutor;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct AutonomyGate {
    inner: Arc<dyn ToolExecutor>,
    service: AutonomyService,
}

impl AutonomyGate {
    pub fn new(inner: Arc<dyn ToolExecutor>, service: AutonomyService) -> Self {
        service.learn_tools(&inner.tools());
        Self { inner, service }
    }
}

fn cancelled(message: &str) -> ToolError {
    ToolError::new(ToolErrorKind::Cancelled, message)
}

#[async_trait]
impl ToolExecutor for AutonomyGate {
    fn tools(&self) -> Vec<ToolDefinition> {
        self.inner.tools()
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        self.execute_with(call, CancellationToken::new()).await
    }

    async fn execute_with(&self, call: ToolCall, cancel: CancellationToken) -> ToolResult {
        // Only AIs are gated: nobody asks the user to authorize the user.
        if !matches!(call.origin, CallOrigin::Agent { .. }) {
            return self.inner.execute_with(call, cancel).await;
        }
        // Tools can appear after startup (MCP servers, ADR-0021): an
        // unknown name is looked up again before it is let through. One
        // still unknown changes nothing; the executor says it is unknown.
        let read_only = match self.service.read_only(&call.tool) {
            Some(read_only) => Some(read_only),
            None => {
                self.service.learn_tools(&self.inner.tools());
                self.service.read_only(&call.tool)
            }
        };
        let Some(read_only) = read_only else {
            return self.inner.execute_with(call, cancel).await;
        };
        let started = Utc::now();
        let service = &self.service;
        let context = service.context_of(&call.origin);
        let agent_id = context.agent.as_ref().map(|a| a.id.to_string());

        if !service
            .wait_while_paused(agent_id.as_deref(), &cancel)
            .await
        {
            return service.refused(
                call,
                read_only,
                cancelled("o turno foi cancelado enquanto as IAs estavam pausadas"),
                &context,
                None,
                started,
            );
        }
        // The mode may have changed during the pause.
        let context = service.context_of(&call.origin);
        if context.mode != AutonomyMode::Unrestricted {
            let verdict = service.judge(&call, read_only, &context);
            match verdict.decision {
                Decision::Allow => {}
                Decision::Deny => {
                    let reason = service.reason(context.mode, context.mode_source, &verdict);
                    return service.refused(
                        call,
                        read_only,
                        ToolError::new(ToolErrorKind::Denied, format!("Negado: {reason}")),
                        &context,
                        verdict.rule,
                        started,
                    );
                }
                Decision::Ask if service.granted(&call, &context, &verdict) => {}
                Decision::Ask => {
                    let (id, reply) = service.ask(&call, read_only, &context, &verdict);
                    tokio::select! {
                        answer = reply => match answer {
                            Ok(Reply::Allowed) => {}
                            Ok(Reply::Denied(message)) => {
                                return service.refused(
                                    call,
                                    read_only,
                                    ToolError::new(ToolErrorKind::Denied, message),
                                    &context,
                                    verdict.rule,
                                    started,
                                );
                            }
                            Err(_) => {
                                return service.refused(
                                    call,
                                    read_only,
                                    cancelled("o pedido de autorização foi descartado"),
                                    &context,
                                    verdict.rule,
                                    started,
                                );
                            }
                        },
                        () = cancel.cancelled() => {
                            service.withdraw(&id);
                            return service.refused(
                                call,
                                read_only,
                                cancelled("o turno foi cancelado enquanto esperava autorização"),
                                &context,
                                verdict.rule,
                                started,
                            );
                        }
                    }
                    // Paused while the user was deciding: authorized, but
                    // it still waits.
                    if !service
                        .wait_while_paused(agent_id.as_deref(), &cancel)
                        .await
                    {
                        return service.refused(
                            call,
                            read_only,
                            cancelled("o turno foi cancelado enquanto as IAs estavam pausadas"),
                            &context,
                            verdict.rule,
                            started,
                        );
                    }
                }
            }
        }
        self.inner.execute_with(call, cancel).await
    }
}
