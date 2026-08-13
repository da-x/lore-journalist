//! Walk a markdown outputs tree and write a mirrored HTML tree plus `style.css`.

use super::links::fix_html_links;
use super::markdown::convert_markdown_document;
use super::page::wrap_page;
use crate::outputs::write_atomic;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use tracing::info;
use walkdir::WalkDir;

const STYLE_CSS: &str = include_str!("style.css");

/// Convert every `*.md` under `md_root` into `*.html` under `html_root`,
/// preserving relative directory structure, and write the shared stylesheet.
///
/// `site_title` is the HTML header wordmark (list-specific).
pub fn render_html_tree(md_root: &Path, html_root: &Path, site_title: &str) -> Result<()> {
    if html_root.as_os_str().is_empty() {
        bail!("html_outputs_path is empty");
    }

    std::fs::create_dir_all(html_root)
        .with_context(|| format!("create html_outputs_path {}", html_root.display()))?;

    write_atomic(&html_root.join("style.css"), STYLE_CSS)
        .with_context(|| format!("write {}/style.css", html_root.display()))?;

    let mut pages = 0usize;
    let mut written: Vec<PathBuf> = Vec::new();
    let walker = WalkDir::new(md_root).follow_links(false).into_iter();
    for entry in walker.filter_entry(|e| {
        e.file_name()
            .to_str()
            .map(|s| !s.starts_with('.'))
            .unwrap_or(false)
    }) {
        let entry = entry.with_context(|| format!("walk {}", md_root.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let rel = path
            .strip_prefix(md_root)
            .with_context(|| format!("strip prefix from {}", path.display()))?;
        let md =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let (_fm, title, body) = convert_markdown_document(&md, site_title);
        let page = wrap_page(&title, rel, &body, site_title);
        let dest = html_root.join(rel).with_extension("html");
        write_atomic(&dest, &page).with_context(|| format!("write {}", dest.display()))?;
        written.push(rel.with_extension("html"));
        pages += 1;
    }

    fix_html_links(html_root, &written)
        .with_context(|| format!("fix intra-links under {}", html_root.display()))?;

    info!(
        html_root = %html_root.display(),
        pages,
        "rendered static HTML from markdown"
    );
    Ok(())
}

/// No-op when `html_root` is unset or empty.
pub fn maybe_render_html(md_root: &Path, html_root: Option<&Path>, site_title: &str) -> Result<()> {
    let Some(html_root) = html_root else {
        return Ok(());
    };
    if html_root.as_os_str().is_empty() {
        return Ok(());
    }
    render_html_tree(md_root, html_root, site_title)
}

/// Parse an optional config string into a path (None if missing or blank).
pub fn html_dir_from_config(value: &Option<String>) -> Option<&Path> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(Path::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_pair() -> (PathBuf, PathBuf) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut md = std::env::temp_dir();
        md.push(format!("nfs-html-md-{}-{nanos}", std::process::id()));
        let mut html = std::env::temp_dir();
        html.push(format!("nfs-html-out-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&md);
        let _ = std::fs::remove_dir_all(&html);
        std::fs::create_dir_all(md.join("2026-07-20/thread")).unwrap();
        (md, html)
    }

    #[test]
    fn tree_mirrors_layout_and_skips_hidden() {
        let (md, html) = temp_pair();
        std::fs::write(
            md.join("index.md"),
            "# NFS Mailing List Weekly Summaries\n\n- [Week ending 2026-07-20](2026-07-20/index.md) — Hello\n",
        )
        .unwrap();
        std::fs::write(
            md.join("2026-07-20/index.md"),
            "---\nheadline: \"Hello\"\nempty: false\n---\n\n# Hello\n\nSee [the thread](thread/foo.md).\n",
        )
        .unwrap();
        std::fs::write(
            md.join("2026-07-20/thread/foo.md"),
            "---\ntitle: \"Foo thread\"\n---\n\n# Foo thread\n\nUse `pnfs` here.\n\n[lore](https://lore.kernel.org/linux-nfs/x/)\n",
        )
        .unwrap();
        std::fs::write(md.join("2026-07-20/.complete"), "").unwrap();
        std::fs::write(md.join(".summarize-week.lock"), "").unwrap();

        render_html_tree(&md, &html, "Weekly Summaries").unwrap();

        let root = std::fs::read_to_string(html.join("index.html")).unwrap();
        let week = std::fs::read_to_string(html.join("2026-07-20/index.html")).unwrap();
        let thread = std::fs::read_to_string(html.join("2026-07-20/thread/foo.html")).unwrap();
        let css = std::fs::read_to_string(html.join("style.css")).unwrap();

        assert!(root.contains("href=\"2026-07-20/index.html\""));
        assert!(!root.contains("2026-07-20/index.md"));
        assert!(root.contains("href=\"style.css\""));
        assert!(week.contains("href=\"thread/foo.html\""));
        assert!(week.contains("href=\"../style.css\""));
        assert!(thread.contains("<code>pnfs</code>"));
        assert!(thread.contains("href=\"https://lore.kernel.org/linux-nfs/x/\""));
        assert!(thread.contains("href=\"../../style.css\""));
        assert!(css.contains("font-family: var(--mono)"));
        assert!(!html.join("2026-07-20/.complete").exists());
        assert!(!html.join(".summarize-week.lock").exists());

        let _ = std::fs::remove_dir_all(&md);
        let _ = std::fs::remove_dir_all(&html);
    }

    #[test]
    fn percent_encoded_thread_hrefs_are_double_encoded_for_urls() {
        let (md, html) = temp_pair();
        let stem = "%3C57c6f8f6464f7ba0c0455875d4c53a0f9bf01a2c.camel%40kernel.org%3E";
        std::fs::write(
            md.join("index.md"),
            format!("# Catalog\n\n- [Week](2026-07-20/index.md)\n"),
        )
        .unwrap();
        std::fs::write(
            md.join("2026-07-20/index.md"),
            format!("# Week\n\n[GIT PULL](thread/{stem}.md)\n"),
        )
        .unwrap();
        std::fs::write(
            md.join("2026-07-20/thread").join(format!("{stem}.md")),
            "# GIT PULL\n\nbody\n",
        )
        .unwrap();

        render_html_tree(&md, &html, "Weekly Summaries").unwrap();

        let week = std::fs::read_to_string(html.join("2026-07-20/index.html")).unwrap();
        assert!(
            week.contains(
                "href=\"thread/%253C57c6f8f6464f7ba0c0455875d4c53a0f9bf01a2c.camel%2540kernel.org%253E.html\""
            ),
            "expected URL-encoded href, got: {week}"
        );
        assert!(
            !week.contains("href=\"thread/%3C57c6"),
            "href still has a single-encoded filename: {week}"
        );
        assert!(
            html.join("2026-07-20/thread")
                .join(format!("{stem}.html"))
                .is_file()
        );

        let _ = std::fs::remove_dir_all(&md);
        let _ = std::fs::remove_dir_all(&html);
    }

    #[test]
    fn maybe_render_skips_unset() {
        let (md, html) = temp_pair();
        std::fs::write(md.join("index.md"), "# Hi\n").unwrap();
        maybe_render_html(&md, None, "Weekly Summaries").unwrap();
        assert!(!html.join("index.html").exists());
        maybe_render_html(&md, Some(Path::new("")), "Weekly Summaries").unwrap();
        assert!(!html.join("index.html").exists());
        let _ = std::fs::remove_dir_all(&md);
    }

    #[test]
    fn html_dir_from_config_treats_blank_as_none() {
        assert!(html_dir_from_config(&None).is_none());
        assert!(html_dir_from_config(&Some(String::new())).is_none());
        assert!(html_dir_from_config(&Some("   ".into())).is_none());
        assert_eq!(
            html_dir_from_config(&Some("/tmp/site".into())),
            Some(Path::new("/tmp/site"))
        );
    }
}
