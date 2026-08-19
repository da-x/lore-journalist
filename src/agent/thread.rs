//! Per-thread summarization agent.

use super::session::{THREAD_AGENT_TIMEOUT, UsageStage, UsageTotals, run_until_submit};
use super::tool_build::build_thread_tools;
use crate::email_index::EmailIndex;
use crate::ids::file_stem_for_id;
use crate::lore::lore_url_for_message_id;
use crate::outputs::{
    format_message_list_lore, prior_thread_glob_pattern, thread_markdown_path, write_atomic,
    yaml_double_quoted,
};
use crate::summarize::ActiveThread;
use crate::tools::ToolCtx;
use crate::tools::submit::{SubmitSlot, ThreadSummaryPayload};
use crate::week::week_window;
use anyhow::{Context, Result};
use chrono::NaiveDate;
use da_harness::OpenAIClient;
use da_harness::multi_tool::InferenceCallback;
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use tracing::info;

/// Rewrite `[text](id://message-id)` citations to lore permalinks for published markdown.
static ID_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\]\(id://([^)]+)\)").expect("id link regex"));

pub fn rewrite_id_links_to_lore(markdown: &str, lore_base: &str) -> String {
    ID_LINK_RE
        .replace_all(markdown, |caps: &regex::Captures| {
            let mid = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let url = lore_url_for_message_id(lore_base, mid);
            format!("]({url})")
        })
        .into_owned()
}

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
    focus: &str,
) -> String {
    let (start, end) = week_window(week);
    let mut s = String::new();

    // Journalistic brief (from legacy one-shot summarizer), adapted for tools.
    s.push_str(
        "Provide a detailed summary of the following mailing list thread.\n\
         Highlight the key technical arguments, the evolution of the discussion, and the final conclusions.\n",
    );
    let focus = focus.trim();
    if !focus.is_empty() {
        s.push_str(focus);
        s.push('\n');
    }
    s.push_str(
        "\n\
         Scope this summary to activity in the current week when the thread spans multiple weeks;\n\
         bridge prior weeks only briefly (use prior summary files if listed below).\n\
         \n\
         IMPORTANT:\n\
         - Quote specific conclusions and significant intermediate remarks from the participants\n\
           to provide context and flavor. Use double quotes and Markdown blockquote syntax\n\
           (e.g., > \"Quote content\") for these quotes.\n\
         - Identify if the discussion is about a new feature, a protocol change, or a bug fix.\n\
         - When referring to a specific message, use the following markup: [text](id://message-id).\n\
           Example: [As mentioned by Alice](id://example-msg-id).\n\
         - The message-id should be taken exactly from the Message-ID header provided by tools\n\
           or listed below (normalized form, e.g. <...@...>).\n\
         - This week's in-window message bodies are included below when they fit. Use GetEmail /\n\
           ListThreadMessages for anything omitted or for earlier history; do not invent content.\n\
         - Do not repeat a read tool with the same arguments; duplicate calls return an error.\n\
           Use the earlier result.\n\
         - When finished, call SubmitThreadSummary exactly once with a non-empty markdown_body\n\
           (and a short title). Writing the summary as assistant text does not complete the task.\n\
         \n",
    );

    s.push_str(&format!(
        "Week ending: {}\n\
         Window (UTC half-open): [{}, {})\n\
         Thread root_id (normalized): {}\n\
         Subject: {}\n\
         Position in week order: {position} of {total}\n\
         Lore base (for your reference; still cite with id:// in the summary): {lore_base}\n",
        week.format("%Y-%m-%d"),
        start.to_rfc3339(),
        end.to_rfc3339(),
        thread.root_id,
        thread.subject,
    ));

    s.push_str(&format!(
        "\nMessages this week ({}):\n",
        thread.message_indices.len()
    ));
    for &idx in &thread.message_indices {
        let m = &index.emails()[idx];
        s.push_str(&format!(
            "  - date={} | from={} | Message-ID: {} | subject={}\n",
            m.date.format("%Y-%m-%d"),
            m.from,
            m.message_id,
            m.subject
        ));
    }

    if !prior.is_empty() {
        s.push_str("\nCross-week prior summaries (ReadOutputFile these if useful):\n");
        for p in prior {
            s.push_str(&format!("  - {p}\n"));
        }
    }
    if !same_week.is_empty() {
        s.push_str(
            "\nSame-week predecessors already summarized (ReadOutputFile these if useful):\n",
        );
        for p in same_week {
            s.push_str(&format!("  - {p}\n"));
        }
    }
    s.push_str(&format!(
        "\nOptional deeper history: GlobOutputs \"{}\"\n",
        prior_thread_glob_pattern(&thread.root_id)
    ));
    s.push_str("\nSummarize this thread using the tools as needed, then SubmitThreadSummary.\n");
    s
}

/// Budget for cleaned in-week bodies injected into the thread-agent prompt.
const IN_WEEK_BODY_BUDGET: usize = 24 * 1024;

