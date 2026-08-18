//! Host-side weekly pipeline: empty stubs, ordering, serial thread agents, overview, complete.

use crate::agent::order::obtain_thread_order;
use crate::agent::session::{
    SessionFailReason, UsageSnapshot, UsageTotals, classify_session_error,
};
use crate::agent::thread::run_thread_agent;
use crate::agent::week::{all_thread_files_present, run_week_overview_and_finalize};
use crate::config::ListConfig;
use crate::email_index::{EmailIndex, EmailMeta, thread_root_id};
use crate::lock::SummarizeLock;
use crate::outputs::{
    complete_marker_path, ensure_week_layout, read_week_headline, regenerate_root_index,
    thread_markdown_path, write_complete_marker, write_empty_week_index,
};
use crate::tools::ToolCtx;
use crate::week::{
    ResolveWeekOutcome, assert_week_ended, resolve_week_from_outputs, week_window,
};
use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use da_harness::OpenAIClient;
use da_harness::multi_tool::InferenceCallback;
use indicatif::{ProgressBar, ProgressStyle};
use sqlx::SqlitePool;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info, warn};

/// One active thread for the week (in-window message indices into the index).
#[derive(Debug, Clone)]
pub struct ActiveThread {
    pub root_id: String,
    pub subject: String,
    /// Indices into `EmailIndex::emails`, sorted by date.
    pub message_indices: Vec<usize>,
}

/// Select threads that have ≥1 message in the half-open week window for `w`.
pub fn select_active_threads(index: &EmailIndex, w: NaiveDate) -> Vec<ActiveThread> {
    let (start, end_excl) = week_window(w);
    let mut by_root: BTreeMap<String, Vec<usize>> = BTreeMap::new();

    for (idx, msg) in index.emails().iter().enumerate() {
        if msg.date >= start && msg.date < end_excl {
            let root = thread_root_id(msg);
            by_root.entry(root).or_default().push(idx);
        }
    }

    by_root
        .into_iter()
        .map(|(root_id, mut indices)| {
            indices.sort_by_key(|&i| index.emails()[i].date);
            let subject = index.emails()[indices[0]].subject.clone();
            ActiveThread {
                root_id,
                subject,
                message_indices: indices,
            }
        })
        .collect()
}

/// Flat list of in-window messages (sorted by date) — for agents/tools, not disk.
#[allow(dead_code)]
pub fn in_window_messages<'a>(index: &'a EmailIndex, w: NaiveDate) -> Vec<&'a EmailMeta> {
    let (start, end_excl) = week_window(w);
    let mut msgs: Vec<&EmailMeta> = index
        .emails()
        .iter()
        .filter(|m| m.date >= start && m.date < end_excl)
        .collect();
    msgs.sort_by_key(|m| m.date);
    msgs
}

/// Result of a summarize-week run.
#[derive(Debug, Clone)]
pub enum MaterializeResult {
    /// Week already had `.complete`; no work done.
    AlreadyComplete { week: NaiveDate },
    /// Empty week stub written and marked complete.
    EmptyWeekComplete { week: NaiveDate },
    /// Non-empty week: layout ready, threads selected; no message files on disk.
    /// Week left incomplete until agents run.
    WeekPrepared {
        week: NaiveDate,
        message_count: usize,
        thread_count: usize,
    },
    /// Agents finished; week may be complete (overview + `.complete`) or partial.
    AgentsFinished {
        week: NaiveDate,
        threads_ok: usize,
        threads_failed: usize,
        failed_thread_ids: Vec<String>,
        /// True when week overview ran and `.complete` was written.
        week_complete: bool,
        headline: Option<String>,
    },
}

