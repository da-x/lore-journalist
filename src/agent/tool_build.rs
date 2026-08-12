//! Build da-harness `Tool` lists wrapping pure handlers.

use crate::tools::get_email::{get_email, GetEmailArgs};
use crate::tools::glob_outputs::{glob_outputs, GlobOutputsArgs};
use crate::tools::grep_emails::{grep_emails, GrepEmailsArgs};
use crate::tools::grep_outputs::{grep_outputs, GrepOutputsArgs};
use crate::tools::list_thread_messages::{list_thread_messages, ListThreadMessagesArgs};
use crate::tools::read_output_file::{read_output_file, ReadOutputFileArgs};
use crate::tools::search_related_threads::{
    search_related_threads, SearchRelatedThreadsArgs,
};
use crate::tools::submit::{
    submit_thread_order, submit_thread_summary, SubmitSlot, SubmitThreadOrder,
    SubmitThreadSummary, ThreadOrderPayload, ThreadSummaryPayload,
};
use crate::tools::ToolCtx;
use anyhow::Result;
use chrono::NaiveDate;
use da_harness::multi_tool::Tool;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

// ── Schema-facing arg types (tool name = struct name) ─────────────────────

/// Regex search over mailing-list subject + body.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GrepEmails {
    /// Regular expression pattern.
    pub pattern: String,
    /// Optional thread root Message-ID filter.
    pub thread_root_id: Option<String>,
    /// Optional start date YYYY-MM-DD (inclusive).
    pub date_from: Option<String>,
    /// Optional end date YYYY-MM-DD (inclusive day).
    pub date_to: Option<String>,
    /// Max matching lines to return.
    pub max_matches: Option<usize>,
}

/// Fetch one full email by Message-ID.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetEmail {
    /// Message-ID (with or without angle brackets / leading space).
    pub message_id: String,
}

/// List metadata for messages in a thread.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ListThreadMessages {
    /// Thread root Message-ID; defaults to session focus when set.
    pub thread_root_id: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
}

/// Regex search under previous summary outputs.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GrepOutputs {
    pub pattern: String,
    /// Optional glob filter under outputs_path.
    pub glob: Option<String>,
    pub max_matches: Option<usize>,
}

/// Glob files under outputs_path.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GlobOutputs {
    /// Glob relative to outputs, e.g. `*/thread/*.md`.
    pub pattern: String,
}

/// Read a file under outputs_path.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReadOutputFile {
    /// Path relative to outputs_path.
    pub path: String,
}

/// Find threads with related subjects.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SearchRelatedThreads {
    pub subject: String,
    pub limit: Option<usize>,
}

fn parse_date(s: &Option<String>) -> Result<Option<NaiveDate>, String> {
    match s {
        None => Ok(None),
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(Some)
            .map_err(|e| format!("invalid date {s:?}: {e}")),
    }
}

fn mail_tools(ctx: ToolCtx) -> Result<Vec<Tool>> {
    let c1 = ctx.clone();
    let grep = Tool::new(Arc::new(move |args: GrepEmails| {
        let ctx = c1.clone();
        async move {
            let date_from = match parse_date(&args.date_from) {
                Ok(d) => d,
                Err(e) => return Ok(format!("ERROR: {e}")),
            };
            let date_to = match parse_date(&args.date_to) {
                Ok(d) => d,
                Err(e) => return Ok(format!("ERROR: {e}")),
            };
            match grep_emails(
                &ctx,
                GrepEmailsArgs {
                    pattern: args.pattern,
                    thread_root_id: args.thread_root_id,
                    date_from,
                    date_to,
                    max_matches: args.max_matches,
                },
            )
            .await
            {
                Ok(s) => Ok(s),
                Err(e) => Ok(format!("ERROR: {e:#}")),
            }
        }
        .boxed()
    }))?;

    let c2 = ctx.clone();
    let get = Tool::new(Arc::new(move |args: GetEmail| {
        let ctx = c2.clone();
        async move {
            match get_email(
                &ctx,
                GetEmailArgs {
                    message_id: args.message_id,
                },
            )
            .await
            {
                Ok(s) => Ok(s),
                Err(e) => Ok(format!("ERROR: {e:#}")),
            }
        }
        .boxed()
    }))?;

    let c3 = ctx.clone();
    let list = Tool::new(Arc::new(move |args: ListThreadMessages| {
        let ctx = c3.clone();
        async move {
            let date_from = match parse_date(&args.date_from) {
                Ok(d) => d,
                Err(e) => return Ok(format!("ERROR: {e}")),
            };
            let date_to = match parse_date(&args.date_to) {
                Ok(d) => d,
                Err(e) => return Ok(format!("ERROR: {e}")),
            };
            match list_thread_messages(
                &ctx,
                ListThreadMessagesArgs {
                    thread_root_id: args.thread_root_id,
                    date_from,
                    date_to,
                },
            )
            .await
            {
                Ok(s) => Ok(s),
                Err(e) => Ok(format!("ERROR: {e:#}")),
            }
        }
        .boxed()
    }))?;

    Ok(vec![grep, get, list])
}

