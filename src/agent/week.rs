//! Week overview agent + finalize week index / root catalog / `.complete`.

use super::session::{run_until_submit, WEEK_AGENT_TIMEOUT};
use super::tool_build::build_week_tools;
use crate::ids::file_stem_for_id;
use crate::outputs::{
    complete_marker_path, root_index_path, thread_markdown_path, week_index_path, write_atomic,
    write_complete_marker, write_root_index, yaml_double_quoted, RootIndexEntry,
};
use crate::summarize::ActiveThread;
use crate::tools::submit::SubmitSlot;
use crate::tools::ToolCtx;
use crate::week::{scan_week_dirs, week_window};
use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use da_harness::multi_tool::InferenceCallback;
use da_harness::OpenAIClient;
use std::fs;
use std::path::Path;
use tracing::info;

const WEEK_SYSTEM: &str = r#"You are a technical editor covering the Linux NFS mailing list.
Write a front-page overview of this week's activity: critical bugs, NFS client focus, major trends, and ongoing debates.
Read thread/*.md files via ReadOutputFile as needed. Link discussions with relative paths like thread/<stem>.md.
Call SubmitWeekOverview exactly once with a non-empty headline (one line) and markdown_body.
"#;

/// True when every expected root has a `thread/<stem>.md` file.
pub fn all_thread_files_present(
    outputs_path: &Path,
    week: NaiveDate,
    expected_roots: &[String],
) -> bool {
    expected_roots
        .iter()
        .all(|r| thread_markdown_path(outputs_path, week, r).is_file())
}

pub fn build_week_user_message(
    week: NaiveDate,
    ordered_roots: &[String],
    by_subject: &[(String, String)],
) -> String {
    let (start, end) = week_window(week);
    let week_s = week.format("%Y-%m-%d").to_string();
    let mut s = format!(
        "Week ending: {week_s}\nWindow (UTC half-open): [{}, {})\nThreads this week ({}):\n\n",
        start.to_rfc3339(),
        end.to_rfc3339(),
        ordered_roots.len(),
    );
    for (i, root) in ordered_roots.iter().enumerate() {
        let stem = file_stem_for_id(root);
        let subj = by_subject
            .iter()
            .find(|(r, _)| r == root)
            .map(|(_, s)| s.as_str())
            .unwrap_or("(unknown subject)");
        s.push_str(&format!(
            "{}. subject={subj}\n   root_id={root}\n   path={week_s}/thread/{stem}.md\n",
            i + 1,
        ));
    }
    s.push_str(
        "\nRead thread summaries as needed with ReadOutputFile using the paths above.\n\
         SubmitWeekOverview with headline (for the site index) and markdown_body overview.\n\
         Prefer relative links: thread/<stem>.md\n",
    );
    s
}

/// Host-built TOC appended after agent overview body.
pub fn format_host_thread_toc(
    ordered_roots: &[String],
    by_subject: &[(String, String)],
) -> String {
    let mut s = String::from("\n## Discussions this week\n\n");
    for root in ordered_roots {
        let stem = file_stem_for_id(root);
        let subj = by_subject
            .iter()
            .find(|(r, _)| r == root)
            .map(|(_, s)| s.as_str())
            .unwrap_or(root.as_str());
        s.push_str(&format!("- [{subj}](thread/{stem}.md)\n"));
    }
    s
}

pub fn write_week_index(
    outputs_path: &Path,
    week: NaiveDate,
    headline: &str,
    agent_body: &str,
    toc: &str,
) -> Result<()> {
    let week_s = week.format("%Y-%m-%d").to_string();
    let mut md = String::new();
    md.push_str("---\n");
    md.push_str(&format!("week_ending: {}\n", yaml_double_quoted(&week_s)));
    md.push_str(&format!("headline: {}\n", yaml_double_quoted(headline)));
    md.push_str("empty: false\n");
    md.push_str("---\n\n");
    md.push_str(&format!("# {headline}\n\n"));
    md.push_str(&format!("*Week ending {week_s}*\n\n"));
    md.push_str(agent_body.trim());
    md.push('\n');
    md.push_str(toc);
    write_atomic(&week_index_path(outputs_path, week), &md)?;
    Ok(())
}

/// Rebuild root index from all complete weeks plus optional extra (this week about to complete).
pub fn regenerate_root_index(
    outputs_path: &Path,
    include_week: Option<(NaiveDate, &str)>,
) -> Result<()> {
    let (mut complete, _) = scan_week_dirs(outputs_path)?;
    if let Some((w, _)) = include_week {
        if !complete.contains(&w) {
            complete.push(w);
        }
    }
    complete.sort_unstable();
    complete.reverse();

    let mut entries = Vec::new();
    for w in complete {
        let headline = if let Some((iw, h)) = include_week {
            if iw == w {
                h.to_string()
            } else {
                read_headline(outputs_path, w).unwrap_or_else(|| "…".into())
            }
        } else {
            read_headline(outputs_path, w).unwrap_or_else(|| "…".into())
        };
        entries.push(RootIndexEntry { week: w, headline });
    }
    write_root_index(outputs_path, &entries)?;
    Ok(())
}

fn read_headline(outputs_path: &Path, w: NaiveDate) -> Option<String> {
    let text = fs::read_to_string(week_index_path(outputs_path, w)).ok()?;
    for line in text.lines().take(20) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("headline:") {
            let v = rest.trim();
            if let Some(inner) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                return Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"));
            }
            return Some(v.to_string());
        }
    }
    None
}

/// Write week index + root catalog + `.complete` last.
pub fn finalize_week(
    outputs_path: &Path,
    week: NaiveDate,
    headline: &str,
    agent_body: &str,
    ordered_roots: &[String],
    by_subject: &[(String, String)],
) -> Result<()> {
    let toc = format_host_thread_toc(ordered_roots, by_subject);
    write_week_index(outputs_path, week, headline, agent_body, &toc)?;
    if let Ok(f) = fs::File::open(week_index_path(outputs_path, week)) {
        let _ = f.sync_all();
    }
    regenerate_root_index(outputs_path, Some((week, headline)))?;
    if let Ok(f) = fs::File::open(root_index_path(outputs_path)) {
        let _ = f.sync_all();
    }
    write_complete_marker(outputs_path, week)?;
    info!(%week, headline, "week finalized (.complete written)");
    Ok(())
}

/// Run week overview agent and finalize (call only when all thread files exist).
pub async fn run_week_overview_and_finalize(
    ctx: ToolCtx,
    week: NaiveDate,
    ordered_roots: &[String],
    active: &[ActiveThread],
    client: Option<OpenAIClient>,
    inference: Option<InferenceCallback>,
) -> Result<()> {
    if !all_thread_files_present(&ctx.outputs_path, week, ordered_roots) {
        bail!("cannot run week overview: missing thread/*.md files");
    }
    if complete_marker_path(&ctx.outputs_path, week).is_file() {
        info!(%week, "week already complete; skip overview");
        return Ok(());
    }

    let by_subject: Vec<(String, String)> = active
        .iter()
        .map(|t| (t.root_id.clone(), t.subject.clone()))
        .collect();

    let slot = SubmitSlot::new();
    let tools = build_week_tools(ctx.clone(), slot.clone())?;
    let user = build_week_user_message(week, ordered_roots, &by_subject);

    let payload = run_until_submit(
        WEEK_SYSTEM,
        user,
        tools,
        slot,
        WEEK_AGENT_TIMEOUT,
        client,
        inference,
    )
    .await
    .context("week overview agent")?;

    finalize_week(
        &ctx.outputs_path,
        week,
        &payload.headline,
        &payload.markdown_body,
        ordered_roots,
        &by_subject,
    )?;
    Ok(())
}
