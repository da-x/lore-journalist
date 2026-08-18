//! Output path builders and writers for weekly markdown editions.
//!
//! Message bodies are **not** written under `outputs_path`. Citations use lore
//! permalinks (`crate::lore`); cleaned bodies stay in SQLite for LLM tools only.

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

/// `{outputs}/{W}/thread/{stem}.md` for a thread root id (any form; normalized inside).
pub fn thread_markdown_path(outputs_path: &Path, w: NaiveDate, thread_root_id: &str) -> PathBuf {
    use crate::ids::file_stem_for_id;
    let stem = file_stem_for_id(thread_root_id);
    thread_dir(outputs_path, w).join(format!("{stem}.md"))
}

/// Glob pattern (relative to `outputs_path`) for all weeks of a thread:
/// `*/thread/{stem}.md`.
pub fn prior_thread_glob_pattern(thread_root_id: &str) -> String {
    use crate::ids::file_stem_for_id;
    let stem = file_stem_for_id(thread_root_id);
    format!("*/thread/{stem}.md")
}

/// Create `W/` and `W/thread/` (no per-message archive directory).
pub fn ensure_week_layout(outputs_path: &Path, w: NaiveDate) -> Result<()> {
    fs::create_dir_all(thread_dir(outputs_path, w))
        .with_context(|| format!("mkdir thread dir for week {w}"))?;
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
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
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

/// Render root `index.md` (newest weeks first, grouped by week-ending month).
pub fn format_root_index(entries: &[RootIndexEntry], site_title: &str) -> String {
    let title = if site_title.trim().is_empty() {
        "Mailing List Weekly Summaries"
    } else {
        site_title.trim()
    };
    let mut out = format!("# {title}\n\n");
    if entries.is_empty() {
        out.push_str("_No editions yet._\n");
        return out;
    }
    let mut last_month = None;
    for e in entries {
        let month = e.week.format("%B %Y").to_string();
        if last_month.as_deref() != Some(month.as_str()) {
            if last_month.is_some() {
                out.push('\n');
            }
            out.push_str(&format!("## {month}\n\n"));
            last_month = Some(month);
        }
        let w = e.week.format("%Y-%m-%d");
        out.push_str(&format!(
            "- [Week ending {w}]({w}/index.md) — {}\n",
            e.headline
        ));
    }
    out
}

/// Write root catalog.
pub fn write_root_index(
    outputs_path: &Path,
    entries: &[RootIndexEntry],
    site_title: &str,
) -> Result<()> {
    let path = root_index_path(outputs_path);
    write_atomic(&path, &format_root_index(entries, site_title))?;
    Ok(())
}

/// Create empty `W/.complete` marker (call last after fsync of indexes).
pub fn write_complete_marker(outputs_path: &Path, w: NaiveDate) -> Result<()> {
    let path = complete_marker_path(outputs_path, w);
    write_atomic(&path, "")?;
    Ok(())
}

/// Host-built markdown list of messages for a thread file, linking to lore.
pub fn format_message_list_lore(
    lore_base: &str,
    items: &[(
        /* date label */ String,
        /* from */ String,
        /* subject */ String,
        /* message_id */ String,
    )],
) -> String {
    use crate::lore::lore_url_for_message_id;
    let mut out = String::from("## Messages this week\n\n");
    for (date, from, subject, mid) in items {
        let label = format!("{date} {from} — {subject}");
        let url = lore_url_for_message_id(lore_base, mid);
        out.push_str(&format!("- [{label}]({url})\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::file_stem_for_id;
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
            thread_markdown_path(root, w, msg),
            PathBuf::from(format!(
                "/tmp/out/2026-07-20/thread/{}.md",
                file_stem_for_id(msg)
            ))
        );
        assert_eq!(
            prior_thread_glob_pattern(msg),
            format!("*/thread/{}.md", file_stem_for_id(msg))
        );
    }

    #[test]
    fn root_index_uses_configured_title() {
        let md = format_root_index(&[], "linux-fsdevel Weekly Summaries");
        assert!(md.starts_with("# linux-fsdevel Weekly Summaries\n"));
        assert!(!md.contains("NFS"));
        assert!(!md.contains("## "));
    }

    #[test]
    fn root_index_inserts_month_dividers() {
        let md = format_root_index(
            &[
                RootIndexEntry {
                    week: NaiveDate::from_ymd_opt(2026, 8, 3).unwrap(),
                    headline: "August open".into(),
                },
                RootIndexEntry {
                    week: NaiveDate::from_ymd_opt(2026, 7, 27).unwrap(),
                    headline: "Late July".into(),
                },
                RootIndexEntry {
                    week: NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
                    headline: "Mid July".into(),
                },
                RootIndexEntry {
                    week: NaiveDate::from_ymd_opt(2025, 12, 29).unwrap(),
                    headline: "Year wrap".into(),
                },
            ],
            "Weekly Summaries",
        );
        assert_eq!(
            md,
            "# Weekly Summaries\n\
             \n\
             ## August 2026\n\
             \n\
             - [Week ending 2026-08-03](2026-08-03/index.md) — August open\n\
             \n\
             ## July 2026\n\
             \n\
             - [Week ending 2026-07-27](2026-07-27/index.md) — Late July\n\
             - [Week ending 2026-07-20](2026-07-20/index.md) — Mid July\n\
             \n\
             ## December 2025\n\
             \n\
             - [Week ending 2025-12-29](2025-12-29/index.md) — Year wrap\n"
        );
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

    #[test]
    fn message_list_uses_lore_urls() {
        let md = format_message_list_lore(
            "https://lore.kernel.org/linux-nfs/",
            &[(
                "2026-07-18".into(),
                "Alice".into(),
                "Hello".into(),
                " <20260720-tcp-read-sock-v2-6-29545d034f3c@kernel.org>".into(),
            )],
        );
        assert!(md.contains(
            "https://lore.kernel.org/linux-nfs/20260720-tcp-read-sock-v2-6-29545d034f3c@kernel.org/"
        ));
        assert!(!md.contains("messages/"));
    }
}
