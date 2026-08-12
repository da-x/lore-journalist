//! Multi-tool session runner with submit-slot completion and hard timeout (KD14).

use crate::tools::submit::SubmitSlot;
use anyhow::{anyhow, bail, Context, Result};
use async_openai::types::ChatCompletionRequestUserMessageContent;
use da_harness::multi_tool::{
    AgentInvocationArgs, InferenceCallback, Tool, UsageCallback, UserRequest,
};
use da_harness::{OpenAIClient, TokenUsage};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{info, warn};

pub const ORDER_AGENT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const THREAD_AGENT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub const WEEK_AGENT_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Run one multi_tool session until `slot` is filled or timeout.
///
/// If `inference` is `Some`, uses `run_without_client` (offline tests).
/// Otherwise requires `client` and calls `run(client)`.
pub async fn run_until_submit<T: Clone + Send + 'static>(
    system_prompt: impl Into<String>,
    user_message: impl Into<String>,
    tools: Vec<Tool>,
    slot: SubmitSlot<T>,
    session_timeout: Duration,
    client: Option<OpenAIClient>,
    inference: Option<InferenceCallback>,
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
            let cb: UsageCallback = Arc::new(|u: TokenUsage| {
                info!(
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
        let client = client.ok_or_else(|| anyhow!("OpenAIClient required without inference_callback"))?;
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
