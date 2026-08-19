//! CLI wrappers for every LLM tool (`code tool <name> …`).
//!
//! Invokes the same pure handlers as `agent::tool_build`. Submit tools are
//! dry-run validation (fresh `SubmitSlot`; no week files written).

use crate::config::Config;
use crate::db::{open_db, open_in_memory};
use crate::email_index::EmailIndex;
use crate::summarize::require_outputs_path;
use crate::tools::ToolCtx;
use crate::tools::get_email::{GetEmailArgs, get_email};
use crate::tools::glob_outputs::{GlobOutputsArgs, glob_outputs};
use crate::tools::grep_emails::{GrepEmailsArgs, grep_emails};
use crate::tools::grep_outputs::{GrepOutputsArgs, grep_outputs};
use crate::tools::list_thread_messages::{ListThreadMessagesArgs, list_thread_messages};
use crate::tools::read_output_file::{ReadOutputFileArgs, read_output_file};
use crate::tools::search_related_threads::{
    SearchRelatedThreadsArgs, normalize_root_set, search_related_threads,
};
use crate::tools::submit::{
    SubmitSlot, SubmitThreadOrder, SubmitThreadSummary, SubmitWeekOverview, submit_thread_order,
    submit_thread_summary, submit_week_overview,
};
use crate::week::{scan_week_dirs, week_window};
use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, Utc};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;

/// Parse `YYYY-MM-DD` for clap flags.
fn parse_ymd(s: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|e| format!("invalid date {s:?}, expected YYYY-MM-DD: {e}"))
}

/// `code tool …` — invoke one LLM tool handler.
#[derive(Parser, Debug)]
#[command(arg_required_else_help = true)]
pub struct ToolCli {
    /// Week ending date for ToolCtx (`YYYY-MM-DD`).
    ///
    /// Defaults to the last complete week under outputs, else UTC today.
    #[arg(long, global = true, value_parser = parse_ymd)]
    pub week: Option<NaiveDate>,

    /// Thread root Message-ID to focus mail tools (`GrepEmails`, `ListThreadMessages`).
    #[arg(long, global = true)]
    pub focus_thread_root: Option<String>,

    /// Override `config.outputs_path`.
    #[arg(long, global = true)]
    pub outputs: Option<PathBuf>,

    /// Restrict `SearchRelatedThreads` to these roots (repeatable).
    #[arg(long, global = true)]
    pub allowed_thread_root: Vec<String>,

    #[command(subcommand)]
    pub command: ToolCommand,
}

#[derive(Subcommand, Debug)]
pub enum ToolCommand {
    /// Regex search over mailing-list subject + body (`GrepEmails`).
    GrepEmails(GrepEmailsCli),
    /// Fetch one full email by Message-ID (`GetEmail`).
    GetEmail(GetEmailCli),
    /// List chronological metadata for a thread (`ListThreadMessages`).
    ListThreadMessages(ListThreadMessagesCli),
    /// Regex search under previous summary outputs (`GrepOutputs`).
    GrepOutputs(GrepOutputsCli),
    /// Glob files under outputs_path (`GlobOutputs`).
    GlobOutputs(GlobOutputsCli),
    /// Read a file under outputs_path (`ReadOutputFile`).
    ReadOutputFile(ReadOutputFileCli),
    /// Find threads with related subjects (`SearchRelatedThreads`).
    SearchRelatedThreads(SearchRelatedThreadsCli),
    /// Dry-run `SubmitThreadOrder` (validate payload; no files written).
    SubmitThreadOrder(SubmitThreadOrderCli),
    /// Dry-run `SubmitThreadSummary` (validate payload; no files written).
    SubmitThreadSummary(SubmitThreadSummaryCli),
    /// Dry-run `SubmitWeekOverview` (validate payload; no files written).
    SubmitWeekOverview(SubmitWeekOverviewCli),
}

#[derive(Args, Debug)]
pub struct GrepEmailsCli {
    /// Regular expression pattern.
    #[arg(long)]
    pattern: String,
    /// Optional thread root Message-ID filter.
    #[arg(long)]
    thread_root_id: Option<String>,
    /// Inclusive start date (`YYYY-MM-DD`).
    #[arg(long, value_parser = parse_ymd)]
    date_from: Option<NaiveDate>,
    /// Inclusive end date (`YYYY-MM-DD`).
    #[arg(long, value_parser = parse_ymd)]
    date_to: Option<NaiveDate>,
    /// Max matching lines to return.
    #[arg(long)]
    max_matches: Option<usize>,
}

#[derive(Args, Debug)]
pub struct GetEmailCli {
    /// Message-ID (with or without angle brackets / leading space).
    #[arg(long)]
    message_id: String,
}

