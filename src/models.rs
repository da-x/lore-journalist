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
