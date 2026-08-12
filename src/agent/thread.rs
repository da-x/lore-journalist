//! Per-thread summarization agent.

use super::session::{run_until_submit, THREAD_AGENT_TIMEOUT};
use super::tool_build::build_thread_tools;
use crate::email_index::EmailIndex;
use crate::ids::file_stem_for_id;
use crate::outputs::{
    format_message_list_lore, prior_thread_glob_pattern, thread_markdown_path, write_atomic,
    yaml_double_quoted,
};
use crate::summarize::ActiveThread;
use crate::tools::submit::{SubmitSlot, ThreadSummaryPayload};
use crate::tools::ToolCtx;
use crate::week::week_window;
use anyhow::{Context, Result};
use chrono::NaiveDate;
use da_harness::multi_tool::InferenceCallback;
use da_harness::OpenAIClient;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

const THREAD_SYSTEM: &str = r#"You are a technical journalist covering the Linux NFS mailing list.
Summarize this week's developments in the focused thread. Use tools to read messages and prior summaries.
Cite messages with lore.kernel.org URLs (use Lore: lines from GetEmail / ListThreadMessages).
Cite other thread summaries with relative paths like 2026-07-20/thread/<stem>.md when relevant.
Bridge prior weeks briefly; focus on new content this week.
Call SubmitThreadSummary exactly once when done with a non-empty markdown_body.
"#;

