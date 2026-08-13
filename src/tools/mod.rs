//! Pure tool handlers for the summarizer agents (no da-harness yet).
//!
//! PR3: mail tools. PR4: outputs tools + related search. PR5 wraps these in `Tool::new`.

pub mod get_email;
pub mod glob_outputs;
pub mod grep_emails;
pub mod grep_outputs;
pub mod list_thread_messages;
pub mod paths;
pub mod read_output_file;
pub mod search_related_threads;
pub mod submit;

#[allow(unused_imports)] // re-exported for agents (PR5+)
pub use get_email::{GetEmailArgs, get_email};
#[allow(unused_imports)]
pub use glob_outputs::{GlobOutputsArgs, glob_outputs};
#[allow(unused_imports)]
pub use grep_emails::{GrepEmailsArgs, grep_emails};
#[allow(unused_imports)]
pub use grep_outputs::{GrepOutputsArgs, grep_outputs};
#[allow(unused_imports)]
pub use list_thread_messages::{ListThreadMessagesArgs, list_thread_messages};
#[allow(unused_imports)]
pub use paths::{path_glob_match, resolve_output_path};
#[allow(unused_imports)]
pub use read_output_file::{ReadOutputFileArgs, read_output_file};
#[allow(unused_imports)]
pub use search_related_threads::{
    SearchRelatedThreadsArgs, normalize_subject, search_related_threads, subject_tokens,
};

use crate::config::ListConfig;
use crate::email_index::EmailIndex;
use crate::lore::DEFAULT_LORE_BASE;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Shared context for pure tool handlers.
#[derive(Clone)]
pub struct ToolCtx {
    pub pool: SqlitePool,
    pub index: Arc<EmailIndex>,
    /// Outputs root (prefer canonical absolute path).
    pub outputs_path: PathBuf,
    #[allow(dead_code)] // reserved for agent prompts / future tools
    pub week_ending: NaiveDate,
    /// Half-open UTC window for the current week edition.
    pub week_window: (DateTime<Utc>, DateTime<Utc>),
    /// Thread agent focus: Some(normalized root). Ordering / week: None.
    pub focus_thread_root: Option<String>,
    /// Lore archive base for message links in tool output.
    pub lore_base_url: String,
    /// Per-list identity / agent briefing.
    pub list: ListConfig,
    /// When set (e.g. ordering agent), `SearchRelatedThreads` only considers these roots.
    pub allowed_thread_roots: Option<HashSet<String>>,
}

impl ToolCtx {
    pub fn new(
        pool: SqlitePool,
        index: Arc<EmailIndex>,
        outputs_path: PathBuf,
        week_ending: NaiveDate,
        week_window: (DateTime<Utc>, DateTime<Utc>),
    ) -> Self {
        let outputs_path = if outputs_path.exists() {
            outputs_path.canonicalize().unwrap_or(outputs_path)
        } else {
            outputs_path
        };
        Self {
            pool,
            index,
            outputs_path,
            week_ending,
            week_window,
            focus_thread_root: None,
            lore_base_url: DEFAULT_LORE_BASE.to_string(),
            list: ListConfig::default(),
            allowed_thread_roots: None,
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

    pub fn with_list(mut self, list: ListConfig) -> Self {
        self.list = list;
        self
    }

    pub fn with_allowed_roots(mut self, roots: Option<HashSet<String>>) -> Self {
        self.allowed_thread_roots = roots;
        self
    }
}
