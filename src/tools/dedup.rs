//! Per-session duplicate detection for read-only agent tools.
//!
//! Mail and output tools read immutable data (SQLite / the outputs tree). Repeating
//! the same call in one session is wasted work: the first result is already in
//! conversation history. Track seen `(tool, args)` pairs in a `HashMap` and return
//! an error on a second identical request.
//!
//! Scope is one agent session (`build_*_tools` call), not the whole week run.

use serde::Serialize;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{Arc, Mutex};

/// Shared tracker: identical read-only tool calls in one session are rejected.
#[derive(Clone, Default)]
pub struct RequestDeduper {
    seen: Arc<Mutex<HashMap<(String, String), ()>>>,
}

impl RequestDeduper {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record this request. `Err` is a model-visible `ERROR: …` string if the
    /// same tool+args pair was already issued in this session.
    ///
    /// Inserts **before** the caller runs the handler so two parallel identical
    /// calls (see `parallel_tools(true)`) only execute once.
    pub fn check_or_insert(&self, tool: &str, args: &impl Serialize) -> Result<(), String> {
        let fingerprint = serde_json::to_string(args).unwrap_or_default();
        let key = (tool.to_string(), fingerprint);
        let mut g = self.seen.lock().expect("request deduper mutex");
        match g.entry(key) {
            Entry::Occupied(_) => Err(duplicate_error(tool)),
            Entry::Vacant(v) => {
                v.insert(());
                Ok(())
            }
        }
    }
}

fn duplicate_error(tool: &str) -> String {
    format!(
        "ERROR: duplicate {tool} request; this exact call was already issued in this session. Use the previous tool result."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Serialize)]
    struct Args {
        n: u32,
    }

    #[test]
    fn first_ok_second_duplicate() {
        let d = RequestDeduper::new();
        let args = Args { n: 1 };
        assert!(d.check_or_insert("GetEmail", &args).is_ok());
        let err = d.check_or_insert("GetEmail", &args).unwrap_err();
        assert!(err.contains("ERROR: duplicate GetEmail"));
        assert!(err.contains("already issued"));
    }

    #[test]
    fn different_tool_or_args_ok() {
        let d = RequestDeduper::new();
        assert!(d.check_or_insert("GetEmail", &Args { n: 1 }).is_ok());
        assert!(
            d.check_or_insert("ListThreadMessages", &Args { n: 1 })
                .is_ok()
        );
        assert!(d.check_or_insert("GetEmail", &Args { n: 2 }).is_ok());
        assert!(d.check_or_insert("GetEmail", &Args { n: 1 }).is_err());
    }

    #[test]
    fn sessions_are_independent() {
        let a = RequestDeduper::new();
        let b = RequestDeduper::new();
        assert!(a.check_or_insert("GetEmail", &Args { n: 1 }).is_ok());
        assert!(b.check_or_insert("GetEmail", &Args { n: 1 }).is_ok());
    }

    #[test]
    fn concurrent_identical_one_ok() {
        let d = RequestDeduper::new();
        let ok = AtomicUsize::new(0);
        let err = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| match d.check_or_insert("GetEmail", &Args { n: 1 }) {
                    Ok(()) => {
                        ok.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(_) => {
                        err.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        });
        assert_eq!(ok.load(Ordering::SeqCst), 1);
        assert_eq!(err.load(Ordering::SeqCst), 7);
    }
}