/// Prior same-stem thread summaries across weeks (paths relative to outputs root), newest first.
pub fn find_prior_thread_summaries(
    outputs_path: &Path,
    week: NaiveDate,
    thread_root_id: &str,
    n: usize,
) -> Vec<String> {
    let stem = file_stem_for_id(thread_root_id);
    let mut found: Vec<(NaiveDate, String)> = Vec::new();

    let Ok(entries) = fs::read_dir(outputs_path) else {
        return Vec::new();
    };
    for ent in entries.flatten() {
        let name = ent.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Ok(d) = NaiveDate::parse_from_str(name, "%Y-%m-%d") else {
            continue;
        };
        if d >= week {
            continue;
        }
        let path = ent.path().join("thread").join(format!("{stem}.md"));
        if path.is_file() {
            found.push((d, format!("{name}/thread/{stem}.md")));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().take(n).map(|(_, p)| p).collect()
}

/// Same-week predecessor thread files already on disk (outputs-relative paths).
pub fn same_week_predecessors(
    outputs_path: &Path,
    week: NaiveDate,
    ordered_roots: &[String],
    current_root: &str,
) -> Vec<String> {
    let week_s = week.format("%Y-%m-%d").to_string();
    let mut out = Vec::new();
    for root in ordered_roots {
        if root == current_root {
            break;
        }
        let path = thread_markdown_path(outputs_path, week, root);
        if path.is_file() {
            let stem = file_stem_for_id(root);
            out.push(format!("{week_s}/thread/{stem}.md"));
        }
    }
    out.truncate(10);
    out
}

pub fn build_thread_user_message(
    week: NaiveDate,
    thread: &ActiveThread,
    index: &EmailIndex,
    lore_base: &str,
    position: usize,
    total: usize,
    prior: &[String],
    same_week: &[String],
) -> String {
    let (start, end) = week_window(week);
    let mut s = format!(
        "Week ending: {}\nWindow (UTC half-open): [{}, {})\nThread root_id (normalized): {}\nSubject: {}\nPosition in week order: {position} of {total}\n",
        week.format("%Y-%m-%d"),
        start.to_rfc3339(),
        end.to_rfc3339(),
        thread.root_id,
        thread.subject,
    );
    s.push_str(&format!(
        "Messages this week ({}):\n",
        thread.message_indices.len()
    ));
    for &idx in &thread.message_indices {
        let m = &index.emails()[idx];
        s.push_str(&format!(
            "  - {} | {} | {} | {}\n",
            m.date.format("%Y-%m-%d"),
            m.from,
            m.message_id,
            m.subject
        ));
    }
    if !prior.is_empty() {
        s.push_str("Cross-week prior summaries (ReadOutputFile these):\n");
        for p in prior {
            s.push_str(&format!("  - {p}\n"));
        }
    }
    if !same_week.is_empty() {
        s.push_str("Same-week predecessors already summarized (ReadOutputFile these):\n");
        for p in same_week {
            s.push_str(&format!("  - {p}\n"));
        }
    }
    s.push_str(&format!(
        "Optional deeper history: GlobOutputs \"{}\"\n",
        prior_thread_glob_pattern(&thread.root_id)
    ));
    s.push_str(&format!(
        "Lore base for citations: {lore_base}\nWrite the weekly summary, then SubmitThreadSummary.\n"
    ));
    s
}

/// Format and write `thread/<stem>.md` from agent payload + host message list.
pub fn write_thread_summary_file(
    outputs_path: &Path,
    week: NaiveDate,
    thread: &ActiveThread,
    index: &EmailIndex,
    lore_base: &str,
    payload: &ThreadSummaryPayload,
    prior: &[String],
) -> Result<PathBuf> {
    let path = thread_markdown_path(outputs_path, week, &thread.root_id);
    let mut items = Vec::new();
    for &idx in &thread.message_indices {
        let m = &index.emails()[idx];
        items.push((
            m.date.format("%Y-%m-%d").to_string(),
            m.from.clone(),
            m.subject.clone(),
            m.message_id.clone(),
        ));
    }
    let list = format_message_list_lore(lore_base, &items);

    let mut fm = String::new();
    fm.push_str("---\n");
    fm.push_str(&format!(
        "thread_root_id: {}\n",
        yaml_double_quoted(&thread.root_id)
    ));
    fm.push_str(&format!(
        "week_ending: {}\n",
        yaml_double_quoted(&week.format("%Y-%m-%d").to_string())
    ));
    fm.push_str(&format!("subject: {}\n", yaml_double_quoted(&thread.subject)));
    fm.push_str(&format!("title: {}\n", yaml_double_quoted(&payload.title)));
    fm.push_str("message_ids_this_week:\n");
    for &idx in &thread.message_indices {
        let m = &index.emails()[idx];
        fm.push_str(&format!("  - {}\n", yaml_double_quoted(&m.message_id)));
    }
    if !prior.is_empty() {
        fm.push_str("prior_summaries:\n");
        for p in prior {
            fm.push_str(&format!("  - {}\n", yaml_double_quoted(p)));
        }
    }
    fm.push_str("---\n\n");
    fm.push_str(&format!("# {}\n\n", payload.title));
    fm.push_str("## Summary\n\n");
    fm.push_str(payload.markdown_body.trim());
    fm.push_str("\n\n");
    fm.push_str(&list);

    write_atomic(&path, &fm)?;
    Ok(path)
}

pub async fn run_thread_agent(
    ctx: ToolCtx,
    week: NaiveDate,
    thread: &ActiveThread,
    index: &EmailIndex,
    ordered_roots: &[String],
    position: usize,
    total: usize,
    client: Option<OpenAIClient>,
    inference: Option<InferenceCallback>,
) -> Result<PathBuf> {
    let outputs_path = ctx.outputs_path.clone();
    let path = thread_markdown_path(&outputs_path, week, &thread.root_id);
    if path.is_file() {
        info!(root = %thread.root_id, "skip existing thread summary");
        return Ok(path);
    }

    let prior = find_prior_thread_summaries(&outputs_path, week, &thread.root_id, 3);
    let same = same_week_predecessors(&outputs_path, week, ordered_roots, &thread.root_id);
    let lore = ctx.lore_base_url.clone();
    let user = build_thread_user_message(
        week,
        thread,
        index,
        &lore,
        position,
        total,
        &prior,
        &same,
    );

    let ctx = ctx.with_focus(Some(thread.root_id.clone()));
    let slot = SubmitSlot::new();
    let tools = build_thread_tools(ctx, slot.clone())?;

    let payload = run_until_submit(
        THREAD_SYSTEM,
        user,
        tools,
        slot,
        THREAD_AGENT_TIMEOUT,
        client,
        inference,
    )
    .await
    .with_context(|| format!("thread agent for {}", thread.root_id))?;

    let written = write_thread_summary_file(
        &outputs_path,
        week,
        thread,
        index,
        &lore,
        &payload,
        &prior,
    )?;
    info!(root = %thread.root_id, path = %written.display(), "wrote thread summary");
    Ok(written)
}
