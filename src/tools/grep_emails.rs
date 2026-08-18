//! `GrepEmails` pure handler: regex search over subject + body without full corpus load.

use super::ToolCtx;
use crate::email_index::thread_root_id;
use crate::ids::normalize_message_id;
use crate::lore::lore_url_for_message_id;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, Utc};
use regex::Regex;

/// Default match caps (KD20).
pub const DEFAULT_MAX_MATCHES_FOCUSED: usize = 50;
pub const DEFAULT_MAX_MATCHES_CROSS: usize = 20;
/// Hard cap on how many message bodies may be decompressed per call.
pub const BODY_SCAN_CAP: usize = 200;

/// Arguments for GrepEmails.
#[derive(Debug, Clone)]
pub struct GrepEmailsArgs {
    pub pattern: String,
    pub thread_root_id: Option<String>,
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
    pub max_matches: Option<usize>,
}

/// Regex search over Subject + Body. Loads bodies on demand (never `load_all_bodies`).
pub async fn grep_emails(ctx: &ToolCtx, args: GrepEmailsArgs) -> Result<String> {
    if args.pattern.is_empty() {
        bail!("pattern is required");
    }
    let re =
        Regex::new(&args.pattern).with_context(|| format!("invalid regex: {}", args.pattern))?;

    let (root_filter, using_focus_default) =
        resolve_root_filter(ctx, args.thread_root_id.as_deref());
    let focused = root_filter.is_some();

    // Date defaults: under focus (or explicit root) when dates omitted → week window (KD20).
    let (date_start, date_end) = if args.date_from.is_none() && args.date_to.is_none() && focused {
        (Some(ctx.week_window.0), Some(ctx.week_window.1))
    } else {
        date_bounds(args.date_from, args.date_to)
    };

    let max_matches = args.max_matches.unwrap_or(if focused {
        DEFAULT_MAX_MATCHES_FOCUSED
    } else {
        DEFAULT_MAX_MATCHES_CROSS
    });

    // Candidate messages: filter by root + date using metadata only.
    let mut candidates: Vec<_> = ctx
        .index
        .emails()
        .iter()
        .filter(|m| {
            if let Some(ref r) = root_filter {
                if thread_root_id(m) != *r {
                    return false;
                }
            }
            in_date_range(m.date, date_start, date_end)
        })
        .collect();
    candidates.sort_by_key(|m| m.date);

    let mut match_lines = 0usize;
    let mut bodies_scanned = 0usize;
    let mut truncated_matches = false;
    let mut truncated_bodies = false;
    let mut out = String::new();

    out.push_str(&format!(
        "GrepEmails pattern={:?} focused={} candidates={} max_matches={} body_scan_cap={}\n",
        args.pattern,
        focused,
        candidates.len(),
        max_matches,
        BODY_SCAN_CAP
    ));
    if using_focus_default {
        out.push_str(&format!(
            "Using focus_thread_root={}\n",
            root_filter.as_deref().unwrap_or("")
        ));
    }
    out.push('\n');

    for m in candidates {
        if match_lines >= max_matches {
            truncated_matches = true;
            break;
        }
        if bodies_scanned >= BODY_SCAN_CAP {
            truncated_bodies = true;
            break;
        }

        bodies_scanned += 1;
        let body = ctx
            .index
            .load_body(&ctx.pool, &m.message_id)
            .await
            .with_context(|| format!("load_body {}", m.message_id))?;

        let lore = lore_url_for_message_id(&ctx.lore_base_url, &m.message_id);
        let root = thread_root_id(m);

        // Search subject as a virtual line, then body lines.
        let subject_line = format!("Subject: {}", m.subject);
        let mut hit_header = false;

        for (kind, line) in std::iter::once(("subject", subject_line.as_str()))
            .chain(body.lines().map(|l| ("body", l)))
        {
            if match_lines >= max_matches {
                truncated_matches = true;
                break;
            }
            if !re.is_match(line) {
                continue;
            }
            if !hit_header {
                out.push_str(&format!(
                    "=== message_id={} thread_root_id={} date={} lore={} ===\n",
                    m.message_id,
                    root,
                    m.date.format("%Y-%m-%d"),
                    lore
                ));
                hit_header = true;
            }
            out.push_str(&format!("  [{kind}] {line}\n"));
            match_lines += 1;
        }
    }

    out.push('\n');
    out.push_str(&format!(
        "Summary: match_lines={match_lines} bodies_scanned={bodies_scanned}"
    ));
    if truncated_matches {
        out.push_str(" truncated=max_matches");
    }
    if truncated_bodies {
        out.push_str(" truncated=body_scan_cap");
    }
    out.push('\n');
    Ok(out)
}

