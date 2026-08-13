//! `GetEmail` pure handler: rebuild one message from DB for the LLM.

use super::ToolCtx;
use crate::ids::normalize_message_id;
use crate::lore::lore_url_for_message_id;
use anyhow::{Context, Result, bail};

/// Arguments for GetEmail (matches future schemars tool args).
#[derive(Debug, Clone)]
pub struct GetEmailArgs {
    pub message_id: String,
}

/// Load a single email by id (raw or normalized). Returns text for the model.
///
/// Headers use **normalized** Message-IDs only (never `message_id_raw`).
pub async fn get_email(ctx: &ToolCtx, args: GetEmailArgs) -> Result<String> {
    let id = args.message_id.trim();
    if id.is_empty() {
        bail!("message_id is required");
    }

    let meta = ctx.index.get(id).with_context(|| {
        format!(
            "unknown message_id after normalize: {:?}",
            normalize_message_id(id)
        )
    })?;

    let body = ctx
        .index
        .load_body(&ctx.pool, &meta.message_id)
        .await
        .with_context(|| format!("load_body failed for {}", meta.message_id))?;

    let lore = lore_url_for_message_id(&ctx.lore_base_url, &meta.message_id);
    let in_reply = meta
        .in_reply_to
        .as_deref()
        .map(normalize_message_id)
        .unwrap_or_default();

    let mut out = String::new();
    out.push_str(&format!("Message-ID: {}\n", meta.message_id));
    out.push_str(&format!("Lore: {lore}\n"));
    out.push_str(&format!("From: {}\n", meta.from));
    out.push_str(&format!("Date: {}\n", meta.date.to_rfc3339()));
    out.push_str(&format!("Subject: {}\n", meta.subject));
    if !in_reply.is_empty() {
        out.push_str(&format!("In-Reply-To: {in_reply}\n"));
    }
    out.push_str(&format!(
        "Thread-Root-ID: {}\n",
        crate::email_index::thread_root_id(meta)
    ));
    out.push_str("\n");
    out.push_str(&body);
    if !body.is_empty() && !body.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::email_index::EmailIndex;
    use crate::week::week_window;
    use chrono::NaiveDate;
    use std::path::PathBuf;
    use std::sync::Arc;

    async fn seed_one() -> (sqlx::SqlitePool, Arc<EmailIndex>) {
        let pool = open_in_memory().await.unwrap();
        crate::db::insert_test_email(
            &pool,
            " <get@test.com>",
            "Hello",
            "a@b",
            "2026-07-18T12:00:00+00:00",
            "line one\nline two\n",
            None,
            "[]",
        )
        .await
        .unwrap();
        let index = Arc::new(EmailIndex::load(&pool).await.unwrap());
        (pool, index)
    }

    fn ctx(pool: sqlx::SqlitePool, index: Arc<EmailIndex>) -> ToolCtx {
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        ToolCtx::new(pool, index, PathBuf::from("/tmp/out"), w, week_window(w))
    }

    #[tokio::test]
    async fn get_email_accepts_raw_or_normalized() {
        let (pool, index) = seed_one().await;
        let ctx = ctx(pool, index);

        let a = get_email(
            &ctx,
            GetEmailArgs {
                message_id: "<get@test.com>".into(),
            },
        )
        .await
        .unwrap();
        let b = get_email(
            &ctx,
            GetEmailArgs {
                message_id: " <get@test.com>".into(),
            },
        )
        .await
        .unwrap();

        assert!(a.contains("Message-ID: <get@test.com>"));
        assert!(!a.contains("Message-ID:  <"));
        assert!(a.contains("Lore: https://lore.kernel.org/linux-nfs/get@test.com/"));
        assert!(a.contains("line one"));
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn get_email_missing_errors() {
        let (pool, index) = seed_one().await;
        let ctx = ctx(pool, index);
        let err = get_email(
            &ctx,
            GetEmailArgs {
                message_id: "<nope@x>".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("unknown message_id"));
    }
}