/// Format this week's cleaned bodies for the thread prompt (size-capped).
pub(crate) async fn format_in_week_bodies(
    pool: &sqlx::SqlitePool,
    index: &EmailIndex,
    thread: &ActiveThread,
    max_bytes: usize,
) -> Result<String> {
    let mut out = String::new();
    let mut used = 0usize;
    let mut omitted = 0usize;

    for &idx in &thread.message_indices {
        let m = &index.emails()[idx];
        let body = index
            .load_body(pool, &m.message_id)
            .await
            .with_context(|| format!("load_body for prompt {}", m.message_id))?;
        let chunk = format!(
            "----- Message-ID: {}\nFrom: {}\nDate: {}\nSubject: {}\n\n{}\n",
            m.message_id,
            m.from,
            m.date.to_rfc3339(),
            m.subject,
            body
        );
        if used > 0 && used + chunk.len() > max_bytes {
            omitted += 1;
            continue;
        }
        if out.is_empty() {
            out.push_str(
                "\nThis week's message bodies (cleaned; patch diffs omitted at ingest):\n",
            );
        }
        if used == 0 && chunk.len() > max_bytes {
            let keep = max_bytes.saturating_sub(80);
            out.push_str(&chunk[..chunk.len().min(keep)]);
            out.push_str("\n[... truncated; use GetEmail for the rest ...]\n");
            omitted += thread.message_indices.len().saturating_sub(1);
            break;
        }
        out.push_str(&chunk);
        used += chunk.len();
    }
    if omitted > 0 {
        out.push_str(&format!(
            "({omitted} further in-week bodies omitted for size; use GetEmail)\n"
        ));
    }
    Ok(out)
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
    let body = rewrite_id_links_to_lore(payload.markdown_body.trim(), lore_base);
    let title = rewrite_id_links_to_lore(&payload.title, lore_base);

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
    fm.push_str(&format!(
        "subject: {}\n",
        yaml_double_quoted(&thread.subject)
    ));
    fm.push_str(&format!("title: {}\n", yaml_double_quoted(&title)));
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
    fm.push_str(&format!("# {title}\n\n"));
    fm.push_str("## Summary\n\n");
    fm.push_str(&body);
    fm.push_str("\n\n");
    fm.push_str(&list);

    write_atomic(&path, &fm)?;
    Ok(path)
}

#[allow(clippy::too_many_arguments)]
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
    usage: UsageTotals,
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
    let mut user = build_thread_user_message(
        week,
        thread,
        index,
        &lore,
        position,
        total,
        &prior,
        &same,
        &ctx.list.focus,
    );
    let bodies = format_in_week_bodies(&ctx.pool, index, thread, IN_WEEK_BODY_BUDGET)
        .await
        .context("preload in-week bodies for thread prompt")?;
    user.push_str(&bodies);

    let system = ctx.list.thread_system_prompt();
    let ctx = ctx.with_focus(Some(thread.root_id.clone()));
    let slot = SubmitSlot::new();
    let tools = build_thread_tools(ctx, slot.clone())?;

    let payload = run_until_submit(
        &system,
        user,
        tools,
        slot,
        THREAD_AGENT_TIMEOUT,
        client,
        inference,
        usage,
        UsageStage::Thread,
    )
    .await
    .with_context(|| format!("thread agent for {}", thread.root_id))?;

    let written =
        write_thread_summary_file(&outputs_path, week, thread, index, &lore, &payload, &prior)?;
    info!(root = %thread.root_id, path = %written.display(), "wrote thread summary");
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{insert_test_email, open_in_memory};
    use crate::summarize::select_active_threads;

    #[test]
    fn rewrites_id_links_to_lore() {
        let md = r#"[As mentioned by Chuck Lever](id://<abc@def.com>) and [x](id://foo@bar)"#;
        let out = rewrite_id_links_to_lore(md, "https://lore.kernel.org/list/");
        assert!(out.contains("](https://lore.kernel.org/list/abc@def.com/)"));
        assert!(out.contains("](https://lore.kernel.org/list/foo@bar/)"));
        assert!(!out.contains("id://"));
    }

    #[tokio::test]
    async fn format_in_week_bodies_includes_cleaned_text() {
        let pool = open_in_memory().await.unwrap();
        insert_test_email(
            &pool,
            " <solo@t>",
            "Solo thread",
            "alice@ex.com",
            "2026-07-18T12:00:00+00:00",
            "important body line\n",
            None,
            "[]",
        )
        .await
        .unwrap();
        let index = EmailIndex::load(&pool).await.unwrap();
        let week = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let active = select_active_threads(&index, week);
        let text = format_in_week_bodies(&pool, &index, &active[0], 24 * 1024)
            .await
            .unwrap();
        assert!(text.contains("important body line"));
        assert!(text.contains("Message-ID: <solo@t>"));
    }

    #[tokio::test]
    async fn format_in_week_bodies_omits_when_over_budget() {
        let pool = open_in_memory().await.unwrap();
        insert_test_email(
            &pool,
            " <a@t>",
            "First",
            "a@b",
            "2026-07-16T00:00:00+00:00",
            "aaaa\n",
            None,
            "[]",
        )
        .await
        .unwrap();
        insert_test_email(
            &pool,
            " <b@t>",
            "Re: First",
            "a@b",
            "2026-07-17T00:00:00+00:00",
            "bbbb\n",
            Some(" <a@t>"),
            r#"[" <a@t>"]"#,
        )
        .await
        .unwrap();
        let index = EmailIndex::load(&pool).await.unwrap();
        let week = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let active = select_active_threads(&index, week);
        // Budget 1 forces the first-message truncate path.
        let text = format_in_week_bodies(&pool, &index, &active[0], 1)
            .await
            .unwrap();
        assert!(
            text.contains("truncated") || text.contains("omitted"),
            "expected size cap notice: {text}"
        );
    }
}
