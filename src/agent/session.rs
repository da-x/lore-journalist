//! Multi-tool session runner with submit-slot completion and hard timeout (KD14).
//!
//! If the model replies with text and goes idle without calling submit, the host
//! nudges it (up to [`MAX_IDLE_SUBMIT_NUDGES`] times) and then ends the session
//! as `no submit` instead of waiting out the full timeout. A deadline nudge is
//! also sent at 80% of the timeout so a tool-call loop can still submit.

use crate::tools::submit::SubmitSlot;
use anyhow::{Context, Result, anyhow, bail};
use async_openai::types::{ChatCompletionRequestMessage, ChatCompletionRequestUserMessageContent};
use da_harness::multi_tool::{
    AgentInvocationArgs, InferenceCallback, TaskFuture, Tool, UsageCallback, UserRequest,
};
use da_harness::{OpenAIClient, TokenUsage};
use futures::FutureExt;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tracing::{info, warn};

/// How many times to remind an idle agent to call the submit tool before
/// ending the session as `no submit` instead of waiting out the hard timeout.
pub const MAX_IDLE_SUBMIT_NUDGES: u32 = 2;

/// Fraction of the session timeout after which a still-running agent is told
/// to submit with whatever it has (covers tool-call loops that never go idle).
const DEADLINE_NUDGE_FRACTION: f32 = 0.8;

const SUBMIT_NUDGE: &str = "You have not called the submit tool yet. \
Call it now with the required fields. Do not reply with only text. \
If you already drafted a title and body in a previous message, submit that content.";

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
    let (idle_tx, mut idle_rx) = tokio::sync::mpsc::unbounded_channel();
    let saw_assistant = Arc::new(AtomicBool::new(false));

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
        })
        .messages_push_callback({
            let saw_assistant = saw_assistant.clone();
            let cb: da_harness::multi_tool::MessagesPushCallback =
                Arc::new(move |msg: &ChatCompletionRequestMessage| {
                    if matches!(msg, ChatCompletionRequestMessage::Assistant(_)) {
                        saw_assistant.store(true, Ordering::SeqCst);
                    }
                });
            cb
        })
        .agent_idle_callback({
            let idle_tx = idle_tx;
            let cb: Arc<dyn Fn() -> TaskFuture + Send + Sync> = Arc::new(move || {
                let idle_tx = idle_tx.clone();
                async move {
                    let _ = idle_tx.send(());
                    Ok(())
                }
                .boxed()
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

    let started = Instant::now();
    let deadline_nudge_at = session_timeout.mul_f32(DEADLINE_NUDGE_FRACTION);
    let mut idle_nudges = 0u32;
    let mut sent_deadline_nudge = false;

    let wait = timeout(session_timeout, async {
        loop {
            if slot.is_filled() {
                break;
            }
            if run_handle.is_finished() {
                break;
            }
            tokio::select! {
                biased;
                ev = idle_rx.recv() => {
                    match ev {
                        None => {
                            // Agent dropped the idle sender; wait for the task to exit.
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        Some(()) => {
                            if slot.is_filled() {
                                break;
                            }
                            if !saw_assistant.load(Ordering::SeqCst) {
                                // Idle before the first model turn (waiting for
                                // the initial user message). Not a missed submit.
                                continue;
                            }
                            if idle_nudges >= MAX_IDLE_SUBMIT_NUDGES {
                                warn!(
                                    idle_nudges,
                                    "agent idle without submit; ending session"
                                );
                                break;
                            }
                            idle_nudges += 1;
                            info!(idle_nudges, "nudging idle agent to call submit");
                            if tx
                                .send(UserRequest::Message(
                                    ChatCompletionRequestUserMessageContent::Text(
                                        SUBMIT_NUDGE.to_string(),
                                    ),
                                ))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if sent_deadline_nudge || slot.is_filled() {
                        continue;
                    }
                    if started.elapsed() >= deadline_nudge_at {
                        sent_deadline_nudge = true;
                        info!(
                            elapsed_ms = started.elapsed().as_millis() as u64,
                            "deadline nudge to call submit"
                        );
                        let _ = tx
                            .send(UserRequest::Message(
                                ChatCompletionRequestUserMessageContent::Text(
                                    SUBMIT_NUDGE.to_string(),
                                ),
                            ))
                            .await;
                    }
                }
            }
        }
    })
    .await;

    drop(tx);

    // Once submit is filled we already have the payload; abort so a trailing
    // confirmation turn cannot delay the host. Otherwise give the loop a
    // moment to see the closed incoming channel (fail-fast / timeout).
    let run_res = if slot.is_filled() {
        run_handle.abort();
        match run_handle.await {
            Ok(r) => r,
            Err(e) if e.is_cancelled() => Ok(()),
            Err(e) => return Err(e).context("agent task join"),
        }
    } else {
        let mut run_handle = run_handle;
        tokio::select! {
            joined = &mut run_handle => joined.context("agent task join")?,
            _ = tokio::time::sleep(Duration::from_secs(2)) => {
                warn!("agent still running after session end; aborting task");
                run_handle.abort();
                match run_handle.await {
                    Ok(r) => r,
                    Err(e) if e.is_cancelled() => Ok(()),
                    Err(e) => return Err(e).context("agent task join"),
                }
            }
        }
    };

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
