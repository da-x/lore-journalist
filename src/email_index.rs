//! In-memory email metadata index with dual Message-ID form (design KD4).

use crate::ids::normalize_message_id;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::collections::HashMap;
use tracing::warn;

/// Email fields without the body (body stays on disk, zstd-compressed).
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields consumed by later summarize/tools PRs
pub struct EmailMeta {
    /// Canonical form for external use: tools, agents, front matter, roots, stems.
    pub message_id: String,
    /// Exact SQLite PRIMARY KEY; only for body SQL and debug logs.
    pub message_id_raw: String,
    pub subject: String,
    pub from: String,
    pub date: DateTime<Utc>,
    /// As stored / may need normalize on use for parent lookup.
    pub in_reply_to: Option<String>,
    /// Elements as stored in DB; always normalize before use.
    pub references: Vec<String>,
}

/// A conversation thread: messages sorted by date.
#[derive(Debug, Clone)]
pub struct MetaThread {
    /// Normalized root Message-ID.
    pub root_id: String,
    /// Subject of the earliest message in the thread.
    pub subject: String,
    /// Indices into `EmailIndex::emails`.
    pub message_indices: Vec<usize>,
}

/// In-memory index of every email's metadata from the SQLite database.
#[derive(Debug, Clone)]
#[allow(dead_code)] // full API used by summarize/tools; grep uses a subset today
pub struct EmailIndex {
    emails: Vec<EmailMeta>,
    /// Normalized message_id → index into `emails`.
    by_message_id: HashMap<String, usize>,
    /// Normalized message_id → index of parent (if parent present in index).
    replies_to: HashMap<String, usize>,
}

impl EmailIndex {
    /// Empty index (output-only CLI tools that do not scan mail metadata).
    pub fn empty() -> Self {
        Self {
            emails: Vec::new(),
            by_message_id: HashMap::new(),
            replies_to: HashMap::new(),
        }
    }

    /// Load all email metadata from the database (does not read `body`).
    ///
    /// Dual-ID contract: `message_id` is normalized; `message_id_raw` is the SQLite PK.
    /// Map keys are normalized. Collision: earliest-by-date wins (rows ordered by date).
    pub async fn load(pool: &SqlitePool) -> Result<Self> {
        let rows = sqlx::query!(
            r#"
            SELECT message_id, subject, from_addr, date, in_reply_to, "references" AS references_json
            FROM emails
            ORDER BY date
            "#
        )
        .fetch_all(pool)
        .await
        .context("Failed to load email metadata from database")?;

        let mut emails = Vec::with_capacity(rows.len());
        let mut by_message_id = HashMap::with_capacity(rows.len());

        for row in rows {
            let date = DateTime::parse_from_rfc3339(&row.date)
                .with_context(|| {
                    format!("Invalid date for message {}: {}", row.message_id, row.date)
                })?
                .with_timezone(&Utc);
            let references: Vec<String> =
                serde_json::from_str(&row.references_json).with_context(|| {
                    format!("Invalid references JSON for message {}", row.message_id)
                })?;

            let message_id_raw = row.message_id;
            let message_id = normalize_message_id(&message_id_raw);

            if let Some(&existing_idx) = by_message_id.get(&message_id) {
                // Earliest-by-date already in map (ORDER BY date); drop this row.
                let existing: &EmailMeta = &emails[existing_idx];
                warn!(
                    normalized = %message_id,
                    kept_raw = %existing.message_id_raw,
                    dropped_raw = %message_id_raw,
                    "duplicate Message-ID after normalize; keeping earliest by date"
                );
                continue;
            }

            let idx = emails.len();
            by_message_id.insert(message_id.clone(), idx);
            emails.push(EmailMeta {
                message_id,
                message_id_raw,
                subject: row.subject,
                from: row.from_addr,
                date,
                in_reply_to: row.in_reply_to,
                references,
            });
        }

        let mut replies_to = HashMap::new();
        for email in &emails {
            let Some(parent_id) = email.in_reply_to.as_deref() else {
                continue;
            };
            let parent_norm = normalize_message_id(parent_id);
            if let Some(&parent_idx) = by_message_id.get(&parent_norm) {
                replies_to.insert(email.message_id.clone(), parent_idx);
            }
        }

        Ok(Self {
            emails,
            by_message_id,
            replies_to,
        })
    }

    pub fn emails(&self) -> &[EmailMeta] {
        &self.emails
    }

