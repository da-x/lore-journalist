//! `ListThreadMessages` pure handler: chronological metadata for a thread.

use super::ToolCtx;
use crate::email_index::thread_root_id;
use crate::ids::{file_stem_for_id, normalize_message_id};
use crate::lore::lore_url_for_message_id;
use anyhow::{Result, bail};
use chrono::{DateTime, NaiveDate, Utc};

/// Arguments for ListThreadMessages.
#[derive(Debug, Clone, Default)]
pub struct ListThreadMessagesArgs {
    /// Normalized or raw root id. Defaults to `focus_thread_root` when set.
    pub thread_root_id: Option<String>,
    /// Inclusive start (UTC date). Optional; if both dates omitted under focus,
    /// caller may leave unset for full thread history, or we default nothing
    /// for list (full thread). Date filters are half-open day bounds when set.
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
}

/// List messages in a thread (metadata only — no bodies).
///
/// Never exposes `message_id_raw`. Includes lore URL, file_stem, in_week.
pub async fn list_thread_messages(ctx: &ToolCtx, args: ListThreadMessagesArgs) -> Result<String> {
    let root = resolve_thread_root(ctx, args.thread_root_id.as_deref())?;
    let (filter_start, filter_end) = date_bounds(args.date_from, args.date_to);

    let mut rows: Vec<_> = ctx
        .index
        .emails()
        .iter()
        .filter(|m| thread_root_id(m) == root)
        .filter(|m| in_date_range(m.date, filter_start, filter_end))
        .collect();
    rows.sort_by_key(|m| m.date);

    if rows.is_empty() {
        return Ok(format!(
            "No messages found for thread_root_id={root} (with optional date filter)."
        ));
    }

    let (week_start, week_end) = ctx.week_window;
    let mut out = String::new();
    out.push_str(&format!("Thread root_id: {root}\n"));
    out.push_str(&format!("Messages: {}\n\n", rows.len()));

    for m in rows {
        let in_week = m.date >= week_start && m.date < week_end;
        let lore = lore_url_for_message_id(&ctx.lore_base_url, &m.message_id);
        let stem = file_stem_for_id(&m.message_id);
        out.push_str(&format!(
            "- date={} from={} message_id={} file_stem={} in_week={} lore={}\n  subject={}\n",
            m.date.to_rfc3339(),
            m.from,
            m.message_id,
            stem,
            in_week,
            lore,
            m.subject,
        ));
    }
    Ok(out)
}

fn resolve_thread_root(ctx: &ToolCtx, explicit: Option<&str>) -> Result<String> {
    if let Some(r) = explicit {
        let r = r.trim();
        if !r.is_empty() {
            return Ok(normalize_message_id(r));
        }
    }
    if let Some(ref f) = ctx.focus_thread_root {
        return Ok(f.clone());
    }
    bail!("thread_root_id is required when focus_thread_root is not set");
}

/// Optional date filter: date_from 00:00 UTC inclusive, date_to+1 00:00 exclusive.
/// If only one bound set, the other is open.
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

    async fn seed_thread() -> (sqlx::SqlitePool, Arc<EmailIndex>) {
        let pool = open_in_memory().await.unwrap();
        // Root + reply with leading-space PKs.
        for (mid, subj, date, irt, refs) in [
            (" <root@t>", "Root", "2026-07-15T10:00:00+00:00", None, "[]"),
            (
                " <child@t>",
                "Re: Root",
                "2026-07-18T11:00:00+00:00",
                Some(" <root@t>"),
                r#"[" <root@t>"]"#,
            ),
            (
                " <old@t>",
                "Old other",
                "2026-01-01T00:00:00+00:00",
                None,
                "[]",
            ),
        ] {
            crate::db::insert_test_email(&pool, mid, subj, "a@b", date, "x\n", irt, refs)
                .await
                .unwrap();
        }
        let index = Arc::new(EmailIndex::load(&pool).await.unwrap());
        (pool, index)
    }

    #[tokio::test]
    async fn list_defaults_to_focus() {
        let (pool, index) = seed_thread().await;
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, PathBuf::from("/tmp/out"), w, week_window(w))
            .with_focus(Some("<root@t>".into()));

        let out = list_thread_messages(
            &ctx,
            ListThreadMessagesArgs {
                thread_root_id: None,
                date_from: None,
                date_to: None,
            },
        )
        .await
        .unwrap();

        assert!(out.contains("Messages: 2"));
        assert!(out.contains("message_id=<root@t>"));
        assert!(out.contains("message_id=<child@t>"));
        assert!(out.contains("in_week=true"));
        assert!(out.contains("lore=https://lore.kernel.org/linux-nfs/root@t/"));
        assert!(!out.contains("message_id_raw"));
        // Normalized ids only — never leading-space PK form after `message_id=`.
        assert!(!out.contains("message_id= <"));
    }

    #[tokio::test]
    async fn list_requires_root_without_focus() {
        let (pool, index) = seed_thread().await;
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, PathBuf::from("/tmp/out"), w, week_window(w));
        let err = list_thread_messages(&ctx, ListThreadMessagesArgs::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("thread_root_id is required"));
    }
}
