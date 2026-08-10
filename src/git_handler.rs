use crate::models::EmailMessage;
use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use git2::{Blob, Oid, Repository};
use mailparse::{parse_mail, MailHeaderMap};
use std::vec;

pub struct GitHandler {
    repo: Repository,
}

impl GitHandler {
    pub fn open(path: &str) -> Result<Self> {
        let repo = Repository::open(path).context("Failed to open git repository")?;
        Ok(Self { repo })
    }

    pub fn get_latest_message_date(&self) -> Result<DateTime<Utc>> {
        let mut revwalk = self.repo.revwalk()?;
        let head = self.repo.revparse_single("HEAD")?.id();
        revwalk.push(head)?;

        for id in revwalk {
            let id = id?;
            let commit = self.repo.find_commit(id)?;
            let tree = commit.tree()?;
            if tree.get_name("m").is_some() {
                return Utc
                    .timestamp_opt(commit.time().seconds(), 0)
                    .single()
                    .context("Invalid commit timestamp");
            }
        }

        Err(anyhow::anyhow!("No email messages found in repository"))
    }

    pub fn get_messages(
        &self,
        now: DateTime<Utc>,
        lookback_days: i64,
    ) -> Result<Vec<EmailMessage>> {
        let cutoff = now - chrono::Duration::days(lookback_days);

        let mut revwalk = self.repo.revwalk()?;
        let head = self.repo.revparse_single("HEAD")?.id();
        revwalk.push(head)?;

        let mut messages = Vec::new();

        for id in revwalk {
            let id = id?;
            let commit = self.repo.find_commit(id)?;
            let commit_time = Utc
                .timestamp_opt(commit.time().seconds(), 0)
                .single()
                .context("Invalid commit timestamp")?;

            if commit_time > now {
                continue;
            }

            if commit_time < cutoff {
                break;
            }

            let tree = commit.tree()?;
            if let Some(entry) = tree.get_name("m") {
                let blob = self.repo.find_blob(entry.id())?;
                if let Ok(msg) = parse_email_blob(&blob, commit_time) {
                    messages.push(msg);
                }
            }
        }

        Ok(messages)
    }

    /// Consume the handler and return a streaming iterator over every email
    /// message in the repository (no date cutoff).
    ///
    /// Commit OIDs are discovered up front (cheap); message bodies are parsed
    /// lazily as the iterator is advanced.
    pub fn get_all_messages(self) -> Result<MessageIter> {
        let mut revwalk = self.repo.revwalk()?;
        let head = self.repo.revparse_single("HEAD")?.id();
        revwalk.push(head)?;

        let mut oids = Vec::new();
        for id in revwalk {
            let id = id?;
            let commit = self.repo.find_commit(id)?;
            if commit.tree()?.get_name("m").is_some() {
                oids.push(id);
            }
        }

        let total = oids.len() as u64;
        Ok(MessageIter {
            repo: self.repo,
            oids: oids.into_iter(),
            total,
        })
    }
}

/// Streaming iterator of emails that owns the underlying git repository.
pub struct MessageIter {
    repo: Repository,
    oids: vec::IntoIter<Oid>,
    total: u64,
}

impl MessageIter {
    /// Number of mail commits discovered in the repository.
    pub fn len(&self) -> u64 {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

impl Iterator for MessageIter {
    type Item = Result<EmailMessage>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let id = self.oids.next()?;
            match message_from_commit(&self.repo, id) {
                // Skip unparseable messages (same as get_messages).
                Ok(None) => continue,
                Ok(Some(msg)) => return Some(Ok(msg)),
                Err(e) => return Some(Err(e)),
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.oids.size_hint()
    }
}

impl ExactSizeIterator for MessageIter {
    fn len(&self) -> usize {
        self.oids.len()
    }
}

fn message_from_commit(repo: &Repository, id: Oid) -> Result<Option<EmailMessage>> {
    let commit = repo.find_commit(id)?;
    let commit_time = Utc
        .timestamp_opt(commit.time().seconds(), 0)
        .single()
        .context("Invalid commit timestamp")?;
    let tree = commit.tree()?;
    let Some(entry) = tree.get_name("m") else {
        return Ok(None);
    };
    let blob = repo.find_blob(entry.id())?;
    match parse_email_blob(&blob, commit_time) {
        Ok(msg) => Ok(Some(msg)),
        Err(_) => Ok(None),
    }
}

fn parse_email_blob(blob: &Blob, date: DateTime<Utc>) -> Result<EmailMessage> {
    let content = blob.content();
    let mail = parse_mail(content).context("Failed to parse email")?;

    let headers = mail.get_headers();
    let message_id = headers
        .get_first_value("Message-ID")
        .unwrap_or_else(|| format!("unknown-{}", date.timestamp_nanos_opt().unwrap_or(0)));
    let subject = headers.get_first_value("Subject").unwrap_or_default();
    let from = headers.get_first_value("From").unwrap_or_default();
    let in_reply_to = headers.get_first_value("In-Reply-To");
    let references = headers
        .get_first_value("References")
        .map(|r| r.split_whitespace().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let body = mail.get_body().unwrap_or_default();
    let body = crate::content_cleaner::clean_email_body(&body);

    Ok(EmailMessage {
        message_id,
        subject,
        from,
        date,
        body,
        in_reply_to,
        references,
    })
}
