//! HTML5 document shell with relative CSS and breadcrumb hrefs.

use super::unfurl::{absolute_page_url, article_published_time, escape_html, unfurl_meta_html};
use std::path::{Component, Path};

const DEFAULT_SITE_TITLE: &str = "Weekly Summaries";

/// Chrome + unfurl fields for [`wrap_page`].
#[derive(Debug, Clone, Copy)]
pub struct PageMeta<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub site_title: &'a str,
    pub site_url: Option<&'a str>,
    pub og_image: Option<&'a str>,
    pub week_ending: Option<&'a str>,
}

impl<'a> PageMeta<'a> {
    pub fn basic(title: &'a str, site_title: &'a str) -> Self {
        Self {
            title,
            description: title,
            site_title,
            site_url: None,
            og_image: None,
            week_ending: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageKind {
    Root,
    Week { week: String },
    Thread { week: String },
    Other,
}

/// Depth of the file's parent directory (normal components only).
pub fn path_depth(rel: &Path) -> usize {
    rel.parent()
        .map(|p| {
            p.components()
                .filter(|c| matches!(c, Component::Normal(_)))
                .count()
        })
        .unwrap_or(0)
}

/// Relative href to `{html_root}/style.css` from a markdown-relative path.
pub fn stylesheet_href(rel_md: &Path) -> String {
    let depth = path_depth(rel_md);
    if depth == 0 {
        "style.css".to_string()
    } else {
        format!("{}style.css", "../".repeat(depth))
    }
}

pub fn classify_page(rel_md: &Path) -> PageKind {
    let comps: Vec<&str> = rel_md
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    match comps.as_slice() {
        ["index.md"] => PageKind::Root,
        [week, "index.md"] => PageKind::Week {
            week: (*week).to_string(),
        },
        [week, "thread", _] => PageKind::Thread {
            week: (*week).to_string(),
        },
        _ => PageKind::Other,
    }
}

fn og_type_for(kind: &PageKind) -> &'static str {
    match kind {
        PageKind::Week { .. } | PageKind::Thread { .. } => "article",
        PageKind::Root | PageKind::Other => "website",
    }
}

fn crumb_items(kind: &PageKind) -> Vec<(Option<String>, String)> {
    match kind {
        PageKind::Root => Vec::new(),
        PageKind::Week { .. } => {
            vec![(Some("../index.html".into()), "Summaries".into())]
        }
        PageKind::Thread { week } => vec![
            (Some("../../index.html".into()), "Summaries".into()),
            (Some("../index.html".into()), format!("Week ending {week}")),
        ],
        PageKind::Other => {
            // Best-effort link to the catalog; still an explicit index.html.
            vec![(Some("index.html".into()), "Summaries".into())]
        }
    }
}

/// Wrap a body fragment in a full HTML5 document.
pub fn wrap_page(rel_md: &Path, body: &str, meta: &PageMeta<'_>) -> String {
    let css = stylesheet_href(rel_md);
    let kind = classify_page(rel_md);
    let title_esc = escape_html(meta.title);
    let header = if meta.site_title.trim().is_empty() {
        DEFAULT_SITE_TITLE
    } else {
        meta.site_title.trim()
    };
    let site_esc = escape_html(header);
    let home_href = match path_depth(rel_md) {
        0 => "index.html".to_string(),
        n => format!("{}index.html", "../".repeat(n)),
    };
    let page_url = meta
        .site_url
        .and_then(|base| absolute_page_url(base, rel_md));
    let published = article_published_time(meta.week_ending);
    let unfurl = unfurl_meta_html(
        meta.title,
        meta.description,
        header,
        og_type_for(&kind),
        page_url.as_deref(),
        meta.og_image,
        published.as_deref(),
    );

    let crumbs = crumb_items(&kind);
    let mut nav = String::new();
    if !crumbs.is_empty() {
        nav.push_str("      <ol class=\"crumbs\">\n");
        for (i, (href, label)) in crumbs.iter().enumerate() {
            if i > 0 {
                nav.push_str("        <li class=\"crumbs__sep\" aria-hidden=\"true\">/</li>\n");
            }
            let label_esc = escape_html(label);
            match href {
                Some(h) => nav.push_str(&format!(
                    "        <li><a href=\"{}\">{label_esc}</a></li>\n",
                    escape_html(h)
                )),
                None => nav.push_str(&format!("        <li>{label_esc}</li>\n")),
            }
        }
        nav.push_str("      </ol>\n");
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title_esc}</title>
{unfurl}  <link rel="stylesheet" href="{css}">
</head>
<body>
  <header class="site-header">
    <div class="site-header__inner">
      <p class="site-title"><a href="{home_href}">{site_esc}</a></p>
{nav}    </div>
  </header>
  <main class="content">
{body}  </main>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn stylesheet_href_by_depth() {
        assert_eq!(stylesheet_href(Path::new("index.md")), "style.css");
        assert_eq!(
            stylesheet_href(Path::new("2026-07-20/index.md")),
            "../style.css"
        );
        assert_eq!(
            stylesheet_href(Path::new("2026-07-20/thread/foo.md")),
            "../../style.css"
        );
    }

    #[test]
    fn crumbs_use_explicit_index_html() {
        let week = wrap_page(
            Path::new("2026-07-20/index.md"),
            "<p>x</p>\n",
            &PageMeta::basic("Week", "Weekly Summaries"),
        );
        assert!(week.contains("href=\"../index.html\""), "{week}");
        assert!(!week.contains("href=\"../\""), "{week}");
        assert!(week.contains("href=\"../style.css\""), "{week}");

        let thread = wrap_page(
            Path::new("2026-07-20/thread/foo.md"),
            "<p>x</p>\n",
            &PageMeta::basic("Thread", "Weekly Summaries"),
        );
        assert!(thread.contains("href=\"../../index.html\""), "{thread}");
        assert!(thread.contains("href=\"../index.html\""), "{thread}");
        assert!(thread.contains("href=\"../../style.css\""), "{thread}");
        assert!(!thread.contains("href=\"/"), "{thread}");
    }

    #[test]
    fn week_page_emits_article_unfurl_tags() {
        let meta = PageMeta {
            title: "Quiet week",
            description: "Twelve client fixes landed.",
            site_title: "NFS Weekly Summaries",
            site_url: Some("https://ex/nfs/"),
            og_image: None,
            week_ending: Some("2026-07-20"),
        };
        let week = wrap_page(Path::new("2026-07-20/index.md"), "<p>x</p>\n", &meta);
        assert!(
            week.contains("property=\"og:description\" content=\"Twelve client fixes landed.\"")
        );
        assert!(week.contains("property=\"og:type\" content=\"article\""));
        assert!(week.contains("property=\"og:site_name\" content=\"NFS Weekly Summaries\""));
        assert!(week.contains("rel=\"canonical\" href=\"https://ex/nfs/2026-07-20/index.html\""));
        assert!(
            week.contains("property=\"article:published_time\" content=\"2026-07-20T00:00:00Z\"")
        );
        assert!(week.contains("name=\"twitter:card\" content=\"summary\""));
    }

    #[test]
    fn thread_canonical_double_encodes_filename() {
        let meta = PageMeta {
            title: "GIT PULL",
            description: "Bugfixes.",
            site_title: "Weekly Summaries",
            site_url: Some("http://host/nfs/"),
            og_image: None,
            week_ending: None,
        };
        let thread = wrap_page(
            Path::new("2026-07-20/thread/%3Cfoo%40bar.com%3E.md"),
            "<p>x</p>\n",
            &meta,
        );
        assert!(
            thread.contains(
                "href=\"http://host/nfs/2026-07-20/thread/%253Cfoo%2540bar.com%253E.html\""
            ),
            "{thread}"
        );
        assert!(thread.contains("property=\"og:url\" content=\"http://host/nfs/2026-07-20/thread/%253Cfoo%2540bar.com%253E.html\""));
    }

    #[test]
    fn classifies_layout_paths() {
        assert_eq!(classify_page(Path::new("index.md")), PageKind::Root);
        assert_eq!(
            classify_page(Path::new("2026-07-20/index.md")),
            PageKind::Week {
                week: "2026-07-20".into()
            }
        );
        assert_eq!(
            classify_page(Path::new("2026-07-20/thread/abc.md")),
            PageKind::Thread {
                week: "2026-07-20".into()
            }
        );
    }
}