/// Run PR2 preparation for one resolved week (no per-message markdown writes).
pub async fn materialize_week(
    _pool: &SqlitePool,
    index: &EmailIndex,
    outputs_path: &Path,
    week: NaiveDate,
    site_title: &str,
) -> Result<MaterializeResult> {
    assert_week_ended(week)?;

    if complete_marker_path(outputs_path, week).is_file() {
        info!(%week, "week already complete; no-op");
        return Ok(MaterializeResult::AlreadyComplete { week });
    }

    fs::create_dir_all(outputs_path)
        .with_context(|| format!("create outputs_path {}", outputs_path.display()))?;

    ensure_week_layout(outputs_path, week)?;

    let active = select_active_threads(index, week);
    if active.is_empty() {
        info!(%week, summarize_empty_week = true, "no messages in week window; writing empty stub");
        write_empty_week_stub(outputs_path, week, site_title)?;
        return Ok(MaterializeResult::EmptyWeekComplete { week });
    }

    let message_count = active.iter().map(|t| t.message_indices.len()).sum();
    info!(
        %week,
        message_count,
        threads = active.len(),
        "week prepared (cleaned bodies remain in DB for inference; no message markdown on disk)"
    );

    Ok(MaterializeResult::WeekPrepared {
        week,
        message_count,
        thread_count: active.len(),
    })
}

/// Empty-week path: stub index, root catalog, `.complete`.
pub fn write_empty_week_stub(outputs_path: &Path, week: NaiveDate, site_title: &str) -> Result<()> {
    ensure_week_layout(outputs_path, week)?;
    write_empty_week_index(outputs_path, week)?;
    regenerate_root_index(outputs_path, Some((week, "No activity")), site_title)?;
    write_complete_marker(outputs_path, week)?;
    Ok(())
}

/// Options for LLM agents during summarize-week.
pub struct AgentRunOpts {
    pub client: Option<OpenAIClient>,
    pub order_inference: Option<InferenceCallback>,
    pub thread_inference: Option<InferenceCallback>,
    pub week_inference: Option<InferenceCallback>,
    /// If true, skip LLM agents (prepare layout only).
    pub prepare_only: bool,
}

impl Default for AgentRunOpts {
    fn default() -> Self {
        Self {
            client: None,
            order_inference: None,
            thread_inference: None,
            week_inference: None,
            prepare_only: false,
        }
    }
}

fn log_summarize_metrics(
    week: NaiveDate,
    duration_ms: u128,
    threads_total: usize,
    threads_skipped: usize,
    threads_failed: usize,
    empty_week: bool,
    usage: &UsageTotals,
) {
    let UsageSnapshot {
        prompt,
        completion,
        total,
        order,
        thread,
        week: tokens_week,
    } = usage.snapshot();
    info!(
        summarize_week = %week,
        summarize_threads_total = threads_total,
        summarize_threads_skipped = threads_skipped,
        summarize_threads_failed = threads_failed,
        summarize_tokens_prompt = prompt,
        summarize_tokens_completion = completion,
        summarize_tokens_total = total,
        summarize_tokens_order = order,
        summarize_tokens_thread = thread,
        summarize_tokens_week = tokens_week,
        summarize_duration_ms = duration_ms,
        summarize_empty_week = empty_week,
        "summarize-week metrics"
    );
}

/// Full CLI entry for `summarize-week` (holds exclusive flock for the whole run).
pub async fn run_summarize_week(
    pool: &SqlitePool,
    outputs_path: &Path,
    week: Option<&str>,
    start_week: Option<&str>,
    lore_base_url: &str,
    list: &ListConfig,
    opts: AgentRunOpts,
) -> Result<MaterializeResult> {
    let started = Instant::now();
    let usage = UsageTotals::new();

    if !outputs_path.exists() {
        fs::create_dir_all(outputs_path)
            .with_context(|| format!("create outputs_path {}", outputs_path.display()))?;
    }

    // KD13: exclusive non-blocking lock for the entire run.
    let _lock = SummarizeLock::try_acquire(outputs_path)?;

    let outcome = resolve_week_from_outputs(outputs_path, week, start_week)?;
    let w = match outcome {
        ResolveWeekOutcome::AlreadyComplete(w) => {
            info!(summarize_week = %w, "week already complete");
            log_summarize_metrics(w, started.elapsed().as_millis(), 0, 0, 0, false, &usage);
            return Ok(MaterializeResult::AlreadyComplete { week: w });
        }
        ResolveWeekOutcome::Process(w) => w,
    };

    info!(summarize_week = %w, "week resolved");
    assert_week_ended(w)?;

    info!("Loading email index for week ending {w}");
    let index = EmailIndex::load(pool).await?;
    let prep = materialize_week(pool, &index, outputs_path, w, &list.title).await?;
    match &prep {
        MaterializeResult::EmptyWeekComplete { week } => {
            log_summarize_metrics(*week, started.elapsed().as_millis(), 0, 0, 0, true, &usage);
            return Ok(prep);
        }
        MaterializeResult::AlreadyComplete { week } => {
            log_summarize_metrics(*week, started.elapsed().as_millis(), 0, 0, 0, false, &usage);
            return Ok(prep);
        }
        MaterializeResult::WeekPrepared { .. } => {}
        MaterializeResult::AgentsFinished { .. } => return Ok(prep),
    }

    if opts.prepare_only {
        return Ok(prep);
    }

    run_agents_for_week(
        pool,
        &index,
        outputs_path,
        w,
        lore_base_url,
        list,
        opts,
        usage,
        started,
    )
    .await
}

