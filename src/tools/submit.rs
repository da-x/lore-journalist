//! Submit tool payloads and Mutex slots (KD21).

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// One-shot submit slot: first writer wins; double-submit returns error to the model.
#[derive(Clone, Default)]
pub struct SubmitSlot<T> {
    inner: Arc<Mutex<Option<T>>>,
}

impl<T> SubmitSlot<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    /// Store payload if empty. Returns Ok if stored, Err message if already submitted.
    pub fn try_submit(&self, value: T) -> Result<(), String> {
        let mut g = self.inner.lock().expect("submit slot lock");
        if g.is_some() {
            return Err("ERROR: already submitted".into());
        }
        *g = Some(value);
        Ok(())
    }

    pub fn is_filled(&self) -> bool {
        self.inner.lock().expect("submit slot lock").is_some()
    }

    pub fn take(&self) -> Option<T> {
        self.inner.lock().expect("submit slot lock").take()
    }

}

/// Payload for `SubmitThreadOrder`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadOrderPayload {
    pub ordered_root_ids: Vec<String>,
    pub notes: Option<String>,
}

/// Payload for `SubmitThreadSummary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadSummaryPayload {
    pub title: String,
    pub markdown_body: String,
    pub key_message_ids: Vec<String>,
}

/// Finish ordering agent: ordered list of thread roots.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SubmitThreadOrder {
    /// Every active thread root_id exactly once (normalized Message-IDs).
    pub ordered_root_ids: Vec<String>,
    /// Optional short rationale for host logs only.
    pub notes: Option<String>,
}

/// Finish thread agent with the weekly summary markdown.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SubmitThreadSummary {
    /// Short title for the discussion.
    pub title: String,
    /// Markdown body of this week's summary (lore links for messages).
    pub markdown_body: String,
    /// Key message ids cited (normalized).
    #[serde(default)]
    pub key_message_ids: Vec<String>,
}

pub fn submit_thread_order(
    slot: &SubmitSlot<ThreadOrderPayload>,
    args: SubmitThreadOrder,
) -> String {
    let payload = ThreadOrderPayload {
        ordered_root_ids: args.ordered_root_ids,
        notes: args.notes,
    };
    match slot.try_submit(payload) {
        Ok(()) => "submitted".to_string(),
        Err(e) => e,
    }
}

pub fn submit_thread_summary(
    slot: &SubmitSlot<ThreadSummaryPayload>,
    args: SubmitThreadSummary,
) -> String {
    if args.markdown_body.trim().is_empty() {
        return "ERROR: markdown_body must be non-empty".to_string();
    }
    let payload = ThreadSummaryPayload {
        title: args.title,
        markdown_body: args.markdown_body,
        key_message_ids: args.key_message_ids,
    };
    match slot.try_submit(payload) {
        Ok(()) => "submitted".to_string(),
        Err(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_submit_rejected() {
        let slot = SubmitSlot::new();
        assert!(slot
            .try_submit(ThreadOrderPayload {
                ordered_root_ids: vec!["<a>".into()],
                notes: None,
            })
            .is_ok());
        let err = slot
            .try_submit(ThreadOrderPayload {
                ordered_root_ids: vec!["<b>".into()],
                notes: None,
            })
            .unwrap_err();
        assert!(err.contains("already submitted"));
        let p = slot.take().unwrap();
        assert_eq!(p.ordered_root_ids, vec!["<a>"]);
    }
}