fn resolve_root_filter(ctx: &ToolCtx, explicit: Option<&str>) -> (Option<String>, bool) {
    if let Some(r) = explicit {
        let r = r.trim();
        if !r.is_empty() {
            return (Some(normalize_message_id(r)), false);
        }
    }
    if let Some(ref f) = ctx.focus_thread_root {
        return (Some(f.clone()), true);
    }
    (None, false)
}

fn date_bounds(
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let start = from.map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc());
    let end = to.map(|d| {
        d.checked_add_days(chrono::Days::new(1))
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
    });
    (start, end)
}

fn in_date_range(
    t: DateTime<Utc>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> bool {
    if let Some(s) = start {
        if t < s {
            return false;
        }
    }
    if let Some(e) = end {
        if t >= e {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::email_index::EmailIndex;
    use crate::week::week_window;
    use std::path::PathBuf;
    use std::sync::Arc;

    async fn seed() -> (sqlx::SqlitePool, Arc<EmailIndex>) {
        let pool = open_in_memory().await.unwrap();
        for (mid, subj, date, body_txt, irt, refs) in [
            (
                " <a@t>",
                "client hang fix",
                "2026-07-16T10:00:00+00:00",
                "unique_token_alpha appears here\n",
                None,
                "[]",
            ),
            (
                " <b@t>",
                "Re: client hang fix",
                "2026-07-17T10:00:00+00:00",
                "reply without the marker\n",
                Some(" <a@t>"),
                r#"[" <a@t>"]"#,
            ),
            (
                " <c@t>",
                "Other",
                "2026-07-18T10:00:00+00:00",
                "unique_token_alpha in other thread\n",
                None,
                "[]",
            ),
        ] {
            crate::db::insert_test_email(&pool, mid, subj, "a@b", date, body_txt, irt, refs)
                .await
                .unwrap();
        }
        let index = Arc::new(EmailIndex::load(&pool).await.unwrap());
        (pool, index)
    }

    #[tokio::test]
    async fn grep_focus_scopes_and_defaults_week() {
        let (pool, index) = seed().await;
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, PathBuf::from("/tmp/out"), w, week_window(w))
            .with_focus(Some("<a@t>".into()));

        let out = grep_emails(
            &ctx,
            GrepEmailsArgs {
                pattern: "unique_token_alpha".into(),
                thread_root_id: None,
                date_from: None,
                date_to: None,
                max_matches: None,
            },
        )
        .await
        .unwrap();

        assert!(out.contains("message_id=<a@t>"));
        assert!(
            !out.contains("message_id=<c@t>"),
            "cross-thread should not match under focus"
        );
        assert!(out.contains("focused=true"));
    }

    #[tokio::test]
    async fn grep_cross_thread_finds_both() {
        let (pool, index) = seed().await;
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, PathBuf::from("/tmp/out"), w, week_window(w));

        let out = grep_emails(
            &ctx,
            GrepEmailsArgs {
                pattern: "unique_token_alpha".into(),
                thread_root_id: None,
                date_from: Some(NaiveDate::from_ymd_opt(2026, 7, 14).unwrap()),
                date_to: Some(NaiveDate::from_ymd_opt(2026, 7, 20).unwrap()),
                max_matches: Some(50),
            },
        )
        .await
        .unwrap();

        assert!(out.contains("message_id=<a@t>"));
        assert!(out.contains("message_id=<c@t>"));
        assert!(out.contains("lore=https://lore.kernel.org/"));
    }

    #[tokio::test]
    async fn grep_subject_match() {
        let (pool, index) = seed().await;
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, PathBuf::from("/tmp/out"), w, week_window(w));

        let out = grep_emails(
            &ctx,
            GrepEmailsArgs {
                pattern: "client hang".into(),
                thread_root_id: Some("<a@t>".into()),
                date_from: None,
                date_to: None,
                max_matches: None,
            },
        )
        .await
        .unwrap();

        assert!(out.contains("[subject]"));
        assert!(out.contains("client hang fix"));
    }

    #[tokio::test]
    async fn grep_invalid_regex_errors() {
        let (pool, index) = seed().await;
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, PathBuf::from("/tmp/out"), w, week_window(w));
        let err = grep_emails(
            &ctx,
            GrepEmailsArgs {
                pattern: "(".into(),
                thread_root_id: None,
                date_from: None,
                date_to: None,
                max_matches: None,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid regex"));
    }
}