/// Ordering + serial thread agents + week overview + `.complete` when all succeed.
#[allow(clippy::too_many_arguments)]
pub async fn run_agents_for_week(
    pool: &SqlitePool,
    index: &EmailIndex,
    outputs_path: &Path,
    week: NaiveDate,
    lore_base_url: &str,
    list: &ListConfig,
    opts: AgentRunOpts,
    usage: UsageTotals,
    started: Instant,
) -> Result<MaterializeResult> {
    ensure_week_layout(outputs_path, week)?;
    let active = select_active_threads(index, week);
    if active.is_empty() {
        bail!("run_agents_for_week called with no active threads");
    }

    let index = Arc::new(index.clone());
    let ctx = ToolCtx::new(
        pool.clone(),
        index.clone(),
        outputs_path.to_path_buf(),
        week,
        week_window(week),
    )
    .with_lore_base(lore_base_url)
    .with_list(list.clone());

    let order = match obtain_thread_order(
        ctx.clone(),
        week,
        &active,
        opts.client.clone(),
        opts.order_inference.clone(),
        usage.clone(),
    )
    .await
    {
        Ok(order) => order,
        Err(e) => {
            error!(
                summarize_week = %week,
                ordering_failed = true,
                error = %e,
                error_debug = ?e,
                "ordering agent failed"
            );
            log_summarize_metrics(
                week,
                started.elapsed().as_millis(),
                active.len(),
                0,
                0,
                false,
                &usage,
            );
            return Err(e).context("obtain thread order");
        }
    };

    let by_root: BTreeMap<_, _> = active
        .iter()
        .map(|t| (t.root_id.clone(), t.clone()))
        .collect();

    let total = order.len();
    let mut failed: Vec<(String, SessionFailReason)> = Vec::new();
    let mut threads_ok = 0usize;
    let mut threads_skipped = 0usize;

    let pb = ProgressBar::new(total as u64);
    if let Ok(style) = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} threads {msg}",
    ) {
        pb.set_style(style.progress_chars("=>-"));
    }

    for (i, root) in order.iter().enumerate() {
        let position = i + 1;
        let Some(thread) = by_root.get(root) else {
            warn!(%root, "ordered root missing from active set; skipping");
            failed.push((root.clone(), SessionFailReason::Missing));
            pb.inc(1);
            continue;
        };
        if thread_markdown_path(outputs_path, week, root).is_file() {
            info!(root = %root, position, total, "thread skip");
            threads_ok += 1;
            threads_skipped += 1;
            pb.inc(1);
            continue;
        }

        info!(root = %root, position, total, "thread start");
        pb.set_message(format!("{position}/{total}"));
        let result = run_thread_agent(
            ctx.clone(),
            week,
            thread,
            index.as_ref(),
            &order,
            position,
            total,
            opts.client.clone(),
            opts.thread_inference.clone(),
            usage.clone(),
        )
        .await;

        match result {
            Ok(_) => {
                info!(root = %root, "thread end");
                threads_ok += 1;
            }
            Err(e) => {
                let reason = classify_session_error(&e);
                error!(
                    root = %root,
                    reason = reason.as_str(),
                    error = %e,
                    error_debug = ?e,
                    "thread agent failed"
                );
                failed.push((root.clone(), reason));
            }
        }
        pb.inc(1);
    }
    pb.finish_and_clear();

    let threads_failed = failed.len();
    let failed_thread_ids: Vec<String> = failed.iter().map(|(id, _)| id.clone()).collect();
    let failed_reasons: Vec<&'static str> = failed.iter().map(|(_, r)| r.as_str()).collect();
    info!(
        %week,
        threads_ok,
        threads_skipped,
        threads_failed,
        "thread agents finished"
    );

    // KD12: no overview / no .complete unless every expected thread file exists.
    if threads_failed > 0 || !all_thread_files_present(outputs_path, week, &order) {
        warn!(?failed_thread_ids, ?failed_reasons, "failed_thread_ids");
        log_summarize_metrics(
            week,
            started.elapsed().as_millis(),
            total,
            threads_skipped,
            threads_failed,
            false,
            &usage,
        );
        let failed_detail: Vec<String> =
            failed.iter().map(|(id, r)| format!("{id} ({r})")).collect();
        bail!(
            "thread agents incomplete: failed={threads_failed} \
             failed_thread_ids={failed_detail:?}; \
             overview and .complete withheld (re-run summarize-week to resume missing threads)"
        );
    }

    // Always re-run overview while .complete is absent (may rewrite W/index.md).
    run_week_overview_and_finalize(
        ctx,
        week,
        &order,
        &active,
        opts.client.clone(),
        opts.week_inference.clone(),
        usage.clone(),
    )
    .await
    .context("week overview / finalize")?;

    log_summarize_metrics(
        week,
        started.elapsed().as_millis(),
        total,
        threads_skipped,
        0,
        false,
        &usage,
    );

    let headline = read_week_headline(outputs_path, week);
    Ok(MaterializeResult::AgentsFinished {
        week,
        threads_ok,
        threads_failed: 0,
        failed_thread_ids: vec![],
        week_complete: complete_marker_path(outputs_path, week).is_file(),
        headline,
    })
}

