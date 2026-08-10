use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailMessage {
    pub message_id: String,
    pub subject: String,
    pub from: String,
    pub date: DateTime<Utc>,
    pub body: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub root_id: String,
    pub subject: String,
    pub messages: Vec<EmailMessage>,
}

impl Thread {
    pub fn last_activity(&self) -> DateTime<Utc> {
        self.messages
            .iter()
            .map(|m| m.date)
            .max()
            .expect("Thread must have at least one message")
    }
}