#[derive(Args, Debug)]
pub struct ListThreadMessagesCli {
    /// Thread root Message-ID; defaults to `--focus-thread-root` when set.
    #[arg(long)]
    thread_root_id: Option<String>,
    #[arg(long, value_parser = parse_ymd)]
    date_from: Option<NaiveDate>,
    #[arg(long, value_parser = parse_ymd)]
    date_to: Option<NaiveDate>,
}

#[derive(Args, Debug)]
pub struct GrepOutputsCli {
    /// Regular expression pattern.
    #[arg(long)]
    pattern: String,
    /// Optional glob filter under outputs_path.
    #[arg(long)]
    glob: Option<String>,
    #[arg(long)]
    max_matches: Option<usize>,
}

#[derive(Args, Debug)]
pub struct GlobOutputsCli {
    /// Glob relative to outputs, e.g. `*/thread/*.md`.
    #[arg(long)]
    pattern: String,
}

#[derive(Args, Debug)]
pub struct ReadOutputFileCli {
    /// Path relative to outputs_path.
    #[arg(long)]
    path: String,
}

#[derive(Args, Debug)]
pub struct SearchRelatedThreadsCli {
    #[arg(long)]
    subject: String,
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Args, Debug)]
pub struct SubmitThreadOrderCli {
    /// Ordered thread root Message-IDs (repeatable).
    #[arg(long = "ordered-root-id", required = true)]
    ordered_root_ids: Vec<String>,
    /// Optional short rationale (host logs only in the agent path).
    #[arg(long)]
    notes: Option<String>,
}

#[derive(Args, Debug)]
pub struct SubmitThreadSummaryCli {
    /// Short title for the discussion.
    #[arg(long)]
    title: String,
    /// Markdown body of this week's summary.
    #[arg(long)]
    markdown_body: String,
    /// Key message ids cited (repeatable).
    #[arg(long = "key-message-id")]
    key_message_ids: Vec<String>,
}

#[derive(Args, Debug)]
pub struct SubmitWeekOverviewCli {
    /// One-line headline for the root catalog and week front matter.
    #[arg(long)]
    headline: String,
    /// Markdown body of the week overview.
    #[arg(long)]
    markdown_body: String,
}

enum CtxKind {
    Mail,
    Outputs,
}

impl ToolCommand {
    fn ctx_kind(&self) -> Option<CtxKind> {
        match self {
            Self::GrepEmails(_)
            | Self::GetEmail(_)
            | Self::ListThreadMessages(_)
            | Self::SearchRelatedThreads(_) => Some(CtxKind::Mail),
            Self::GrepOutputs(_) | Self::GlobOutputs(_) | Self::ReadOutputFile(_) => {
                Some(CtxKind::Outputs)
            }
            Self::SubmitThreadOrder(_)
            | Self::SubmitThreadSummary(_)
            | Self::SubmitWeekOverview(_) => None,
        }
    }
}

/// Run one nested tool command and print the handler result to stdout.
pub async fn run(config: Config, cli: ToolCli) -> Result<()> {
    let text = execute(&config, cli).await?;
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
    Ok(())
}

async fn execute(config: &Config, cli: ToolCli) -> Result<String> {
    match cli.command.ctx_kind() {
        None => dispatch_submit(cli.command),
        Some(kind) => {
            let ctx = build_ctx(config, &cli, kind).await?;
            dispatch_read(&ctx, cli.command).await
        }
    }
}

