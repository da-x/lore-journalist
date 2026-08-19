//! Build da-harness `Tool` lists wrapping pure handlers.

use crate::ids::normalize_message_id;
use crate::tools::ToolCtx;
use crate::tools::dedup::RequestDeduper;
use crate::tools::get_email::{GetEmailArgs, get_email};
use crate::tools::glob_outputs::{GlobOutputsArgs, glob_outputs};
use crate::tools::grep_emails::{GrepEmailsArgs, grep_emails};
use crate::tools::grep_outputs::{GrepOutputsArgs, grep_outputs};
use crate::tools::list_thread_messages::{ListThreadMessagesArgs, list_thread_messages};
use crate::tools::read_output_file::{ReadOutputFileArgs, read_output_file};
use crate::tools::search_related_threads::{SearchRelatedThreadsArgs, search_related_threads};
use crate::tools::submit::{
    SubmitSlot, SubmitThreadOrder, SubmitThreadSummary, SubmitWeekOverview, ThreadOrderPayload,
    ThreadSummaryPayload, WeekOverviewPayload, submit_thread_order, submit_thread_summary,
    submit_week_overview,
};
use anyhow::{Result, anyhow};
use chrono::NaiveDate;
use da_harness::multi_tool::Tool;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;

// ── Schema-facing arg types (tool name = struct name) ─────────────────────

/// Regex search over mailing-list subject + body.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetEmail {
    /// Message-ID (with or without angle brackets / leading space).
    pub message_id: String,
}

/// List chronological metadata for messages in a thread (including In-Reply-To).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ListThreadMessages {
    /// Thread root Message-ID; defaults to session focus when set.
    pub thread_root_id: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
}

/// Regex search under previous summary outputs.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GrepOutputs {
    pub pattern: String,
    /// Optional glob filter under outputs_path.
    pub glob: Option<String>,
    pub max_matches: Option<usize>,
}

/// Glob files under outputs_path.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GlobOutputs {
    /// Glob relative to outputs, e.g. `*/thread/*.md`.
    pub pattern: String,
}

/// Read a file under outputs_path.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ReadOutputFile {
    /// Path relative to outputs_path.
    pub path: String,
}

/// Find threads with related subjects.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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

fn trim_opt(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string())
}

fn canon_opt_id(id: Option<String>) -> Option<String> {
    id.map(|s| normalize_message_id(&s))
        .filter(|s| !s.is_empty())
}

impl GrepEmails {
    fn canonicalize(mut self) -> Self {
        self.pattern = self.pattern.trim().to_string();
        self.thread_root_id = canon_opt_id(self.thread_root_id);
        self.date_from = trim_opt(self.date_from);
        self.date_to = trim_opt(self.date_to);
        self
    }
}

impl GetEmail {
    fn canonicalize(mut self) -> Self {
        self.message_id = normalize_message_id(&self.message_id);
        self
    }
}

impl ListThreadMessages {
    fn canonicalize(mut self) -> Self {
        self.thread_root_id = canon_opt_id(self.thread_root_id);
        self.date_from = trim_opt(self.date_from);
        self.date_to = trim_opt(self.date_to);
        self
    }
}

impl GrepOutputs {
    fn canonicalize(mut self) -> Self {
        self.pattern = self.pattern.trim().to_string();
        self.glob = trim_opt(self.glob);
        self
    }
}

impl GlobOutputs {
    fn canonicalize(mut self) -> Self {
        self.pattern = self.pattern.trim().to_string();
        self
    }
}

impl ReadOutputFile {
    fn canonicalize(mut self) -> Self {
        self.path = self.path.trim().to_string();
        self
    }
}

impl SearchRelatedThreads {
    fn canonicalize(mut self) -> Self {
        self.subject = self.subject.trim().to_string();
        self
    }
}

/// Dedup, then run a read handler. Handler errors become model-visible `ERROR:` strings.
async fn guarded<A, F, Fut>(
    deduper: RequestDeduper,
    tool: &'static str,
    args: A,
    run: F,
) -> Result<String>
where
    A: Serialize,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<String>>,
{
    if let Err(e) = deduper.check_or_insert(tool, &args) {
        return Ok(e);
    }
    match run().await {
        Ok(s) => Ok(s),
        Err(e) => Ok(format!("ERROR: {e:#}")),
    }
}