    /// Lookup by flexible id (raw or normalized); normalizes input first.
    #[allow(dead_code)]
    pub fn get(&self, message_id: &str) -> Option<&EmailMeta> {
        let key = normalize_message_id(message_id);
        self.by_message_id.get(&key).map(|&idx| &self.emails[idx])
    }

    /// The email this message replies to, if parent is present in the index.
    #[allow(dead_code)]
    pub fn replies_to(&self, message_id: &str) -> Option<&EmailMeta> {
        let key = normalize_message_id(message_id);
        self.replies_to.get(&key).map(|&idx| &self.emails[idx])
    }

    pub fn len(&self) -> usize {
        self.emails.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.emails.is_empty()
    }

    #[allow(dead_code)]
    pub fn contains(&self, message_id: &str) -> bool {
        self.by_message_id
            .contains_key(&normalize_message_id(message_id))
    }

    #[allow(dead_code)]
    pub fn iter(&self) -> impl Iterator<Item = &EmailMeta> {
        self.emails.iter()
    }

    /// Group all messages into threads (normalized `thread_root_id` rules).
    pub fn threads(&self) -> Vec<MetaThread> {
        let mut thread_map: HashMap<String, Vec<usize>> = HashMap::new();

        for (idx, msg) in self.emails.iter().enumerate() {
            let root_id = thread_root_id(msg);
            thread_map.entry(root_id).or_default().push(idx);
        }

        let mut threads: Vec<MetaThread> = thread_map
            .into_iter()
            .map(|(root_id, mut indices)| {
                indices.sort_by_key(|&i| self.emails[i].date);
                let subject = self.emails[indices[0]].subject.clone();
                MetaThread {
                    root_id,
                    subject,
                    message_indices: indices,
                }
            })
            .collect();

        threads.sort_by_key(|t| {
            t.message_indices
                .last()
                .map(|&i| self.emails[i].date)
                .unwrap_or_default()
        });
        threads.reverse();
        threads
    }

    /// Load and zstd-decompress the body for a message.
    ///
    /// Accepts raw or normalized id; always binds **`message_id_raw`** in SQL.
    pub async fn load_body(&self, pool: &SqlitePool, message_id: &str) -> Result<String> {
        let meta = self.get(message_id).with_context(|| {
            format!(
                "unknown message_id after normalize: {:?}",
                normalize_message_id(message_id)
            )
        })?;
        let raw = &meta.message_id_raw;
        let row = sqlx::query!(
            r#"SELECT body AS "body!" FROM emails WHERE message_id = ?"#,
            raw
        )
        .fetch_one(pool)
        .await
        .with_context(|| {
            format!(
                "Failed to load body for normalized={} raw={raw:?}",
                meta.message_id
            )
        })?;

        decompress_body(&row.body)
    }