fn output_tools(ctx: ToolCtx) -> Result<Vec<Tool>> {
    let c1 = ctx.clone();
    let grep = Tool::new(Arc::new(move |args: GrepOutputs| {
        let ctx = c1.clone();
        async move {
            match grep_outputs(
                &ctx,
                GrepOutputsArgs {
                    pattern: args.pattern,
                    glob: args.glob,
                    max_matches: args.max_matches,
                },
            )
            .await
            {
                Ok(s) => Ok(s),
                Err(e) => Ok(format!("ERROR: {e:#}")),
            }
        }
        .boxed()
    }))?;

    let c2 = ctx.clone();
    let glob = Tool::new(Arc::new(move |args: GlobOutputs| {
        let ctx = c2.clone();
        async move {
            match glob_outputs(
                &ctx,
                GlobOutputsArgs {
                    pattern: args.pattern,
                },
            )
            .await
            {
                Ok(s) => Ok(s),
                Err(e) => Ok(format!("ERROR: {e:#}")),
            }
        }
        .boxed()
    }))?;

    let c3 = ctx.clone();
    let read = Tool::new(Arc::new(move |args: ReadOutputFile| {
        let ctx = c3.clone();
        async move {
            match read_output_file(
                &ctx,
                ReadOutputFileArgs { path: args.path },
            )
            .await
            {
                Ok(s) => Ok(s),
                Err(e) => Ok(format!("ERROR: {e:#}")),
            }
        }
        .boxed()
    }))?;

    Ok(vec![grep, glob, read])
}

fn related_tool(ctx: ToolCtx) -> Result<Tool> {
    let c = ctx;
    Tool::new(Arc::new(move |args: SearchRelatedThreads| {
        let ctx = c.clone();
        async move {
            match search_related_threads(
                &ctx,
                SearchRelatedThreadsArgs {
                    subject: args.subject,
                    limit: args.limit,
                },
            )
            .await
            {
                Ok(s) => Ok(s),
                Err(e) => Ok(format!("ERROR: {e:#}")),
            }
        }
        .boxed()
    }))
}

/// Tools for the ordering agent.
pub fn build_order_tools(
    ctx: ToolCtx,
    slot: SubmitSlot<ThreadOrderPayload>,
) -> Result<Vec<Tool>> {
    let mut tools = mail_tools(ctx.clone())?;
    tools.extend(output_tools(ctx.clone())?);
    tools.push(related_tool(ctx)?);

    let submit = Tool::new(Arc::new(move |args: SubmitThreadOrder| {
        let slot = slot.clone();
        async move { Ok(submit_thread_order(&slot, args)) }.boxed()
    }))?;
    tools.push(submit);
    Ok(tools)
}

/// Tools for a per-thread summarization agent (focused).
pub fn build_thread_tools(
    ctx: ToolCtx,
    slot: SubmitSlot<ThreadSummaryPayload>,
) -> Result<Vec<Tool>> {
    let mut tools = mail_tools(ctx.clone())?;
    tools.extend(output_tools(ctx.clone())?);
    tools.push(related_tool(ctx)?);

    let submit = Tool::new(Arc::new(move |args: SubmitThreadSummary| {
        let slot = slot.clone();
        async move { Ok(submit_thread_summary(&slot, args)) }.boxed()
    }))?;
    tools.push(submit);
    Ok(tools)
}