fn mail_tools(ctx: ToolCtx, deduper: RequestDeduper) -> Result<Vec<Tool>> {
    let c1 = ctx.clone();
    let d1 = deduper.clone();
    let grep = Tool::new(Arc::new(move |args: GrepEmails| {
        let ctx = c1.clone();
        let d = d1.clone();
        async move {
            let args = args.canonicalize();
            guarded(d, "GrepEmails", args.clone(), || async move {
                let date_from = parse_date(&args.date_from).map_err(|e| anyhow!("{e}"))?;
                let date_to = parse_date(&args.date_to).map_err(|e| anyhow!("{e}"))?;
                grep_emails(
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
            })
            .await
        }
        .boxed()
    }))?;

    let c2 = ctx.clone();
    let d2 = deduper.clone();
    let get = Tool::new(Arc::new(move |args: GetEmail| {
        let ctx = c2.clone();
        let d = d2.clone();
        async move {
            let args = args.canonicalize();
            guarded(d, "GetEmail", args.clone(), || {
                get_email(
                    &ctx,
                    GetEmailArgs {
                        message_id: args.message_id,
                    },
                )
            })
            .await
        }
        .boxed()
    }))?;

    let c3 = ctx.clone();
    let d3 = deduper.clone();
    let list = Tool::new(Arc::new(move |args: ListThreadMessages| {
        let ctx = c3.clone();
        let d = d3.clone();
        async move {
            let args = args.canonicalize();
            guarded(d, "ListThreadMessages", args.clone(), || async move {
                let date_from = parse_date(&args.date_from).map_err(|e| anyhow!("{e}"))?;
                let date_to = parse_date(&args.date_to).map_err(|e| anyhow!("{e}"))?;
                list_thread_messages(
                    &ctx,
                    ListThreadMessagesArgs {
                        thread_root_id: args.thread_root_id,
                        date_from,
                        date_to,
                    },
                )
                .await
            })
            .await
        }
        .boxed()
    }))?;

    Ok(vec![grep, get, list])
}

fn output_tools(ctx: ToolCtx, deduper: RequestDeduper) -> Result<Vec<Tool>> {
    let c1 = ctx.clone();
    let d1 = deduper.clone();
    let grep = Tool::new(Arc::new(move |args: GrepOutputs| {
        let ctx = c1.clone();
        let d = d1.clone();
        async move {
            let args = args.canonicalize();
            guarded(d, "GrepOutputs", args.clone(), || {
                grep_outputs(
                    &ctx,
                    GrepOutputsArgs {
                        pattern: args.pattern,
                        glob: args.glob,
                        max_matches: args.max_matches,
                    },
                )
            })
            .await
        }
        .boxed()
    }))?;

    let c2 = ctx.clone();
    let d2 = deduper.clone();
    let glob = Tool::new(Arc::new(move |args: GlobOutputs| {
        let ctx = c2.clone();
        let d = d2.clone();
        async move {
            let args = args.canonicalize();
            guarded(d, "GlobOutputs", args.clone(), || {
                glob_outputs(
                    &ctx,
                    GlobOutputsArgs {
                        pattern: args.pattern,
                    },
                )
            })
            .await
        }
        .boxed()
    }))?;

    let c3 = ctx.clone();
    let d3 = deduper.clone();
    let read = Tool::new(Arc::new(move |args: ReadOutputFile| {
        let ctx = c3.clone();
        let d = d3.clone();
        async move {
            let args = args.canonicalize();
            guarded(d, "ReadOutputFile", args.clone(), || {
                read_output_file(&ctx, ReadOutputFileArgs { path: args.path })
            })
            .await
        }
        .boxed()
    }))?;

    Ok(vec![grep, glob, read])
}

fn related_tool(ctx: ToolCtx, deduper: RequestDeduper) -> Result<Tool> {
    Tool::new(Arc::new(move |args: SearchRelatedThreads| {
        let ctx = ctx.clone();
        let d = deduper.clone();
        async move {
            let args = args.canonicalize();
            guarded(d, "SearchRelatedThreads", args.clone(), || {
                search_related_threads(
                    &ctx,
                    SearchRelatedThreadsArgs {
                        subject: args.subject,
                        limit: args.limit,
                    },
                )
            })
            .await
        }
        .boxed()
    }))
}

/// Tools for the ordering agent.
pub fn build_order_tools(ctx: ToolCtx, slot: SubmitSlot<ThreadOrderPayload>) -> Result<Vec<Tool>> {
    let deduper = RequestDeduper::new();
    let mut tools = mail_tools(ctx.clone(), deduper.clone())?;
    tools.extend(output_tools(ctx.clone(), deduper.clone())?);
    tools.push(related_tool(ctx, deduper)?);

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
    let deduper = RequestDeduper::new();
    let mut tools = mail_tools(ctx.clone(), deduper.clone())?;
    tools.extend(output_tools(ctx.clone(), deduper.clone())?);
    tools.push(related_tool(ctx, deduper)?);

    let submit = Tool::new(Arc::new(move |args: SubmitThreadSummary| {
        let slot = slot.clone();
        async move { Ok(submit_thread_summary(&slot, args)) }.boxed()
    }))?;
    tools.push(submit);
    Ok(tools)
}

/// Tools for the week overview agent (outputs only + submit).
pub fn build_week_tools(ctx: ToolCtx, slot: SubmitSlot<WeekOverviewPayload>) -> Result<Vec<Tool>> {
    let deduper = RequestDeduper::new();
    let mut tools = output_tools(ctx, deduper)?;
    let submit = Tool::new(Arc::new(move |args: SubmitWeekOverview| {
        let slot = slot.clone();
        async move { Ok(submit_week_overview(&slot, args)) }.boxed()
    }))?;
    tools.push(submit);
    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_email_fingerprint_normalizes_id() {
        let d = RequestDeduper::new();
        let a = GetEmail {
            message_id: " <x@y> ".into(),
        }
        .canonicalize();
        let b = GetEmail {
            message_id: "<x@y>".into(),
        }
        .canonicalize();
        assert!(d.check_or_insert("GetEmail", &a).is_ok());
        assert!(d.check_or_insert("GetEmail", &b).is_err());
    }

    #[test]
    fn list_thread_empty_id_collapses_to_none() {
        let d = RequestDeduper::new();
        let a = ListThreadMessages {
            thread_root_id: Some("  ".into()),
            date_from: None,
            date_to: None,
        }
        .canonicalize();
        let b = ListThreadMessages {
            thread_root_id: None,
            date_from: None,
            date_to: None,
        }
        .canonicalize();
        assert!(a.thread_root_id.is_none());
        assert!(d.check_or_insert("ListThreadMessages", &a).is_ok());
        assert!(d.check_or_insert("ListThreadMessages", &b).is_err());
    }
}
