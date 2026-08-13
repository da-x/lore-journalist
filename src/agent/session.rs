//! Multi-tool session runner with submit-slot completion and hard timeout (KD14).

use crate::tools::submit::SubmitSlot;
use anyhow::{Context, Result, anyhow, bail};
use async_openai::types::ChatCompletionRequestUserMessageContent;
use da_harness::multi_tool::{
    AgentInvocationArgs, InferenceCallback, Tool, UsageCallback, UserRequest,
};
use da_harness::{OpenAIClient, TokenUsage};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::timeout;
use tracing::{info, warn};

pub const ORDER_AGENT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const THREAD_AGENT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub const WEEK_AGENT_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Which agent session produced a usage increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageStage {
    Order,
    Thread,
    Week,
}

impl UsageStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Order => "order",
            Self::Thread => "thread",
            Self::Week => "week",
        }
    }
}

/// Snapshot of accumulated LLM token counts for one summarize-week run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub prompt: u64,
    pub completion: u64,
    pub total: u64,
    pub order: u64,
    pub thread: u64,
    pub week: u64,
}

#[derive(Debug, Default)]
struct UsageInner {
    prompt: u64,
    completion: u64,
    total: u64,
    order: u64,
    thread: u64,
    week: u64,
}

/// Thread-safe token accumulator shared across order / thread / week sessions.
#[derive(Debug, Clone, Default)]
pub struct UsageTotals {
    inner: Arc<Mutex<UsageInner>>,
}

impl UsageTotals {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, stage: UsageStage, u: TokenUsage) {
        let mut g = self.inner.lock().expect("usage totals mutex");
        g.prompt += u64::from(u.prompt_tokens);
        g.completion += u64::from(u.completion_tokens);
        g.total += u64::from(u.total_tokens);
        let stage_total = u64::from(u.total_tokens);
        match stage {
            UsageStage::Order => g.order += stage_total,
            UsageStage::Thread => g.thread += stage_total,
            UsageStage::Week => g.week += stage_total,
        }
    }

    pub fn snapshot(&self) -> UsageSnapshot {
        let g = self.inner.lock().expect("usage totals mutex");
        UsageSnapshot {
            prompt: g.prompt,
            completion: g.completion,
            total: g.total,
            order: g.order,
            thread: g.thread,
            week: g.week,
        }
    }
}

/// Why a thread (or session) failed — logged on `failed_thread_ids`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFailReason {
    Timeout,
    NoSubmit,
    AgentError,
    Missing,
}

impl SessionFailReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::NoSubmit => "no submit",
            Self::AgentError => "agent error",
            Self::Missing => "missing",
        }
    }
}

impl fmt::Display for SessionFailReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classify a session error by walking the anyhow chain.
pub fn classify_session_error(err: &anyhow::Error) -> SessionFailReason {
    for cause in err.chain() {
        let s = cause.to_string();
        if s.contains("timed out") {
            return SessionFailReason::Timeout;
        }
        if s.contains("without calling submit") {
            return SessionFailReason::NoSubmit;
        }
    }
    SessionFailReason::AgentError
}

/// Run one multi_tool session until `slot` is filled or timeout.
///
/// If `inference` is `Some`, uses `run_without_client` (offline tests).
/// Otherwise requires `client` and calls `run(client)`.
///
/// Token increments are added to `usage` under `stage` and logged per request.
#[allow(clippy::too_many_arguments)]
pub async fn run_until_submit<T: Clone + Send + 'static>(
    system_prompt: impl Into<String>,
    user_message: impl Into<String>,
    tools: Vec<Tool>,
    slot: SubmitSlot<T>,
    session_timeout: Duration,
    client: Option<OpenAIClient>,
    inference: Option<InferenceCallback>,
    usage: UsageTotals,
    stage: UsageStage,
) -> Result<T> {
    let system_prompt = system_prompt.into();
    let user_message = user_message.into();

    let (tx, rx) = tokio::sync::mpsc::channel(8);

    let mut args = AgentInvocationArgs::default()
        .system_prompt(system_prompt)
        .tools(tools)
        .parallel_tools(true)
        .incoming(rx)
        .usage_callback({
            let usage = usage.clone();
            let cb: UsageCallback = Arc::new(move |u: TokenUsage| {
                usage.add(stage, u);
                info!(
                    stage = stage.as_str(),
                    prompt = u.prompt_tokens,
                    completion = u.completion_tokens,
                    total = u.total_tokens,
                    "llm usage"
                );
            });
            cb
        });

    if let Some(cb) = inference.clone() {
        args = args.inference_callback(cb);
    }

    let invocation = args.build().context("build AgentInvocation")?;

    let run_handle = if inference.is_some() {
        tokio::spawn(async move { invocation.run_without_client().await })
    } else {
        let client =
            client.ok_or_else(|| anyhow!("OpenAIClient required without inference_callback"))?;
        tokio::spawn(async move { invocation.run(client).await })
    };

    tx.send(UserRequest::Message(
        ChatCompletionRequestUserMessageContent::Text(user_message),
    ))
    .await
    .context("send user message to agent")?;

    let wait = timeout(session_timeout, async {
        loop {
            if slot.is_filled() {
                break;
            }
            if run_handle.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;

    drop(tx);

    let run_res = run_handle.await.context("agent task join")?;

    match wait {
        Err(_) => {
            warn!("agent session timed out after {:?}", session_timeout);
            bail!("agent session timed out after {session_timeout:?}");
        }
        Ok(()) => {
            if let Err(e) = run_res {
                // Prefer payload if submit happened before loop error.
                if let Some(p) = slot.take() {
                    warn!(error = %e, "agent run ended with error after submit; using payload");
                    return Ok(p);
                }
                return Err(e).context("agent run failed");
            }
            slot.take()
                .ok_or_else(|| anyhow!("agent ended without calling submit tool"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_totals_accumulate_by_stage() {
        let t = UsageTotals::new();
        t.add(
            UsageStage::Order,
            TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 2,
                total_tokens: 12,
                cached_prompt_tokens: None,
            },
        );
        t.add(
            UsageStage::Thread,
            TokenUsage {
                prompt_tokens: 20,
                completion_tokens: 5,
                total_tokens: 25,
                cached_prompt_tokens: None,
            },
        );
        t.add(
            UsageStage::Thread,
            TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cached_prompt_tokens: None,
            },
        );
        t.add(
            UsageStage::Week,
            TokenUsage {
                prompt_tokens: 4,
                completion_tokens: 4,
                total_tokens: 8,
                cached_prompt_tokens: None,
            },
        );
        let s = t.snapshot();
        assert_eq!(s.prompt, 35);
        assert_eq!(s.completion, 12);
        assert_eq!(s.total, 47);
        assert_eq!(s.order, 12);
        assert_eq!(s.thread, 27);
        assert_eq!(s.week, 8);
    }

    #[test]
    fn classify_timeout_through_context() {
        let e = anyhow!("agent session timed out after 15m").context("thread agent for <x@y>");
        assert_eq!(classify_session_error(&e), SessionFailReason::Timeout);
    }

    #[test]
    fn classify_no_submit_through_context() {
        let e =
            anyhow!("agent ended without calling submit tool").context("thread agent for <x@y>");
        assert_eq!(classify_session_error(&e), SessionFailReason::NoSubmit);
    }

    #[test]
    fn classify_agent_error() {
        let e = anyhow!("boom")
            .context("agent run failed")
            .context("thread agent for <x@y>");
        assert_eq!(classify_session_error(&e), SessionFailReason::AgentError);
    }
}
