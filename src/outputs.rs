//! Output path builders and writers for weekly markdown editions.

use crate::email_index::{thread_root_id, EmailMeta};
use crate::ids::{file_stem_for_id, normalize_message_id};
use crate::week::week_window;
use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::fs::{self, File};
use std::io::Write;
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
#[allow(dead_code)] // used when flock lands (PR6)
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
#[allow(dead_code)] // ordering agent (PR5+)
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
#[allow(dead_code)] // thread agents (PR5+)
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
#[allow(dead_code)] // prior-week discovery (PR5+)
pub fn prior_thread_glob_pattern(thread_root_id: &str) -> String {
    let stem = file_stem_for_id(thread_root_id);
    format!("*/thread/{stem}.md")
}

/// Create `W/`, `W/thread/`, `W/messages/`.
pub fn ensure_week_layout(outputs_path: &Path, w: NaiveDate) -> Result<()> {
    fs::create_dir_all(thread_dir(outputs_path, w))
        .with_context(|| format!("mkdir thread dir for week {w}"))?;
    fs::create_dir_all(messages_dir(outputs_path, w))
        .with_context(|| format!("mkdir messages dir for week {w}"))?;
    Ok(())
}

/// Write `path` atomically via a sibling `.tmp` file then rename.
pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create parent for {}", path.display()))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(contents.as_bytes())
            .with_context(|| format!("write {}", tmp.display()))?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, path).with_context(|| {
        format!(
            "rename {} -> {}",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}

/// Escape a string for a double-quoted YAML scalar.
pub fn yaml_double_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Render one message file (front matter + body).
pub fn format_message_markdown(msg: &EmailMeta, body: &str) -> String {
    let stem = file_stem_for_id(&msg.message_id);
    let root = thread_root_id(msg);
    let in_reply = msg
        .in_reply_to
        .as_deref()
        .map(normalize_message_id);

    let mut fm = String::new();
    fm.push_str("---\n");
    fm.push_str(&format!(
        "message_id: {}\n",
        yaml_double_quoted(&msg.message_id)
    ));
    fm.push_str(&format!("subject: {}\n", yaml_double_quoted(&msg.subject)));
    fm.push_str(&format!("from: {}\n", yaml_double_quoted(&msg.from)));
    fm.push_str(&format!(
        "date: {}\n",
        yaml_double_quoted(&msg.date.to_rfc3339())
    ));
    if let Some(ref irt) = in_reply {
        fm.push_str(&format!("in_reply_to: {}\n", yaml_double_quoted(irt)));
    }
    fm.push_str(&format!(
        "thread_root_id: {}\n",
        yaml_double_quoted(&root)
    ));
    fm.push_str(&format!("file_stem: {}\n", yaml_double_quoted(&stem)));
    fm.push_str("---\n\n");
    fm.push_str(body);
    if !body.is_empty() && !body.ends_with('\n') {
        fm.push('\n');
    }
    fm
}

/// Write `{W}/messages/{stem}.md` for `msg` (idempotent overwrite).
pub fn write_message_markdown(
    outputs_path: &Path,
    w: NaiveDate,
    msg: &EmailMeta,
    body: &str,
) -> Result<PathBuf> {
    let path = message_markdown_path(outputs_path, w, &msg.message_id);
    let content = format_message_markdown(msg, body);
    write_atomic(&path, &content)?;
    Ok(path)
}

/// Empty-week `W/index.md` content.
pub fn format_empty_week_index(w: NaiveDate) -> String {
    let (start, end_excl) = week_window(w);
    let week_s = w.format("%Y-%m-%d").to_string();
    format!(
        r#"---
week_ending: {week}
headline: "No activity"
empty: true
---

# No mailing list activity in this week.

No messages in the database fell within the UTC window
`[{start}, {end})`.
"#,
        week = yaml_double_quoted(&week_s),
        start = start.format("%Y-%m-%d %H:%M:%S UTC"),
        end = end_excl.format("%Y-%m-%d %H:%M:%S UTC"),
    )
}

/// Write empty-week index.md.
pub fn write_empty_week_index(outputs_path: &Path, w: NaiveDate) -> Result<()> {
    let path = week_index_path(outputs_path, w);
    write_atomic(&path, &format_empty_week_index(w))?;
    Ok(())
}

/// One line in the root catalog.
#[derive(Debug, Clone)]
pub struct RootIndexEntry {
    pub week: NaiveDate,
    pub headline: String,
}

/// Render root `index.md` (newest weeks first).
pub fn format_root_index(entries: &[RootIndexEntry]) -> String {
    let mut out = String::from("# NFS Mailing List Weekly Summaries\n\n");
    if entries.is_empty() {
        out.push_str("_No editions yet._\n");
        return out;
    }
    for e in entries {
        let w = e.week.format("%Y-%m-%d");
        out.push_str(&format!(
            "- [Week ending {w}]({w}/index.md) — {}\n",
            e.headline
        ));
    }
    out
}

/// Write root catalog.
pub fn write_root_index(outputs_path: &Path, entries: &[RootIndexEntry]) -> Result<()> {
    let path = root_index_path(outputs_path);
    write_atomic(&path, &format_root_index(entries))?;
    Ok(())
}

/// Create empty `W/.complete` marker (call last after fsync of indexes).
pub fn write_complete_marker(outputs_path: &Path, w: NaiveDate) -> Result<()> {
    let path = complete_marker_path(outputs_path, w);
    write_atomic(&path, "")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::email_index::EmailMeta;
    use chrono::{TimeZone, Utc};
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

    #[test]
    fn format_message_uses_normalized_ids() {
        let msg = EmailMeta {
            message_id: "<abc@def.com>".into(),
            message_id_raw: " <abc@def.com>".into(),
            subject: r#"Hello "world""#.into(),
            from: "a@b".into(),
            date: Utc.with_ymd_and_hms(2026, 7, 18, 12, 0, 0).unwrap(),
            in_reply_to: Some(" <parent@x>".into()),
            references: vec![" <parent@x>".into()],
        };
        let md = format_message_markdown(&msg, "body line\n");
        assert!(md.contains("message_id: \"<abc@def.com>\""));
        assert!(md.contains("in_reply_to: \"<parent@x>\""));
        assert!(md.contains("thread_root_id: \"<parent@x>\""));
        assert!(md.contains("file_stem: \"%3Cabc%40def.com%3E\""));
        assert!(md.contains(r#"subject: "Hello \"world\"""#));
        assert!(md.contains("body line"));
    }

    #[test]
    fn empty_week_index_shape() {
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let s = format_empty_week_index(w);
        assert!(s.contains("empty: true"));
        assert!(s.contains("No mailing list activity"));
        assert!(s.contains("2026-07-14"));
        assert!(s.contains("2026-07-21"));
    }
}
