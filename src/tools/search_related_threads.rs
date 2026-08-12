//! `SearchRelatedThreads` pure handler: subject-normalized related roots.

use super::ToolCtx;
use crate::email_index::thread_root_id;
use crate::ids::normalize_message_id;
use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};

const DEFAULT_LIMIT: usize = 10;

#[derive(Debug, Clone)]
pub struct SearchRelatedThreadsArgs {
    pub subject: String,
    pub limit: Option<usize>,
}

struct RootAgg {
    title: String,
    title_date: DateTime<Utc>,
    last_activity: DateTime<Utc>,
    tokens: HashSet<String>,
}

/// Find threads with overlapping subject tokens.
///
/// If `ctx.allowed_thread_roots` is set (ordering agent), only those roots are considered.
pub async fn search_related_threads(
    ctx: &ToolCtx,
    args: SearchRelatedThreadsArgs,
) -> Result<String> {
    let subject = args.subject.trim();
    if subject.is_empty() {
        bail!("subject is required");
    }
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).max(1);
    let query_tokens = subject_tokens(subject);
    if query_tokens.is_empty() {
        return Ok(
            "SearchRelatedThreads: no usable tokens after normalizing subject.\n".into(),
        );
    }

    let mut by_root: HashMap<String, RootAgg> = HashMap::new();

    for m in ctx.index.emails() {
        let root = thread_root_id(m);
        if let Some(ref allow) = ctx.allowed_thread_roots {
            if !allow.contains(&root) {
                continue;
            }
        }
        let entry = by_root.entry(root).or_insert_with(|| RootAgg {
            title: m.subject.clone(),
            title_date: m.date,
            last_activity: m.date,
            tokens: HashSet::new(),
        });
        if m.date < entry.title_date {
            entry.title_date = m.date;
            entry.title = m.subject.clone();
        }
        if m.date > entry.last_activity {
            entry.last_activity = m.date;
        }
        for t in subject_tokens(&m.subject) {
            entry.tokens.insert(t);
        }
    }

    let mut scored: Vec<(usize, String, String, DateTime<Utc>)> = Vec::new();
    for (root, agg) in &by_root {
        let score = query_tokens.iter().filter(|t| agg.tokens.contains(*t)).count();
        if score == 0 {
            continue;
        }
        scored.push((score, root.clone(), agg.title.clone(), agg.last_activity));
    }

    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.3.cmp(&a.3))
            .then_with(|| a.1.cmp(&b.1))
    });
    scored.truncate(limit);

    let mut out = format!(
        "SearchRelatedThreads query_subject={:?} tokens={:?} hits={}\n\n",
        subject, query_tokens, scored.len()
    );
    if scored.is_empty() {
        out.push_str("(no related threads)\n");
        return Ok(out);
    }
    for (score, root, title, last) in scored {
        out.push_str(&format!(
            "- score={score} root_id={root} last_activity={} subject={title}\n",
            last.to_rfc3339()
        ));
    }
    Ok(out)
}

/// Strip Re:/Fwd:/[PATCH*] noise and lowercase.
pub fn normalize_subject(subject: &str) -> String {
    let mut s = subject.trim().to_lowercase();
    loop {
        let before = s.clone();
        s = s.trim().to_string();
        for prefix in ["re:", "fwd:", "fw:", "aw:", "sv:"] {
            if let Some(rest) = s.strip_prefix(prefix) {
                s = rest.trim_start().to_string();
            }
        }
        if s.starts_with('[') {
            if let Some(end) = s.find(']') {
                s = s[end + 1..].trim_start().to_string();
                continue;
            }
        }
        if s == before {
            break;
        }
    }
    s
}

pub fn subject_tokens(subject: &str) -> HashSet<String> {
    let norm = normalize_subject(subject);
    let mut set = HashSet::new();
    for raw in norm.split(|c: char| !c.is_ascii_alphanumeric()) {
        let t = raw.to_ascii_lowercase();
        if t.len() < 3 {
            continue;
        }
        if matches!(
            t.as_str(),
            "the" | "and" | "for" | "with" | "from" | "this" | "that" | "nfs" | "patch" | "linux"
        ) {
            continue;
        }
        set.insert(t);
    }
    set
}

/// Normalize a list of roots for allow-list.
#[allow(dead_code)] // used by ordering agent host + tests
pub fn normalize_root_set(roots: impl IntoIterator<Item = impl AsRef<str>>) -> HashSet<String> {
    roots
        .into_iter()
        .map(|r| normalize_message_id(r.as_ref()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{insert_test_email, open_in_memory};
    use crate::email_index::EmailIndex;
    use crate::week::week_window;
    use chrono::NaiveDate;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn normalize_strips_re_and_patch() {
        assert_eq!(
            normalize_subject("Re: [PATCH v2] nfs: fix client hang"),
            "nfs: fix client hang"
        );
        assert_eq!(normalize_subject("FWD: Hello"), "hello");
    }

    #[tokio::test]
    async fn related_scores_overlap() {
        let pool = open_in_memory().await.unwrap();
        insert_test_email(
            &pool,
            " <t1@x>",
            "[PATCH] client hang on mount",
            "a@b",
            "2026-07-15T00:00:00+00:00",
            "b\n",
            None,
            "[]",
        )
        .await
        .unwrap();
        insert_test_email(
            &pool,
            " <t2@x>",
            "Re: client hang on mount more",
            "a@b",
            "2026-07-16T00:00:00+00:00",
            "b\n",
            None,
            "[]",
        )
        .await
        .unwrap();
        insert_test_email(
            &pool,
            " <t3@x>",
            "unrelated bakeathon reminder",
            "a@b",
            "2026-07-17T00:00:00+00:00",
            "b\n",
            None,
            "[]",
        )
        .await
        .unwrap();

        let index = Arc::new(EmailIndex::load(&pool).await.unwrap());
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, PathBuf::from("/tmp/out"), w, week_window(w));

        let out = search_related_threads(
            &ctx,
            SearchRelatedThreadsArgs {
                subject: "Re: client hang".into(),
                limit: Some(5),
            },
        )
        .await
        .unwrap();

        assert!(out.contains("root_id=<t1@x>") || out.contains("root_id=<t2@x>"));
        // Unrelated bakeathon should not outrank hang threads
        if let (Some(p_hang), Some(p_unrel)) = (out.find("t1@x").or(out.find("t2@x")), out.find("t3@x"))
        {
            assert!(p_hang < p_unrel, "related should rank above unrelated: {out}");
        }
    }

    #[tokio::test]
    async fn allowed_roots_scopes_search() {
        let pool = open_in_memory().await.unwrap();
        insert_test_email(
            &pool,
            " <a@x>",
            "client hang alpha",
            "a@b",
            "2026-07-15T00:00:00+00:00",
            "b\n",
            None,
            "[]",
        )
        .await
        .unwrap();
        insert_test_email(
            &pool,
            " <b@x>",
            "client hang beta",
            "a@b",
            "2026-07-16T00:00:00+00:00",
            "b\n",
            None,
            "[]",
        )
        .await
        .unwrap();
        let index = Arc::new(EmailIndex::load(&pool).await.unwrap());
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, PathBuf::from("/tmp/out"), w, week_window(w))
            .with_allowed_roots(Some(normalize_root_set(["<a@x>"])));

        let out = search_related_threads(
            &ctx,
            SearchRelatedThreadsArgs {
                subject: "client hang".into(),
                limit: Some(10),
            },
        )
        .await
        .unwrap();
        assert!(out.contains("<a@x>"));
        assert!(!out.contains("<b@x>"));
    }
}
