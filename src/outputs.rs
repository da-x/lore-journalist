//! Output path builders for weekly markdown editions (design output layout).

use crate::ids::file_stem_for_id;
use chrono::NaiveDate;
use std::path::{Path, PathBuf};

/// `{outputs}/{YYYY-MM-DD}`.
pub fn week_dir(outputs_path: &Path, w: NaiveDate) -> PathBuf {
    outputs_path.join(w.format("%Y-%m-%d").to_string())
}

/// Root catalog: `{outputs}/index.md`.
pub fn root_index_path(outputs_path: &Path) -> PathBuf {
    outputs_path.join("index.md")
}

/// Exclusive lock file: `{outputs}/.summarize-week.lock`.
pub fn summarize_lock_path(outputs_path: &Path) -> PathBuf {
    outputs_path.join(".summarize-week.lock")
}

/// `{outputs}/{W}/index.md`.
pub fn week_index_path(outputs_path: &Path, w: NaiveDate) -> PathBuf {
    week_dir(outputs_path, w).join("index.md")
}

/// `{outputs}/{W}/.complete`.
pub fn complete_marker_path(outputs_path: &Path, w: NaiveDate) -> PathBuf {
    week_dir(outputs_path, w).join(".complete")
}

/// `{outputs}/{W}/.thread-order.json`.
pub fn thread_order_path(outputs_path: &Path, w: NaiveDate) -> PathBuf {
    week_dir(outputs_path, w).join(".thread-order.json")
}

/// `{outputs}/{W}/thread/`.
pub fn thread_dir(outputs_path: &Path, w: NaiveDate) -> PathBuf {
    week_dir(outputs_path, w).join("thread")
}

/// `{outputs}/{W}/messages/`.
pub fn messages_dir(outputs_path: &Path, w: NaiveDate) -> PathBuf {
    week_dir(outputs_path, w).join("messages")
}

/// `{outputs}/{W}/thread/{stem}.md` for a thread root id (any form; normalized inside).
pub fn thread_markdown_path(outputs_path: &Path, w: NaiveDate, thread_root_id: &str) -> PathBuf {
    let stem = file_stem_for_id(thread_root_id);
    thread_dir(outputs_path, w).join(format!("{stem}.md"))
}

/// `{outputs}/{W}/messages/{stem}.md` for a message id (any form; normalized inside).
pub fn message_markdown_path(outputs_path: &Path, w: NaiveDate, message_id: &str) -> PathBuf {
    let stem = file_stem_for_id(message_id);
    messages_dir(outputs_path, w).join(format!("{stem}.md"))
}

/// Glob pattern (relative to `outputs_path`) for all weeks of a thread:
/// `*/thread/{stem}.md`.
pub fn prior_thread_glob_pattern(thread_root_id: &str) -> String {
    let stem = file_stem_for_id(thread_root_id);
    format!("*/thread/{stem}.md")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn paths_match_design_layout() {
        let root = Path::new("/tmp/out");
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        assert_eq!(root_index_path(root), PathBuf::from("/tmp/out/index.md"));
        assert_eq!(
            week_index_path(root, w),
            PathBuf::from("/tmp/out/2026-07-20/index.md")
        );
        assert_eq!(
            complete_marker_path(root, w),
            PathBuf::from("/tmp/out/2026-07-20/.complete")
        );
        assert_eq!(
            thread_order_path(root, w),
            PathBuf::from("/tmp/out/2026-07-20/.thread-order.json")
        );

        let msg = " <abc@def.com>";
        assert_eq!(
            message_markdown_path(root, w, msg),
            PathBuf::from("/tmp/out/2026-07-20/messages/%3Cabc%40def.com%3E.md")
        );
        assert_eq!(
            thread_markdown_path(root, w, msg),
            PathBuf::from("/tmp/out/2026-07-20/thread/%3Cabc%40def.com%3E.md")
        );
        assert_eq!(
            prior_thread_glob_pattern(msg),
            "*/thread/%3Cabc%40def.com%3E.md"
        );
    }
}
