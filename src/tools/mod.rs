//! Pure tool handlers for the summarizer agents (no da-harness yet).
//!
//! PR3: mail tools. PR4+: outputs tools / submit. PR5 wraps these in `Tool::new`.

mod get_email;
mod grep_emails;
mod list_thread_messages;

#[allow(unused_imports)] // re-exported for agents (PR5+) and external tests
pub use get_email::{get_email, GetEmailArgs};
#[allow(unused_imports)]
pub use grep_emails::{grep_emails, GrepEmailsArgs};
#[allow(unused_imports)]
pub use list_thread_messages::{list_thread_messages, ListThreadMessagesArgs};

use crate::email_index::EmailIndex;
use crate::lore::DEFAULT_LORE_BASE;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;

/// Shared context for pure tool handlers.
#[derive(Clone)]
#[allow(dead_code)] // outputs_path / week_ending used by PR4+ and agents
pub struct ToolCtx {
    pub pool: SqlitePool,
    pub index: Arc<EmailIndex>,
    /// Outputs root (absolute preferred). Unused by mail tools; required for PR4+.
    pub outputs_path: PathBuf,
    pub week_ending: NaiveDate,
    /// Half-open UTC window for the current week edition.
    pub week_window: (DateTime<Utc>, DateTime<Utc>),
    /// Thread agent focus: Some(normalized root). Ordering / week: None.
    pub focus_thread_root: Option<String>,
    /// Lore archive base for message links in tool output.
    pub lore_base_url: String,
}

impl ToolCtx {
    pub fn new(
        pool: SqlitePool,
        index: Arc<EmailIndex>,
        outputs_path: PathBuf,
        week_ending: NaiveDate,
        week_window: (DateTime<Utc>, DateTime<Utc>),
    ) -> Self {
        Self {
            pool,
            index,
            outputs_path,
            week_ending,
            week_window,
            focus_thread_root: None,
            lore_base_url: DEFAULT_LORE_BASE.to_string(),
        }
    }

    pub fn with_focus(mut self, root: Option<String>) -> Self {
        self.focus_thread_root = root.map(|s| crate::ids::normalize_message_id(&s));
        self
    }

    pub fn with_lore_base(mut self, lore_base_url: impl Into<String>) -> Self {
        self.lore_base_url = lore_base_url.into();
        self
    }
}