/// Require `outputs_path` from config.
pub fn require_outputs_path(config_outputs: &Option<String>) -> Result<PathBuf> {
    let Some(p) = config_outputs.as_ref() else {
        bail!("config.outputs_path is required");
    };
    if p.is_empty() {
        bail!("config.outputs_path is empty");
    }
    Ok(PathBuf::from(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::lore::lore_url_for_message_id;
    use crate::outputs::{format_message_list_lore, root_index_path, week_index_path};
    use crate::week::week_window;
    use chrono::{TimeZone, Utc};

    async fn insert_email(
        pool: &SqlitePool,
        message_id: &str,
        subject: &str,
        date: &str,
        body: &str,
        in_reply_to: Option<&str>,
        references: &str,
    ) {
        let compressed = zstd::encode_all(body.as_bytes(), 3).unwrap();
        let from_addr = "alice@example.com";
        sqlx::query!(
            r#"
            INSERT INTO emails
                (message_id, subject, from_addr, date, body, in_reply_to, "references")
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
            message_id,
            subject,
            from_addr,
            date,
            compressed,
            in_reply_to,
            references,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    fn temp_outputs() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "nfs-sum-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[tokio::test]
    async fn non_empty_week_does_not_write_message_files() {
        let pool = open_in_memory().await.unwrap();
        insert_email(
            &pool,
            " <20260720-tcp-read-sock-v2-6-29545d034f3c@kernel.org>",
            "Test subject",
            "2026-07-18T12:00:00+00:00",
            "hello body\n",
            None,
            "[]",
        )
        .await;

        let index = EmailIndex::load(&pool).await.unwrap();
        let out = temp_outputs();
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();

        let result = materialize_week(&pool, &index, &out, w, "Mailing List Weekly Summaries")
            .await
            .unwrap();
        match result {
            MaterializeResult::WeekPrepared {
                message_count,
                thread_count,
                ..
            } => {
                assert_eq!(message_count, 1);
                assert_eq!(thread_count, 1);
            }
            other => panic!("unexpected {other:?}"),
        }

        let messages_dir = out.join("2026-07-20").join("messages");
        assert!(
            !messages_dir.exists(),
            "must not create messages/ archive directory"
        );
        assert!(out.join("2026-07-20").join("thread").is_dir());
        assert!(!complete_marker_path(&out, w).is_file());

        // Bodies still available for inference.
        let body = index
            .load_body(
                &pool,
                "<20260720-tcp-read-sock-v2-6-29545d034f3c@kernel.org>",
            )
            .await
            .unwrap();
        assert_eq!(body, "hello body\n");

        // Lore link shape for host-built message lists.
        let list = format_message_list_lore(
            "https://lore.kernel.org/linux-nfs/",
            &[(
                "2026-07-18".into(),
                "alice@example.com".into(),
                "Test subject".into(),
                " <20260720-tcp-read-sock-v2-6-29545d034f3c@kernel.org>".into(),
            )],
        );
        assert!(list.contains(
            "https://lore.kernel.org/linux-nfs/20260720-tcp-read-sock-v2-6-29545d034f3c@kernel.org/"
        ));
        assert_eq!(
            lore_url_for_message_id(
                "https://lore.kernel.org/linux-nfs/",
                "<20260720-tcp-read-sock-v2-6-29545d034f3c@kernel.org>"
            ),
            "https://lore.kernel.org/linux-nfs/20260720-tcp-read-sock-v2-6-29545d034f3c@kernel.org/"
        );

        let _ = fs::remove_dir_all(&out);
    }

    #[tokio::test]
    async fn empty_week_writes_stub_and_complete() {
        let pool = open_in_memory().await.unwrap();
        insert_email(
            &pool,
            " <old@example.com>",
            "Old",
            "2026-01-01T00:00:00+00:00",
            "old\n",
            None,
            "[]",
        )
        .await;

        let index = EmailIndex::load(&pool).await.unwrap();
        let out = temp_outputs();
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();

        let result = materialize_week(&pool, &index, &out, w, "Mailing List Weekly Summaries")
            .await
            .unwrap();
        assert!(matches!(
            result,
            MaterializeResult::EmptyWeekComplete { .. }
        ));

        let index_md = week_index_path(&out, w);
        assert!(index_md.is_file());
        let body = fs::read_to_string(&index_md).unwrap();
        assert!(body.contains("No mailing list activity"));
        assert!(body.contains("empty: true"));
        assert!(complete_marker_path(&out, w).is_file());
        assert!(!out.join("2026-07-20").join("messages").exists());

        let root = fs::read_to_string(root_index_path(&out)).unwrap();
        assert!(root.contains("2026-07-20"));
        assert!(root.contains("No activity"));

        let again = materialize_week(&pool, &index, &out, w, "Mailing List Weekly Summaries")
            .await
            .unwrap();
        assert!(matches!(again, MaterializeResult::AlreadyComplete { .. }));

        let _ = fs::remove_dir_all(&out);
    }

    #[test]
    fn select_active_respects_half_open_window() {
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let (start, end) = week_window(w);
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 7, 14, 0, 0, 0).unwrap());
        assert_eq!(end, Utc.with_ymd_and_hms(2026, 7, 21, 0, 0, 0).unwrap());
    }

    #[tokio::test]
    async fn resolve_and_run_start_week_empty() {
        let pool = open_in_memory().await.unwrap();
        let out = temp_outputs();
        let result = run_summarize_week(
            &pool,
            &out,
            None,
            Some("2026-07-20"),
            "https://lore.kernel.org/linux-nfs/",
            &crate::config::ListConfig::default(),
            AgentRunOpts {
                prepare_only: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            result,
            MaterializeResult::EmptyWeekComplete { .. }
        ));
        assert!(
            complete_marker_path(&out, NaiveDate::from_ymd_opt(2026, 7, 20).unwrap()).is_file()
        );
        let _ = fs::remove_dir_all(&out);
    }
}