async fn build_ctx(config: &Config, cli: &ToolCli, kind: CtxKind) -> Result<ToolCtx> {
    let outputs_override = cli.outputs.clone();
    let outputs = match kind {
        CtxKind::Outputs => match outputs_override {
            Some(p) => p,
            None => require_outputs_path(&config.outputs_path)
                .context("output tools need --outputs or config.outputs_path")?,
        },
        CtxKind::Mail => outputs_override
            .or_else(|| {
                config
                    .outputs_path
                    .as_ref()
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| PathBuf::from(".")),
    };

    let week = resolve_week_ending(cli.week, Some(outputs.as_path()));

    let (pool, index) = match kind {
        CtxKind::Mail => {
            let pool = open_db(&config.db_path, false).await?;
            info!("Loading email metadata into memory...");
            let index = EmailIndex::load(&pool).await?;
            info!("Loaded {} emails into memory", index.len());
            (pool, Arc::new(index))
        }
        CtxKind::Outputs => {
            let pool = open_in_memory().await?;
            (pool, Arc::new(EmailIndex::empty()))
        }
    };

    let mut ctx = ToolCtx::new(pool, index, outputs, week, week_window(week))
        .with_lore_base(&config.lore_base_url)
        .with_list(config.list.clone())
        .with_focus(cli.focus_thread_root.clone());

    if !cli.allowed_thread_root.is_empty() {
        ctx = ctx.with_allowed_roots(Some(normalize_root_set(&cli.allowed_thread_root)));
    }
    Ok(ctx)
}

fn resolve_week_ending(week: Option<NaiveDate>, outputs: Option<&Path>) -> NaiveDate {
    if let Some(w) = week {
        return w;
    }
    if let Some(dir) = outputs {
        if let Ok((complete, _)) = scan_week_dirs(dir) {
            if let Some(last) = complete.last().copied() {
                return last;
            }
        }
    }
    Utc::now().date_naive()
}

async fn dispatch_read(ctx: &ToolCtx, command: ToolCommand) -> Result<String> {
    let result = match command {
        ToolCommand::GrepEmails(a) => {
            grep_emails(
                ctx,
                GrepEmailsArgs {
                    pattern: a.pattern,
                    thread_root_id: a.thread_root_id,
                    date_from: a.date_from,
                    date_to: a.date_to,
                    max_matches: a.max_matches,
                },
            )
            .await
        }
        ToolCommand::GetEmail(a) => {
            get_email(
                ctx,
                GetEmailArgs {
                    message_id: a.message_id,
                },
            )
            .await
        }
        ToolCommand::ListThreadMessages(a) => {
            list_thread_messages(
                ctx,
                ListThreadMessagesArgs {
                    thread_root_id: a.thread_root_id,
                    date_from: a.date_from,
                    date_to: a.date_to,
                },
            )
            .await
        }
        ToolCommand::GrepOutputs(a) => {
            grep_outputs(
                ctx,
                GrepOutputsArgs {
                    pattern: a.pattern,
                    glob: a.glob,
                    max_matches: a.max_matches,
                },
            )
            .await
        }
        ToolCommand::GlobOutputs(a) => {
            glob_outputs(ctx, GlobOutputsArgs { pattern: a.pattern }).await
        }
        ToolCommand::ReadOutputFile(a) => {
            read_output_file(ctx, ReadOutputFileArgs { path: a.path }).await
        }
        ToolCommand::SearchRelatedThreads(a) => {
            search_related_threads(
                ctx,
                SearchRelatedThreadsArgs {
                    subject: a.subject,
                    limit: a.limit,
                },
            )
            .await
        }
        ToolCommand::SubmitThreadOrder(_)
        | ToolCommand::SubmitThreadSummary(_)
        | ToolCommand::SubmitWeekOverview(_) => {
            unreachable!("submit commands go through dispatch_submit")
        }
    };
    match result {
        Ok(s) => Ok(s),
        Err(e) => bail!("ERROR: {e:#}"),
    }
}

fn dispatch_submit(command: ToolCommand) -> Result<String> {
    match command {
        ToolCommand::SubmitThreadOrder(a) => {
            let slot = SubmitSlot::new();
            let status = submit_thread_order(
                &slot,
                SubmitThreadOrder {
                    ordered_root_ids: a.ordered_root_ids,
                    notes: a.notes,
                },
            );
            finish_submit(status, &slot)
        }
        ToolCommand::SubmitThreadSummary(a) => {
            let slot = SubmitSlot::new();
            let status = submit_thread_summary(
                &slot,
                SubmitThreadSummary {
                    title: a.title,
                    markdown_body: a.markdown_body,
                    key_message_ids: a.key_message_ids,
                },
            );
            finish_submit(status, &slot)
        }
        ToolCommand::SubmitWeekOverview(a) => {
            let slot = SubmitSlot::new();
            let status = submit_week_overview(
                &slot,
                SubmitWeekOverview {
                    headline: a.headline,
                    markdown_body: a.markdown_body,
                },
            );
            finish_submit(status, &slot)
        }
        _ => unreachable!("read commands go through dispatch_read"),
    }
}

fn finish_submit<T: Serialize>(status: String, slot: &SubmitSlot<T>) -> Result<String> {
    if status.starts_with("ERROR:") {
        bail!("{status}");
    }
    let payload = slot.take().context("submit succeeded but slot empty")?;
    let json = serde_json::to_string_pretty(&payload)?;
    Ok(format!("{status}\n{json}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::insert_test_email;
    use clap::Parser;

    fn parse(args: &[&str]) -> ToolCli {
        ToolCli::try_parse_from(args).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn clap_get_email() {
        let cli = parse(&["tool", "get-email", "--message-id", "<x@y>"]);
        match cli.command {
            ToolCommand::GetEmail(a) => assert_eq!(a.message_id, "<x@y>"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(cli.week.is_none());
    }

    #[test]
    fn clap_global_week_after_subcommand() {
        let cli = parse(&[
            "tool",
            "grep-emails",
            "--week",
            "2026-07-20",
            "--pattern",
            "nfsd",
            "--thread-root-id",
            "<a@t>",
        ]);
        assert_eq!(
            cli.week,
            Some(NaiveDate::from_ymd_opt(2026, 7, 20).unwrap())
        );
        match cli.command {
            ToolCommand::GrepEmails(a) => {
                assert_eq!(a.pattern, "nfsd");
                assert_eq!(a.thread_root_id.as_deref(), Some("<a@t>"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn clap_glob_and_submit() {
        let cli = parse(&["tool", "glob-outputs", "--pattern", "*/thread/*.md"]);
        match cli.command {
            ToolCommand::GlobOutputs(a) => assert_eq!(a.pattern, "*/thread/*.md"),
            other => panic!("unexpected {other:?}"),
        }

        let cli = parse(&[
            "tool",
            "submit-thread-order",
            "--ordered-root-id",
            "<a>",
            "--ordered-root-id",
            "<b>",
            "--notes",
            "deps",
        ]);
        match cli.command {
            ToolCommand::SubmitThreadOrder(a) => {
                assert_eq!(a.ordered_root_ids, vec!["<a>", "<b>"]);
                assert_eq!(a.notes.as_deref(), Some("deps"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn resolve_week_explicit_wins() {
        let w = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(resolve_week_ending(Some(w), None), w);
    }

    #[test]
    fn resolve_week_last_complete() {
        let mut root = std::env::temp_dir();
        root.push(format!("lore-tool-week-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("2026-07-13")).unwrap();
        std::fs::write(root.join("2026-07-13").join(".complete"), b"").unwrap();
        std::fs::create_dir_all(root.join("2026-07-20")).unwrap();
        std::fs::write(root.join("2026-07-20").join(".complete"), b"").unwrap();

        let got = resolve_week_ending(None, Some(&root));
        assert_eq!(got, NaiveDate::from_ymd_opt(2026, 7, 20).unwrap());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn dispatch_get_email() {
        let pool = open_in_memory().await.unwrap();
        insert_test_email(
            &pool,
            " <get@test.com>",
            "Hello",
            "a@b",
            "2026-07-18T12:00:00+00:00",
            "line one\n",
            None,
            "[]",
        )
        .await
        .unwrap();
        let index = Arc::new(EmailIndex::load(&pool).await.unwrap());
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, PathBuf::from("/tmp/out"), w, week_window(w));

        let out = dispatch_read(
            &ctx,
            ToolCommand::GetEmail(GetEmailCli {
                message_id: "<get@test.com>".into(),
            }),
        )
        .await
        .unwrap();
        assert!(out.contains("Message-ID: <get@test.com>"));
        assert!(out.contains("line one"));
    }

    #[tokio::test]
    async fn dispatch_glob_outputs() {
        let mut root = std::env::temp_dir();
        root.push(format!("lore-tool-glob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("2026-07-20/thread")).unwrap();
        std::fs::write(root.join("2026-07-20/thread/a.md"), b"x").unwrap();
        let root = root.canonicalize().unwrap();

        let pool = open_in_memory().await.unwrap();
        let index = Arc::new(EmailIndex::empty());
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, root.clone(), w, week_window(w));

        let out = dispatch_read(
            &ctx,
            ToolCommand::GlobOutputs(GlobOutputsCli {
                pattern: "*/thread/*.md".into(),
            }),
        )
        .await
        .unwrap();
        assert!(out.contains("2026-07-20/thread/a.md"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn submit_dry_run_prints_payload() {
        let out = dispatch_submit(ToolCommand::SubmitWeekOverview(SubmitWeekOverviewCli {
            headline: "Busy week".into(),
            markdown_body: "See thread/a.md\n".into(),
        }))
        .unwrap();
        assert!(out.starts_with("submitted\n"));
        assert!(out.contains("\"headline\": \"Busy week\""));
        assert!(out.contains("thread/a.md"));
    }

    #[test]
    fn submit_empty_body_errors() {
        let err = dispatch_submit(ToolCommand::SubmitThreadSummary(SubmitThreadSummaryCli {
            title: "T".into(),
            markdown_body: "  ".into(),
            key_message_ids: vec![],
        }))
        .unwrap_err();
        assert!(err.to_string().contains("markdown_body must be non-empty"));
    }
}
