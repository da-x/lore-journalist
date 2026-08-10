#![allow(unused)]
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::collections::HashMap;

/// Email fields without the body (body stays on disk, zstd-compressed).
#[derive(Debug, Clone)]
pub struct EmailMeta {
    pub message_id: String,
    pub subject: String,
    pub from: String,
    pub date: DateTime<Utc>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

/// A conversation thread: messages sorted by date.
#[derive(Debug, Clone)]
pub struct MetaThread {
    pub root_id: String,
    /// Subject of the earliest message in the thread.
    pub subject: String,
    /// Indices into `EmailIndex::emails`.
    pub message_indices: Vec<usize>,
}

/// In-memory index of every email's metadata from the SQLite database.
#[derive(Debug, Clone)]
pub struct EmailIndex {
    emails: Vec<EmailMeta>,
    by_message_id: HashMap<String, usize>,
    /// message_id → index of the email it replies to (only if that parent is in the index).
    replies_to: HashMap<String, usize>,
}

impl EmailIndex {
    /// Load all email metadata from the database (does not read `body`).
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

            let idx = emails.len();
            by_message_id.insert(row.message_id.clone(), idx);
            emails.push(EmailMeta {
                message_id: row.message_id,
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
            if let Some(&parent_idx) = by_message_id.get(parent_id) {
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

    pub fn get(&self, message_id: &str) -> Option<&EmailMeta> {
        self.by_message_id
            .get(message_id)
            .map(|&idx| &self.emails[idx])
    }

    /// The email this message replies to, if `In-Reply-To` is set and present in the index.
    pub fn replies_to(&self, message_id: &str) -> Option<&EmailMeta> {
        self.replies_to
            .get(message_id)
            .map(|&idx| &self.emails[idx])
    }

    pub fn len(&self) -> usize {
        self.emails.len()
    }

    pub fn is_empty(&self) -> bool {
        self.emails.is_empty()
    }

    pub fn contains(&self, message_id: &str) -> bool {
        self.by_message_id.contains_key(message_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &EmailMeta> {
        self.emails.iter()
    }

    /// Group all messages into threads (same root rules as `process_threads`).
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

    /// Load and zstd-decompress the body for a single message.
    pub async fn load_body(pool: &SqlitePool, message_id: &str) -> Result<String> {
        let row = sqlx::query!(
            r#"SELECT body AS "body!" FROM emails WHERE message_id = ?"#,
            message_id
        )
        .fetch_one(pool)
        .await
        .with_context(|| format!("Failed to load body for {message_id}"))?;

        decompress_body(&row.body)
    }

    /// Load and decompress bodies for every message id → plain text.
    pub async fn load_all_bodies(pool: &SqlitePool) -> Result<HashMap<String, String>> {
        let rows = sqlx::query!(r#"SELECT message_id, body AS "body!" FROM emails"#)
            .fetch_all(pool)
            .await
            .context("Failed to load email bodies")?;

        let mut bodies = HashMap::with_capacity(rows.len());
        for row in rows {
            let text = decompress_body(&row.body)
                .with_context(|| format!("Failed to decompress body for {}", row.message_id))?;
            bodies.insert(row.message_id, text);
        }
        Ok(bodies)
    }

    /// Compose Subject + Body text for every message in a thread (in date order).
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

fn thread_root_id(msg: &EmailMeta) -> String {
    if !msg.references.is_empty() {
        msg.references[0].clone()
    } else if let Some(ref parent) = msg.in_reply_to {
        parent.clone()
    } else {
        msg.message_id.clone()
    }
}

fn decompress_body(compressed: &[u8]) -> Result<String> {
    let bytes = zstd::decode_all(compressed).context("zstd decompression failed")?;
    String::from_utf8(bytes).context("Email body is not valid UTF-8")
}