    /// Load and decompress bodies keyed by **normalized** message_id.
    ///
    /// On normalized-id collision, earliest-by-date wins (`ORDER BY date`, first insert kept).
    /// Prefer [`Self::load_body`] when you need the index winner's raw PK path.
    pub async fn load_all_bodies(pool: &SqlitePool) -> Result<HashMap<String, String>> {
        let rows = sqlx::query!(r#"SELECT message_id, body AS "body!" FROM emails ORDER BY date"#)
            .fetch_all(pool)
            .await
            .context("Failed to load email bodies")?;

        let mut bodies = HashMap::with_capacity(rows.len());
        for row in rows {
            let key = normalize_message_id(&row.message_id);
            let text = decompress_body(&row.body)
                .with_context(|| format!("Failed to decompress body for {}", row.message_id))?;
            bodies.entry(key).or_insert(text);
        }
        Ok(bodies)
    }

    /// Compose Subject + Body text for every message in a thread (in date order).
    ///
    /// Expects `bodies` keys to be **normalized** message ids (as from `load_all_bodies`).
    pub fn compose_thread_text(
        &self,
        thread: &MetaThread,
        bodies: &HashMap<String, String>,
    ) -> String {
        let mut out = String::new();
        for &idx in &thread.message_indices {
            let msg = &self.emails[idx];
            let body = bodies
                .get(&msg.message_id)
                .map(String::as_str)
                .unwrap_or("");
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str("Subject: ");
            out.push_str(&msg.subject);
            out.push('\n');
            out.push_str(body);
            if !body.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    }
}

/// Thread root Message-ID (normalized).
///
/// Rules: `references[0]` if non-empty, else `in_reply_to`, else self `message_id`.
pub fn thread_root_id(msg: &EmailMeta) -> String {
    if !msg.references.is_empty() {
        normalize_message_id(&msg.references[0])
    } else if let Some(ref parent) = msg.in_reply_to {
        normalize_message_id(parent)
    } else {
        // message_id is already normalized in the index.
        msg.message_id.clone()
    }
}

fn decompress_body(compressed: &[u8]) -> Result<String> {
    let bytes = zstd::decode_all(compressed).context("zstd decompression failed")?;
    String::from_utf8(bytes).context("Email body is not valid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    async fn pool_with_schema() -> SqlitePool {
        crate::db::open_in_memory()
            .await
            .expect("in-memory db + migrations")
    }

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
        let from_addr = "a@b";
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

    #[tokio::test]
    async fn leading_space_pk_load_body_and_compose() {
        let pool = pool_with_schema().await;
        // Real corpus shape: leading space in PK.
        insert_email(
            &pool,
            " <id@example.com>",
            "Hello",
            "2026-07-18T12:00:00+00:00",
            "body-one\n",
            None,
            "[]",
        )
        .await;

        let index = EmailIndex::load(&pool).await.unwrap();
        assert_eq!(index.len(), 1);
        let meta = index.get("<id@example.com>").unwrap();
        assert_eq!(meta.message_id, "<id@example.com>");
        assert_eq!(meta.message_id_raw, " <id@example.com>");

        // Flexible id forms.
        let body_norm = index.load_body(&pool, "<id@example.com>").await.unwrap();
        let body_raw = index.load_body(&pool, " <id@example.com>").await.unwrap();
        assert_eq!(body_norm, "body-one\n");
        assert_eq!(body_raw, "body-one\n");

        let bodies = EmailIndex::load_all_bodies(&pool).await.unwrap();
        assert!(bodies.contains_key("<id@example.com>"));
        assert!(!bodies.contains_key(" <id@example.com>"));

        let threads = index.threads();
        assert_eq!(threads.len(), 1);
        let text = index.compose_thread_text(&threads[0], &bodies);
        assert!(text.contains("body-one"), "compose text was: {text:?}");
        assert!(text.contains("Subject: Hello"));
    }

    #[tokio::test]
    async fn collision_keeps_earliest_raw_pk() {
        let pool = pool_with_schema().await;
        insert_email(
            &pool,
            " <same@x>",
            "first",
            "2026-01-01T00:00:00+00:00",
            "early\n",
            None,
            "[]",
        )
        .await;
        // Same normalized id, different raw PK (trailing space only on second is still same after trim —
        // use a form that SQLite allows as distinct PK but normalizes equal: leading double space).
        insert_email(
            &pool,
            "  <same@x>",
            "second",
            "2026-01-02T00:00:00+00:00",
            "late\n",
            None,
            "[]",
        )
        .await;

        let index = EmailIndex::load(&pool).await.unwrap();
        assert_eq!(index.len(), 1);
        let meta = index.get("<same@x>").unwrap();
        assert_eq!(meta.message_id_raw, " <same@x>");
        assert_eq!(meta.subject, "first");
        let body = index.load_body(&pool, "<same@x>").await.unwrap();
        assert_eq!(body, "early\n");
    }

    #[tokio::test]
    async fn thread_root_normalizes_parent() {
        let pool = pool_with_schema().await;
        insert_email(
            &pool,
            " <root@x>",
            "Root",
            "2026-01-01T00:00:00+00:00",
            "r\n",
            None,
            "[]",
        )
        .await;
        insert_email(
            &pool,
            " <child@x>",
            "Re: Root",
            "2026-01-02T00:00:00+00:00",
            "c\n",
            Some(" <root@x>"),
            r#"[" <root@x>"]"#,
        )
        .await;

        let index = EmailIndex::load(&pool).await.unwrap();
        let threads = index.threads();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].root_id, "<root@x>");
        assert_eq!(threads[0].message_indices.len(), 2);
        assert!(index.replies_to("<child@x>").unwrap().message_id == "<root@x>");
    }

    #[test]
    fn thread_root_id_self() {
        let msg = EmailMeta {
            message_id: "<a@b>".into(),
            message_id_raw: " <a@b>".into(),
            subject: "s".into(),
            from: "f".into(),
            date: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            in_reply_to: None,
            references: vec![],
        };
        assert_eq!(thread_root_id(&msg), "<a@b>");
    }
}
